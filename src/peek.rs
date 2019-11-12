use crate::{AsyncRead, AsyncReadExt, AsyncWrite};
use bytes::BytesMut;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

/**
 * Wrapper to make any AsyncRead peekable.
 */
#[derive(Debug)]
pub struct Peekable<R>
where
    R: io::Read + AsyncRead + Unpin + io::Write,
{
    pub(crate) wrapped: R,
    buffered: BytesMut,
}

impl<R: io::Read + AsyncRead + Unpin + io::Write + AsyncWrite> Peekable<R> {
    pub fn new(wrapped: R) -> Self {
        Peekable {
            wrapped,
            buffered: BytesMut::new(),
        }
    }

    pub async fn peek(
        &mut self,
        buf: &mut [u8],
        is_enough: &dyn Fn(&[u8]) -> bool,
    ) -> io::Result<usize> {
        let mut total = self.buffered.len();

        // ensure we have enough space in the buffered to hold the amount needed to peek.
        if self.buffered.len() < buf.len() {
            self.buffered.resize(buf.len(), 0x0);
        }

        // fill buffered
        while total < buf.len() {
            let read = AsyncReadExt::read(&mut self.wrapped, &mut self.buffered[total..]).await?;
            if read == 0 {
                // end
                break;
            }
            total += read;
            if is_enough(&self.buffered[0..total]) {
                break;
            }
        }

        // at this point we have total or enough amount of bytes to copy out.
        (&mut buf[0..total]).copy_from_slice(&self.buffered[0..total]);

        Ok(total)
    }
}

impl<R: io::Read + AsyncRead + Unpin + io::Write> io::Read for Peekable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.wrapped, buf)
    }
}

impl<R: io::Read + AsyncRead + Unpin + io::Write> AsyncRead for Peekable<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // if we have buffered, provide that first.
        if !self.buffered.is_empty() {
            let max = self.buffered.len().min(buf.len());
            let removed = self.get_mut().buffered.split_to(max);
            (&mut buf[0..max]).copy_from_slice(&removed[..]);
            return Poll::Ready(Ok(max));
        }

        // nothing more buffered, delegate to wrapped
        Pin::new(&mut self.get_mut().wrapped).poll_read(cx, buf)
    }
}

impl<R: io::Read + AsyncRead + Unpin + io::Write> AsyncWrite for Peekable<R>
where
    R: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.get_mut().wrapped).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().wrapped).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().wrapped).poll_shutdown(cx)
    }
}
