use crate::chunked::{ChunkedDecoder, ChunkedEncoder};
use crate::limit::{LimitRead, LimitWrite};
use crate::peek::Peekable;
use crate::Socket;
use crate::{AsyncRead, AsyncReadExt, LolbError, LolbResult};
use bytes::Bytes;

/// Helper type to unite body stream reading for http11 and http2
pub(crate) enum RecvBody<'a, S>
where
    S: Socket,
{
    Http2(h2::RecvStream),
    Http11Plain(LimitRead<&'a mut Peekable<S>>),
    Http11Chunked(ChunkedDecoder),
}

impl<'a, S: Socket> RecvBody<'a, S> {
    pub async fn data(&mut self) -> Option<LolbResult<Bytes>> {
        match self {
            RecvBody::Http2(r) => r.data().await.map(|r| Ok(r?)),
            RecvBody::Http11Plain(r) => read_chunk(r).await,
            RecvBody::Http11Chunked(_) => unimplemented!(), // TODO (see chunked.rs)
        }
    }

    pub fn release_capacity(&mut self, amount: usize) -> LolbResult<()> {
        match self {
            RecvBody::Http2(r) => Ok(r.release_capacity().release_capacity(amount)?),
            _ => Ok(()),
        }
    }
}

async fn read_chunk<S: AsyncRead + Unpin>(s: &mut S) -> Option<LolbResult<Bytes>> {
    const BUF_SIZE: usize = 16_384;
    let mut chunk = bytes::BytesMut::with_capacity(BUF_SIZE);
    chunk.resize(BUF_SIZE, 0x0);
    match s.read(&mut chunk[0..BUF_SIZE]).await {
        Ok(read) => {
            if read == 0 {
                None
            } else {
                chunk.resize(read, 0x0);
                Some(Ok(chunk.into()))
            }
        }
        Err(e) => Some(Err(LolbError::Io(e))),
    }
}

pub(crate) enum Http11BodyRead {
    Plain {
        bytes_read: usize,
        content_length: usize,
    },
}

pub(crate) enum SendBody<'a, S>
where
    S: Socket,
{
    Http2Client(h2::SendStream<Bytes>),
    Http2Server(h2::server::SendResponse<Bytes>),
    Http11(LimitWrite<&'a mut Peekable<S>>),
    Http11Chunked(ChunkedEncoder),
}

pub(crate) struct PollCapacity<'a, B: bytes::IntoBuf>(pub &'a mut h2::SendStream<B>);

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

impl<'a, B: bytes::IntoBuf> Future for PollCapacity<'a, B> {
    type Output = Result<usize, h2::Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.get_mut().0).poll_capacity(cx) {
            Poll::Ready(Some(res)) => Poll::Ready(res),
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }
}
