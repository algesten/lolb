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

/// Domain to be serviced by a load balancer.
#[derive(Debug)]
pub(crate) struct ServiceDomain {
    /// The dns name of the domain serviced. Something like `example.com`.
    domain: webpki::DNSName,
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
    host: webpki::DNSName,
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

/// Hold of the actual connection to the service.
#[derive(Debug)]
pub struct ServiceConnection {
    send: h2::client::SendRequest<bytes::Bytes>,
}

/// A preauthed record associated with a unique secret valid for a limited time.
pub(crate) struct PreAuthed {
    created: Instant,
    domain: webpki::DNSName,
    host: webpki::DNSName,
    prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PreAuthedKey(pub u64);

impl PreAuthed {
    fn is_valid(&self) -> bool {
        let age = Instant::now() - self.created;
        age < Duration::from_secs(10)
    }
}
