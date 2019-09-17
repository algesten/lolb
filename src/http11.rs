use crate::body::RecvBody;
use crate::chunked::ChunkedDecoder;
use crate::conn::{Connection, Socket};
use crate::limit::LimitRead;
use crate::{AsyncReadExt, LolbError, LolbResult};
use std::io;

// Request headers today vary in size from ~200 bytes to over 2KB.
// As applications use more cookies and user agents expand features,
// typical header sizes of 700-800 bytes is common.
// http://dev.chromium.org/spdy/spdy-whitepaper
const HTTP11_PARSE_BUF_SIZE: usize = 16_384;

pub(crate) async fn parse_http11<'a, S>(
    conn: &'a mut Connection<S>,
) -> LolbResult<Option<http::Request<RecvBody<'a, S>>>>
where
    S: Socket,
{
    let scheme = if conn.is_secure() { "https" } else { "http" };
    let mut buf = vec![0; HTTP11_PARSE_BUF_SIZE];

    // peek and parse until we got enough in the buffer for the entire http header
    let peeked_amount = conn
        .socket()
        .peek(&mut buf, &|so_far| {
            try_parse_http11(so_far, scheme)
                .ok()
                .and_then(|v| v)
                .is_some()
        })
        .await?;

    // TODO an obvious optimization would be to reuse the parse result from the peek
    let result = try_parse_http11(&buf[0..peeked_amount], scheme)?;

    if result.is_none() {
        // we failed to parse a header within HTTP11_PARSE_BUF_SIZE
        if peeked_amount < HTTP11_PARSE_BUF_SIZE {
            // stream ended before we managed to parse a header
            return Ok(None);
        } else {
            return Err(LolbError::Owned(format!(
                "Failed to parse http11 header: {:?}…",
                &buf[0..40]
            )));
        }
    }

    let (mut req, header_len) = result.unwrap();

    // discard header_len from the socket to position it where the body starts.
    conn.socket().read(&mut buf[0..header_len]).await?;

    let recv_chunked = req
        .headers_mut()
        // transfer-encoding is not allowed in http2, so we remove it
        .remove("transfer-encoding")
        .map(|h| h.to_str().unwrap_or("").to_ascii_lowercase() == "chunked")
        .unwrap_or(false);

    let content_len = req
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let (parts, _) = req.into_parts();

    let body = if recv_chunked {
        RecvBody::Http11Chunked(ChunkedDecoder())
    } else {
        RecvBody::Http11Plain(LimitRead::new(conn.socket(), content_len))
    };

    Ok(Some(http::Request::from_parts(parts, body)))
}

fn try_parse_http11(buf: &[u8], scheme: &str) -> LolbResult<Option<(http::Request<()>, usize)>> {
    let mut headers = [httparse::EMPTY_HEADER; 128];
    let mut request = httparse::Request::new(&mut headers);
    let status = request.parse(buf)?;
    if status.is_partial() {
        return Ok(None);
    }
    let mut bld = http::Request::builder();

    bld.version(http::Version::HTTP_2);

    if let Some(method) = request.method {
        bld.method(method);
    }

    let mut authority = None;
    for head in request.headers.iter() {
        // the upstream is always http2, so we
        // translate host to :authority
        if head.name.to_ascii_lowercase() == "host" {
            authority = Some(head.value);
        } else {
            bld.header(head.name, head.value);
        }
    }

    let mut uri = http::uri::Builder::new();
    uri.scheme(scheme);
    if let Some(authority) = authority {
        uri.authority(authority);
    }
    if let Some(path) = request.path {
        uri.path_and_query(path);
    }
    bld.uri(uri.build()?);

    let head_len = status.unwrap();

    Ok(bld.body(()).map(|r| Some((r, head_len)))?)
}

/// Helper with generic writer.
pub(crate) fn write_http11_response<W: io::Write, X>(
    w: &mut W,
    response: http::Response<X>,
) -> io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {} {}\r\n",
        response.status().as_u16(),
        response.status().canonical_reason().unwrap_or("Unknown")
    )?;
    for (name, value) in response.headers() {
        write!(w, "{}: ", name)?;
        w.write_all(value.as_bytes())?;
        write!(w, "\r\n")?;
    }
    write!(w, "\r\n")?;
    Ok(())
}
