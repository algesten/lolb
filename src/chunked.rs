use crate::{AsyncRead, AsyncWrite};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

// TODO decode chunked encoding in an async way.
pub(crate) struct ChunkedDecoder<S: AsyncRead>(pub S);

// TODO encode chunked encoding in an async way.
pub(crate) struct ChunkedEncoder<S: AsyncWrite>(pub S);

impl<S: AsyncRead> AsyncRead for ChunkedDecoder<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        unimplemented!()
    }
}

impl<S: AsyncWrite> AsyncWrite for ChunkedEncoder<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        unimplemented!()
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        unimplemented!()
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        unimplemented!()
    }
}
