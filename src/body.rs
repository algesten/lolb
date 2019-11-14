use crate::chunked::{ChunkedDecoder, ChunkedEncoder};
use crate::limit::{LimitRead, LimitWrite};
use crate::peek::Peekable;
use crate::Socket;
use crate::{AsyncRead, AsyncReadExt, AsyncWrite, LolbError, LolbResult};
use bytes::{Bytes, BytesMut};
use std::io;

/// Helper type to unite body stream reading for http11 and http2
pub(crate) enum RecvBody<'a, S>
where
    S: Socket,
{
    Http2(h2::RecvStream),
    Http11Plain(LimitRead<&'a mut Peekable<S>>),
    Http11Chunked(ChunkedDecoder<&'a mut Peekable<S>>),
}

impl<'a, S: Socket> RecvBody<'a, S> {
    pub async fn data(&mut self) -> Option<LolbResult<Bytes>> {
        match self {
            RecvBody::Http2(r) => r.data().await.map(|r| Ok(r?)),
            RecvBody::Http11Plain(r) => read_chunk(r).await,
            RecvBody::Http11Chunked(r) => read_chunk(r).await,
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
    let mut chunk = BytesMut::with_capacity(BUF_SIZE);
    chunk.resize(BUF_SIZE, 0x0);
    match s.read(&mut chunk[..]).await {
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

pub(crate) enum Http11Body<'a, S>
where
    S: Socket,
{
    NoBody,
    Limited(LimitWrite<&'a mut S>),
    Chunked(ChunkedEncoder<&'a mut S>),
}

// tedious proxying
impl<'a, S: Socket> AsyncWrite for Http11Body<'a, S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match self.get_mut() {
            Http11Body::NoBody => Poll::Ready(Ok(0)),
            Http11Body::Limited(w) => Pin::new(w).poll_write(cx, buf),
            Http11Body::Chunked(w) => Pin::new(w).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Http11Body::NoBody => Poll::Ready(Ok(())),
            Http11Body::Limited(w) => Pin::new(w).poll_flush(cx),
            Http11Body::Chunked(w) => Pin::new(w).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match self.get_mut() {
            Http11Body::NoBody => Poll::Ready(Ok(())),
            Http11Body::Limited(w) => Pin::new(w).poll_shutdown(cx),
            Http11Body::Chunked(w) => Pin::new(w).poll_shutdown(cx),
        }
    }
}
