use crate::{AsyncRead, AsyncWrite, LolbError};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Helper to make an AsyncRead that reads up to a limit of bytes.
pub(crate) struct LimitRead<S>
where
    S: AsyncRead,
{
    source: S,
    read: usize,
    limit: usize,
}

impl<S: AsyncRead> LimitRead<S> {
    pub fn new(source: S, limit: usize) -> Self {
        LimitRead {
            source,
            read: 0,
            limit,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for LimitRead<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let max = (self.limit - self.read).min(buf.len());
        Pin::new(&mut self.get_mut().source).poll_read(cx, &mut buf[0..max])
    }
}

/// Helper to make an AsyncWrite that checks we only write a fixed number of bytes.
pub(crate) struct LimitWrite<S>
where
    S: AsyncWrite,
{
    source: S,
    written: usize,
    limit: usize,
}

impl<S: AsyncWrite> LimitWrite<S> {
    pub fn new(source: S, limit: usize) -> Self {
        LimitWrite {
            source,
            written: 0,
            limit,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for LimitWrite<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let total = self.written + buf.len();
        if total > self.limit {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                LolbError::Owned(format!(
                    "More bytes than LimitWrite allows: {} > {}",
                    total, self.limit
                )),
            )));
        }
        let self_mut = self.get_mut();
        match Pin::new(&mut self_mut.source).poll_write(cx, buf) {
            Poll::Ready(r) => {
                let wr = r?;
                self_mut.written += wr;
                Poll::Ready(Ok(wr))
            }
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().source).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().source).poll_shutdown(cx)
    }
}
