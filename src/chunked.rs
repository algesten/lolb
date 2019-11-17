use crate::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::{LolbError, LolbResult};
use bytes::Bytes;
use std::io;

/// Decode AsyncRead as transfer-encoding chunked.
pub(crate) struct ChunkedDecoder<S: AsyncRead> {
    socket: S,
    amount_left: usize,
}

/// Amount we max read from the underlying chunked stream. To not use up too much memory.
const MAX_READ_SIZE: usize = 1024 * 1024;

impl<S: AsyncRead + Unpin> ChunkedDecoder<S> {
    pub fn new(socket: S) -> Self {
        ChunkedDecoder {
            socket,
            amount_left: 0,
        }
    }

    pub async fn data(&mut self) -> Option<LolbResult<Bytes>> {
        match self.data_res().await {
            Ok(Some(r)) => Some(Ok(r)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }

    async fn data_res(&mut self) -> LolbResult<Option<Bytes>> {
        if self.amount_left == 0 {
            let chunk_size = self.read_chunk_size().await?;
            if chunk_size == 0 {
                return Ok(None);
            }
            self.amount_left = chunk_size;
        }
        let to_read = self.amount_left.min(MAX_READ_SIZE);
        self.amount_left -= to_read;
        let mut buf = Vec::with_capacity(to_read);
        self.read_buf(to_read, &mut buf).await?;
        self.skip_until_lf().await?; // skip trailing \r\n
        Ok(Some(buf.into()))
    }

    // 3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n
    async fn read_chunk_size(&mut self) -> LolbResult<usize> {
        let mut buf = Vec::with_capacity(16);

        // read until we get a non-numeric character. this could be
        // either \r or maybe a ; if we are using "extensions"
        loop {
            self.read_buf(1, &mut buf).await?;
            let c: char = buf[buf.len() - 1].into();
            // keep reading until we get ; or \r
            if !c.is_numeric() {
                break;
            }
        }

        self.skip_until_lf().await?;

        // no length, no number to parse.
        if buf.is_empty() {
            return Ok(0);
        }

        // parse the read numbers as a chunk size.
        let chunk_size = String::from_utf8(buf)
            .ok()
            .and_then(|c| usize::from_str_radix(c.trim(), 16).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Not a number in chunk size")
            })?;

        Ok(chunk_size)
    }

    // skip until we get a \n
    async fn skip_until_lf(&mut self) -> LolbResult<()> {
        // skip until we get a \n
        let mut one = [0_u8; 1];
        loop {
            self.read_amount(&mut one[..]).await?;
            if one[0] == b'\n' {
                break;
            }
        }
        Ok(())
    }

    async fn read_buf(&mut self, amount: usize, buf: &mut Vec<u8>) -> LolbResult<()> {
        buf.reserve(amount);
        let len = buf.len();
        buf.resize(len + amount, 0);
        self.read_amount(&mut buf[len..len + amount]).await?;
        Ok(())
    }

    async fn read_amount(&mut self, buf: &mut [u8]) -> LolbResult<()> {
        let read = self.socket.read_exact(buf).await?;
        if read != buf.len() {
            return Err(LolbError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Read amount less than expected {} < {}", read, buf.len()),
            )));
        }
        Ok(())
    }
}

/// Transfer encoding chunked to an AsyncWrite
pub(crate) struct ChunkedEncoder<S: AsyncWrite + Unpin>(pub S);

impl<S: AsyncWrite + Unpin> ChunkedEncoder<S> {
    pub async fn send_chunk(&mut self, buf: Bytes) -> LolbResult<()> {
        let header = format!("{}\r\n", buf.len()).into_bytes();
        self.0.write_all(&header[..]).await?;
        self.0.write_all(&buf[..]).await?;
        const CRLF: &[u8] = b"\r\n";
        self.0.write_all(CRLF).await?;
        Ok(())
    }
    pub async fn send_finish(&mut self) -> LolbResult<()> {
        const END: &[u8] = b"0\r\n\r\n";
        self.0.write_all(END).await?;
        Ok(())
    }
}
