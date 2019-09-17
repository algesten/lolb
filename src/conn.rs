use crate::peek::Peekable;
use crate::{AsyncRead, AsyncWrite, LolbResult};
use std::future::Future;
use std::io::{Read, Write};

pub trait Socket: Read + Write + AsyncRead + AsyncWrite + Unpin {}

/// Trait for providing incoming connections to LoadBalancer::handle_incoming()
pub trait ConnectionProvider<S, F>
where
    S: Socket,
    F: Future<Output = LolbResult<Connection<S>>>,
{
    fn accept(&mut self) -> F;
}

#[derive(Debug)]
pub struct Connection<S>
where
    S: Socket,
{
    /// The underlying socket.
    socket: Peekable<S>,
    /// Whether we are treating the connection as http11 or http2. This is in the
    /// ALPN negotiated when using TLS.
    http_version: HttpVersion,
    /// Whether request was secure.
    is_secure: bool,
}

impl<S: Socket> Connection<S> {
    pub fn new(socket: S, http_version: HttpVersion, is_secure: bool) -> Self {
        Connection {
            socket: Peekable::new(socket),
            http_version,
            is_secure,
        }
    }

    pub fn socket(&mut self) -> &mut Peekable<S> {
        &mut self.socket
    }

    pub fn http_version(&self) -> HttpVersion {
        self.http_version
    }

    pub fn is_secure(&self) -> bool {
        self.is_secure
    }
}

/// The version of http connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    /// Not known (requires peeking)
    Unknown,
    /// HTTP/1.1
    Http11,
    /// HTTP/2
    Http2,
}
