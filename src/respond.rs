use crate::body::Http11Body;
use crate::body::PollCapacity;
use crate::chunked::ChunkedEncoder;
use crate::error::LolbResult;
use crate::http11;
use crate::limit::LimitWrite;
use crate::peek::Peekable;
use crate::AsyncWriteExt;
use crate::Socket;
use bytes::Bytes;
use h2::server::SendResponse;
use h2::RecvStream;
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
        let (part, body) = res_body.into_parts();
        let res = http::Response::from_parts(part, ());
        match self {
            Responder::Http2(send_res) => {
                send_response_http2(send_res, res, body).await?;
            }
            Responder::Http11(socket) => {
                send_response_http1(&mut socket.wrapped, res, body).await?;
            }
        }
        Ok(())
    }
}

async fn send_response_http2(
    mut send_res: SendResponse<Bytes>,
    res: http::Response<()>,
    mut body: RecvStream,
) -> LolbResult<()> {
    let is_end = body.is_end_stream();
    let mut send_body = send_res.send_response(res, is_end)?;
    if is_end {
        // do not attempt to propagate body
        return Ok(());
    }
    // propagate body
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
    Ok(())
}

async fn send_response_http1<S: Socket>(
    socket: &mut S,
    mut res: http::Response<()>,
    mut body: RecvStream,
) -> LolbResult<()> {
    // figure out if we are to send the response as chunked.
    let content_len = res
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());
    let is_end = body.is_end_stream();
    let send_chunked = content_len.is_none() && !is_end;

    // add chunked header
    if send_chunked {
        trace!("Send chunked http11 response");
        res.headers_mut().insert(
            "transfer-encoding",
            http::header::HeaderValue::from_static("chunked"),
        );
    }

    // write the http1.1 header into a buffer
    let mut buf = io::Cursor::new(vec![0_u8; 4096]);
    http11::write_http11_response(&mut buf, res)?;
    let header = buf.into_inner();

    // async write header to the socket
    AsyncWriteExt::write_all(socket, &header[..]).await?;

    // wrapper object depending on how we are to send the body
    let mut http11body = if send_chunked {
        Http11Body::Chunked(ChunkedEncoder(socket))
    } else {
        match content_len {
            None => Http11Body::NoBody,
            Some(size) => Http11Body::Limited(LimitWrite::new(socket, size)),
        }
    };

    // send body.
    while let Some(chunk) = body.data().await {
        let body_data = chunk?;
        AsyncWriteExt::write_all(&mut http11body, &body_data[..]).await?;
    }
    Ok(())
}
