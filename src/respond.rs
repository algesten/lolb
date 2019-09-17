use crate::peek::Peekable;
use crate::Socket;
use bytes::Bytes;
use h2::server::SendResponse;

pub(crate) enum Responder<'a, S>
where
    S: Socket,
{
    Http2(SendResponse<Bytes>),
    Http11(&'a mut Peekable<S>),
}
