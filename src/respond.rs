use crate::body::PollCapacity;
use crate::error::LolbResult;
use crate::http11;
use crate::peek::Peekable;
use crate::AsyncWriteExt;
use crate::Socket;
use bytes::Bytes;
use h2::server::SendResponse;
use std::io;

pub(crate) enum Responder<'a, S>
where
    S: Socket,
{
    Http2(SendResponse<Bytes>),
    Http11(&'a mut Peekable<S>),
}

impl<'a, S: Socket> Responder<'a, S> {
    pub async fn send_response(self, res_body: http::Response<h2::RecvStream>) -> LolbResult<()> {
        let (part, mut body) = res_body.into_parts();
        let res = http::Response::from_parts(part, ());
        match self {
            Responder::Http2(mut r) => {
                let mut send_body = r.send_response(res, false)?;
                let mut release = { body.release_capacity().clone() };
                while let Some(chunk) = body.data().await {
                    let mut body_data = chunk?;
                    while !body_data.is_empty() {
                        // reserving capacity to send
                        send_body.reserve_capacity(body_data.len());
                        // wait for capacity to be available
                        let actual_capacity = PollCapacity(&mut send_body).await?;
                        // then send it over to the client
                        let send_len = actual_capacity.min(body_data.len());
                        let to_send = body_data.slice_to(send_len);
                        send_body.send_data(to_send, false)?;
                        // once sent, release the corresponding amount from incoming
                        release.release_capacity(send_len)?;
                        // move pointer in what is yet to send in current chunk.
                        body_data = body_data.slice_from(send_len);
                    }
                }
                // no more body data
                let empty = bytes::Bytes::new();
                send_body.send_data(empty, true)?; // true here is end-of-stream
            }
            Responder::Http11(socket) => {
                // write the http1.1 header into a buffer
                let mut buf = io::Cursor::new(vec![0_u8; 4096]);
                http11::write_http11_response(&mut buf, res)?;
                let header = buf.into_inner();
                // async write header to the socket
                AsyncWriteExt::write_all(&mut socket.wrapped, &header[..]).await?;
                // send body.
                while let Some(chunk) = body.data().await {
                    let body_data = chunk?;
                    AsyncWriteExt::write_all(&mut socket.wrapped, &body_data[..]).await?;
                }
            }
        }
        Ok(())
    }
}
