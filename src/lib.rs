#![warn(clippy::all)]
#![allow(dead_code)]

#[macro_use]
extern crate log;

use bytes::Buf;
use std::future::Future;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

pub(crate) use tokio_io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod body;
mod chunked;
mod conn;
mod error;
mod http11;
mod limit;
pub mod peek;
pub mod persist;
mod respond;
mod serv_auth;
mod serv_conn;
mod service;
mod util;

use body::*;
use conn::*;
pub use error::*;
use respond::*;
use serv_auth::*;
use serv_conn::*;
use service::*;

use crate::persist::{load_preauthed, Persist};
use acme_lib::Account;

pub(crate) const PATH_NODE_REGISTER: &str = "/__lolb_node_register";
pub(crate) const PATH_KEEP_ALIVE: &str = "/__lolb_keep_alive";

/// A load balancer instance.
pub struct LoadBalancer<P>
where
    P: Persist,
{
    /// Persistence for saving/loading stuff.
    persist: P,
    /// The acme account to use for managing TLS certificates.
    account: Account<P>,
    /// Configured serviced domains.
    services: Services,
}

pub async fn accept_incoming<P, S, R, F>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    mut provider: R,
) -> LolbResult<()>
where
    P: Persist,
    S: Socket,
    S: 'static,
    R: ConnectionProvider<S, F>,
    F: Future<Output = LolbResult<Connection<S>>>,
{
    loop {
        // wait for provider to produce the next incoming connection. A failure here
        // means we abort the entire handling.
        let conn = provider.accept().await?;

        // async handling of incoming request.
        match handle_incoming(lb.clone(), conn).await {
            Ok(_) => {}
            // requests fail, that's life on the internet. just debug output in case
            // it's needed for hunting bugs.
            Err(e) => debug!("{}", e),
        }
    }
}

async fn handle_incoming<P, S>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    mut conn: Connection<S>,
) -> LolbResult<()>
where
    P: Persist,
    S: Socket,
    S: 'static,
{
    // First we must check if the incoming connection is a preauthed service connection.
    // If it is, then we are acting as an h2 client instead of a server.
    //
    let mut peeked = vec![0; PREAUTH_LEN];
    let read = conn.socket().peek(&mut peeked, &|_| false).await?;

    // did we manage to peek enough bytes?
    if read < PREAUTH_LEN {
        return Err(LolbError::Owned(format!(
            "Stream ended when {} < {} bytes peeked for preauth.",
            read, PREAUTH_LEN
        )));
    }

    // This check must be fast for client request that are to be routed.
    if &peeked[0..4] == PREAUTH_PREFIX {
        // This appears to be a preauthed service connection. To check the auth
        // we need to determine if the remaining 8 bytes corresponds to
        // any preauthed secret.
        let n = Cursor::new(&mut peeked[4..]).get_u64_be();
        let authed = {
            let key = PreAuthedKey(n);
            let lock = lb.lock().unwrap();
            load_preauthed(&lock.persist, &key).await?
        };
        if let Some(authed) = authed {
            // Discard the preauth from the incoming bytes.
            let read = conn.socket().read(&mut peeked).await?;
            if read < PREAUTH_LEN {
                panic!("Discarded less than peeked length of preauth");
            }
            // Start handling the authed service. This is where we start acting as an
            // h2 client instead of a server.
            add_preauthed_service(lb.clone(), conn, authed).await?;
            return Ok(());
        } else {
            // sending "lolb" without any corresonding preauth is an error
            return Err(LolbError::Message("No preauth for incoming 'lolb' prefix"));
        }
    }

    // This is a "normal" client connection that most likely should be routed to a service,
    // but could also be an incoming service connection doing an auth. Either way, we are to
    // act "server" to the incoming connection.
    handle_client(lb.clone(), conn).await?;

    Ok(())
}

async fn add_preauthed_service<P, S>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    mut conn: Connection<S>,
    preauthed: PreAuthed,
) -> LolbResult<()>
where
    P: Persist,
    S: Socket,
{
    // Start an h2 client against this service.
    let (h2, conn) = h2::client::handshake(conn.socket()).await?;

    // The idea is that the drive closure below retains the strong reference to
    // the service connection and the weak reference goes into the service routing
    // logic. Thus on disconnect, the weak refererence will instantly be invalid.
    let service_conn = ServiceConnection(h2);
    let strong = Arc::new(service_conn);
    let weak = Arc::downgrade(&strong);

    let drive = async move {
        let _strong = strong;
        if let Err(e) = conn.await {
            // service probably disconnected. that's expected.
            debug!("Service disconnect: {}", e);
        }
    };

    // tokio::spawn(drive);

    // add service connection to service definitions.
    let mut lock = lb.lock().unwrap();
    lock.services.add_preauthed(preauthed, weak);

    Ok(())
}

async fn handle_client<P, S>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    mut conn: Connection<S>,
) -> LolbResult<()>
where
    P: Persist,
    S: Socket,
    S: 'static,
{
    let mut http_version = conn.http_version(); // this can be set by alpn from TLS negotiation.

    // if the http version is not known, we need to peek the beginning to
    // see if we find the http2 preface.
    if http_version == HttpVersion::Unknown {
        const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
        let mut buf = vec![0; H2_PREFACE.len()];
        let read = conn.socket().peek(&mut buf, &|_| false).await?;
        if read < buf.len() {
            return Err(LolbError::Owned(format!(
                "Stream ended when {} < {} bytes peeked for http2 check.",
                read,
                H2_PREFACE.len()
            )));
        }
        http_version = if buf == H2_PREFACE {
            HttpVersion::Http2
        } else {
            HttpVersion::Http11
        };
    }

    // Here we normalize the incoming requests that can be either http11 or http2
    // to a common format for routing, then normalize the responses to a common format
    // for responding.
    if http_version == HttpVersion::Http2 {
        let mut h2 = h2::server::handshake(conn.socket()).await?;
        let mut check_service_auth = true;
        // http2 can have several streams (requests) in the same socket.
        while let Some(r) = h2.accept().await {
            let (h2req, send_resp) = r?;
            let (parts, body) = h2req.into_parts();
            let req = http::Request::from_parts(parts, RecvBody::<S>::Http2(body));

            // we only check service auth once in the first stream.
            if check_service_auth {
                check_service_auth = false;
                if is_service_auth(lb.clone(), &req) {
                    // this is a service auth request, deal with it and no further processing
                    // of streams in this h2 connection.
                    handle_service_auth(lb.clone(), req).await?;
                    return Ok(());
                }
            }

            // route request to service and wait for a response
            let res = request_to_service(lb.clone(), req).await?;
            let respond = Responder::<S>::Http2(send_resp);
            respond.send_response(res).await?;
        }
    } else if http_version == HttpVersion::Http11 {
        // http11 have one request at a time.
        while let Some(req) = http11::parse_http11(&mut conn).await? {
            // route request to service and wait for a response
            let res = request_to_service(lb.clone(), req).await?;
            let respond = Responder::Http11(conn.socket());
            respond.send_response(res).await?;
        }
    } else {
        panic!("Unknown http version after peek: {:?}", http_version);
    }

    Ok(())
}

/// Check if this request is a service auth.
pub(crate) fn is_service_auth<'a, P, S>(
    _lb: Arc<Mutex<LoadBalancer<P>>>,
    req: &http::Request<RecvBody<'a, S>>,
) -> bool
where
    P: Persist,
    S: Socket,
{
    req.uri().path() == PATH_NODE_REGISTER
}

/// Authenticate incoming service auth.
async fn handle_service_auth<'a, P, S>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    req: http::Request<RecvBody<'a, S>>,
) -> LolbResult<()>
where
    P: Persist,
    S: Socket,
{
    Ok(())
}

/// Route a normalized request to a matching service.
async fn request_to_service<'a, P, S>(
    lb: Arc<Mutex<LoadBalancer<P>>>,
    req: http::Request<RecvBody<'a, S>>,
) -> LolbResult<http::Response<h2::RecvStream>>
where
    P: Persist,
    S: Socket,
{
    let s_conn = {
        let mut lock = lb.lock().unwrap();
        lock.services.route(&req)
    };
    if let Some(s_conn) = s_conn {
        Ok(s_conn.send_request(req).await?)
    } else {
        Err(LolbError::Message("No service accepted incoming request"))
    }
}
