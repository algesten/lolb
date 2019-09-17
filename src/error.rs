use std::fmt;
use std::io;

pub type LolbResult<T> = std::result::Result<T, LolbError>;

#[derive(Debug)]
pub enum LolbError {
    Message(&'static str),
    Owned(String),
    Io(io::Error),
    Acme(acme_lib::Error),
    H2(h2::Error),
    Http11Parse(httparse::Error),
    Http(http::Error),
}
use LolbError::*;

impl std::error::Error for LolbError {}

impl fmt::Display for LolbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Message(s) => write!(f, "{}", s),
            Owned(s) => write!(f, "{}", s),
            Io(e) => write!(f, "io: {}", e),
            Acme(e) => write!(f, "Acme: {}", e),
            H2(e) => write!(f, "h2: {}", e),
            Http11Parse(e) => write!(f, "http11parse: {}", e),
            Http(e) => write!(f, "http: {}", e),
        }
    }
}

impl From<io::Error> for LolbError {
    fn from(e: io::Error) -> Self {
        LolbError::Io(e)
    }
}

impl From<acme_lib::Error> for LolbError {
    fn from(e: acme_lib::Error) -> Self {
        LolbError::Acme(e)
    }
}

impl From<h2::Error> for LolbError {
    fn from(e: h2::Error) -> Self {
        LolbError::H2(e)
    }
}

impl From<httparse::Error> for LolbError {
    fn from(e: httparse::Error) -> Self {
        LolbError::Http11Parse(e)
    }
}

impl From<http::Error> for LolbError {
    fn from(e: http::Error) -> Self {
        LolbError::Http(e)
    }
}
