use crate::service::{ServiceDomain, ServiceHost, ServiceRoute};
use std::time::{Duration, Instant};

/// A preauthed record associated with a unique secret valid for a limited time.
pub(crate) struct PreAuthed {
    created: Instant,
    domain: String,
    host: String,
    prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PreAuthedKey(pub u64);

impl PreAuthed {
    pub(crate) fn is_valid(&self) -> bool {
        let age = Instant::now() - self.created;
        age < Duration::from_secs(10)
    }
    pub(crate) fn domain(&self) -> &str {
        &self.domain
    }
    pub(crate) fn host(&self) -> &str {
        &self.host
    }
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }
    pub(crate) fn is_same_domain(&self, s: &ServiceDomain) -> bool {
        self.domain == s.domain()
    }
    pub(crate) fn is_same_host(&self, s: &ServiceHost) -> bool {
        self.host == s.host()
    }
    pub(crate) fn is_same_prefix(&self, s: &ServiceRoute) -> bool {
        self.prefix == s.prefix()
    }
}
