use crate::body::PollCapacity;
use crate::conn::Socket;
use crate::{LolbResult, RecvBody};

/// Holder of the actual connection to the service.
#[derive(Debug, Clone)]
pub struct ServiceConnection(pub h2::client::SendRequest<bytes::Bytes>);

impl ServiceConnection {
    /// Send request + request body to service.
    pub(crate) async fn send_request<'a, S>(
        self,
        req: http::Request<RecvBody<'a, S>>,
    ) -> LolbResult<http::Response<h2::RecvStream>>
    where
        S: Socket,
    {
        // wait for h2 conn to be ready to receive req
        let mut h2 = self.0.ready().await?;

        // reconstitute req to Request<()>
        let (parts, mut body) = req.into_parts();
        let req = http::Request::from_parts(parts, ());

        // send request + headers
        let (response, mut send_body) = h2.send_request(req, false).unwrap();

        // send body
        loop {
            // read next body chunk from incoming
            if let Some(read_body_res) = body.data().await {
                let mut body_data = read_body_res?;
                // try pushing data up to service as capacity becomes available.
                while !body_data.is_empty() {
                    // reserving capacity to send
                    send_body.reserve_capacity(body_data.len());
                    // wait for capacity to be available
                    let actual_capacity = PollCapacity(&mut send_body).await?;
                    // then send it over to service
                    let send_len = actual_capacity.min(body_data.len());
                    let to_send = body_data.slice_to(send_len);
                    send_body.send_data(to_send, false)?;
                    // once sent, release the corresponding amount from incoming
                    body.release_capacity(send_len)?;
                    // move pointer in what is yet to send in current chunk.
                    body_data = body_data.slice_from(send_len);
                }
            } else {
                // no more body data
                let empty = bytes::Bytes::new();
                send_body.send_data(empty, true)?; // true here is end-of-stream
                break;
            }
        }

        Ok(response.await?)
    }
}
