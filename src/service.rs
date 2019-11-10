use crate::body::PollCapacity;
use crate::conn::Socket;
use crate::util::ArcExt;
use crate::{LolbResult, RecvBody};
use acme_lib::Certificate;
use serde::{Deserialize, Serialize};
use std::sync::Weak;
use std::time::{Duration, Instant};

// It's important the peek doesn't expect more than the smallest possible request.
// The smallest possible HTTP request would be about 18 bytes
// "GET / HTTP/1.0\r\n\r\n"
// we use "lolb<8 bytes>" to indicate a service connection (12 bytes).
pub(crate) const PREAUTH_LEN: usize = 12;
pub(crate) const PREAUTH_PREFIX: &[u8] = b"lolb";

/// Holder of all defined services.
#[derive(Debug, Default)]
pub(crate) struct Services {
    domains: Vec<ServiceDomain>,
}

/// Domain to be serviced by a load balancer.
#[derive(Debug)]
pub(crate) struct ServiceDomain {
    /// The dns name of the domain serviced. Something like `example.com`.
    domain: String,
    /// Auth to use when adding service connections to this domain.
    auth: ServiceAuth,
    /// The current set of of hosts serviced under the domain. A host
    /// would be something like `myservice.example.com`.
    hosts: Vec<ServiceHost>,
}

/// Kinds of authentications for authenticating connections added to the service domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceAuth {
    /// Some secret string shared between the load balancer and the service.
    PreSharedSecret(String),
}

/// Service host gather a bunch of routes for that host. It is possible to route
/// `https://myservice.example.com/something/*` to one set of hosts, and
/// `https://myservice.example.com/other/*` to another.
#[derive(Debug)]
pub(crate) struct ServiceHost {
    /// The service host name. Something like `myservice.example.com`.
    host: String,
    /// TLS certificate needed to service this domain. If known.
    cert: Option<Certificate>,
    /// Current routes for the host. When selecting route the longest route prefix
    /// wins.
    routes: Vec<ServiceRoute>,
}

/// A route under a service host.
#[derive(Debug)]
pub(crate) struct ServiceRoute {
    /// Route under service. I.e. `/something` or `/other`. Empty string is not
    /// a valid route, instead we use `/`.
    prefix: String,
    /// Current connections servicing this route. The strong reference is held by the
    /// closure driving the connection.
    connections: Vec<Weak<ServiceConnection>>,
}

/// Holder of the actual connection to the service.
#[derive(Debug, Clone)]
pub struct ServiceConnection(pub h2::client::SendRequest<bytes::Bytes>);

impl ServiceConnection {
    /// Send request + request body to service.
    pub(crate) async fn send_request<'a, S>(
        self,
        req: http::Request<RecvBody<'a, S>>,
    ) -> LolbResult<http::Response<h2::RecvStream>>
    where
        S: Socket,
    {
        // wait for h2 conn to be ready to receive req
        let mut h2 = self.0.ready().await?;

        // reconstitute req to Request<()>
        let (parts, mut body) = req.into_parts();
        let req = http::Request::from_parts(parts, ());

        // send request + headers
        let (response, mut send_body) = h2.send_request(req, false).unwrap();

        // send body
        loop {
            // read next body chunk from incoming
            if let Some(read_body_res) = body.data().await {
                let mut body_data = read_body_res?;
                // try pushing data up to service as capacity becomes available.
                while !body_data.is_empty() {
                    // reserving capacity to send
                    send_body.reserve_capacity(body_data.len());
                    // wait for capacity to be available
                    let actual_capacity = PollCapacity(&mut send_body).await?;
                    // then send it over to service
                    let send_len = actual_capacity.min(body_data.len());
                    let to_send = body_data.slice_to(send_len);
                    send_body.send_data(to_send, false)?;
                    // once sent, release the corresponding amount from incoming
                    body.release_capacity(send_len)?;
                    // move pointer in what is yet to send in current chunk.
                    body_data = body_data.slice_from(send_len);
                }
            } else {
                // no more body data
                let empty = bytes::Bytes::new();
                send_body.send_data(empty, true)?; // true here is end-of-stream
                break;
            }
        }

        Ok(response.await?)
    }
}

/// A preauthed record associated with a unique secret valid for a limited time.
pub(crate) struct PreAuthed {
    created: Instant,
    domain: String,
    host: String,
    prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PreAuthedKey(pub u64);

impl Services {
    pub fn new() -> Self {
        Services {
            ..Default::default()
        }
    }
    pub fn add_preauthed(&mut self, p: PreAuthed, c: Weak<ServiceConnection>) {
        let service = self
            .domains
            .iter_mut()
            .find(|s| p.is_same_domain(s))
            // we shouldn't be able to have created a preauthed instance for
            // a domain that doesn't exist, so this is a fault.
            .expect("Preauthed for not configured domain");
        service.add_preauthed(p, c);
    }

    /// Route the request to a service.
    pub fn route<X>(&mut self, req: &http::Request<X>) -> Option<ServiceConnection> {
        let uri = req.uri();
        let host = uri.authority_part()?.host();
        let path = uri.path_and_query().map(|p| p.path()).unwrap_or("/");

        // find something that matches domain ending i.e: `a.b.c.com` might match
        // `b.c.com` and `c.com`. Longest wins.
        let mut domains: Vec<&mut ServiceDomain> = self
            .domains
            .iter_mut()
            .filter(|s| s.domain.ends_with(host))
            .collect();
        domains.as_mut_slice().sort_by_key(|d| d.domain.len());

        // the "best" domain is last.
        let domain = domains.last_mut()?;

        // the host must be an exact match.
        let host = domain.hosts.iter_mut().find(|h| h.host == host)?;

        // find all routes that has a prefix that matches the incoming request path.
        let mut routes: Vec<&mut ServiceRoute> = host
            .routes
            .iter_mut()
            .filter(|r| path.starts_with(&r.prefix))
            .collect();
        routes.as_mut_slice().sort_by_key(|r| r.prefix.len());

        // the "best" is the last.
        let route = routes.last_mut()?;

        // find a connection that is alive.
        // TODO sticky logic
        let conn = loop {
            // prune dead connections.
            route.connections.retain(|c| c.upgrade().is_some());
            if route.connections.is_empty() {
                break None;
            }
            // find first connection that is not dead
            if let Some(s) = route
                .connections
                .iter()
                .find(|c| c.upgrade().is_some())
                .and_then(|c| c.upgrade())
            {
                // ServiceConnection contains a h2 SendRequest, that we must clone to
                // get "our own" instance to send requests to.
                //
                // At this point we hold a _strong_ reference
                // to Arc<ServiceConnection> and it will not be gone by connection disconnecting.
                // Whether it will work to send requests to is a whole other matter.
                break Some(s.clone_contained());
            }
        }?;

        Some(conn)
    }
}

impl ServiceDomain {
    /// add/create a routing entry for a preauthed service connection.
    pub fn add_preauthed(&mut self, p: PreAuthed, c: Weak<ServiceConnection>) {
        let mut idx = self.hosts.iter().position(|h| p.is_same_host(h));
        if idx.is_none() {
            idx = Some(self.hosts.len());
            self.hosts.push(ServiceHost::new(&p.host));
        }
        let host = self.hosts.get_mut(idx.unwrap()).unwrap();
        host.add_preauthed(p, c);
    }
}

impl ServiceHost {
    fn new(host: &str) -> Self {
        ServiceHost {
            host: host.to_string(),
            cert: None,
            routes: vec![],
        }
    }
    /// add/create a routing entry for a preauthed service connection.
    pub fn add_preauthed(&mut self, p: PreAuthed, c: Weak<ServiceConnection>) {
        let mut idx = self.routes.iter().position(|r| p.is_same_prefix(r));
        if idx.is_none() {
            idx = Some(self.routes.len());
            self.routes.push(ServiceRoute::new(&p.prefix));
        }
        let route = self.routes.get_mut(idx.unwrap()).unwrap();
        route.add_connection(c);
    }
}

impl ServiceRoute {
    pub fn new(prefix: &str) -> Self {
        ServiceRoute {
            prefix: prefix.to_string(),
            connections: vec![],
        }
    }
    pub fn add_connection(&mut self, c: Weak<ServiceConnection>) {
        self.connections.push(c);
    }
}

impl PreAuthed {
    fn is_valid(&self) -> bool {
        let age = Instant::now() - self.created;
        age < Duration::from_secs(10)
    }
    fn domain(&self) -> &str {
        &self.domain
    }
    fn is_same_domain(&self, s: &ServiceDomain) -> bool {
        self.domain == s.domain
    }
    fn is_same_host(&self, s: &ServiceHost) -> bool {
        self.host == s.host
    }
    fn is_same_prefix(&self, s: &ServiceRoute) -> bool {
        self.prefix == s.prefix
    }
}
