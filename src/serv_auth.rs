use crate::service::{ServiceDomain, ServiceHost, ServiceRoute};
use crate::util::current_time_millis;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Secret key for a preauthed record. This is sent as a preamble when a service connects
/// for the second time after auth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PreAuthedKey(pub u64);

/// A preauthed record with information service provided as part of the auth.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PreAuthed {
    created: u64, // unix time millis
    domain: String,
    host: String,
    prefix: String,
}

impl PreAuthed {
    pub(crate) fn created(&self) -> u64 {
        self.created
    }
    pub(crate) fn is_valid(&self) -> bool {
        let age = Duration::from_millis(current_time_millis() - self.created);
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
