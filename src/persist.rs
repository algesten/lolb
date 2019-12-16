use crate::serv_auth::{Preauthed, ReconnectKey};
use crate::LolbResult;
pub use acme_lib::persist::{Persist as AcmePersist, PersistKey as AcmePersistKey};
pub use acme_lib::{Error as AcmeError, Result as AcmeResult};
use std::sync::mpsc::{channel, Sender};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PersistKey<'a> {
    Acme(&'a AcmePersistKey<'a>),
    ReconnectKey(u64),
}

/// Trait for persistence implementations.
pub trait Persist: AcmePersist + Clone + Send {
    /// Bridge ACME put into our own "save" with callback. This stalls the acme thread worker
    /// thread which is ok cause it's expected by that lib.
    fn put(&self, key: &AcmePersistKey, value: &[u8]) -> AcmeResult<()> {
        let (tx, rx) = channel::<LolbResult<()>>();
        let pk = PersistKey::Acme(key);
        self.save(&pk, value, tx);
        rx.recv()
            .expect("Failed to rx.recv()")
            .map_err(|e| AcmeError::Other(format!("Failed to persist acme key {}: {}", key, e)))
    }

    /// Bridge ACME get into our own "load" with callback. This stalls the acme thread worker
    /// thread which is ok cause it's expected by that lib.
    fn get(&self, key: &AcmePersistKey) -> AcmeResult<Option<Vec<u8>>> {
        let (tx, rx) = channel::<LolbResult<Option<Vec<u8>>>>();
        let pk = PersistKey::Acme(key);
        self.load(&pk, tx);
        rx.recv()
            .expect("Failed to rx.recv()")
            .map_err(|e| AcmeError::Other(format!("Failed to load acme key {}: {}", key, e)))
    }

    /// Async takes callback until traits can have async functions.
    fn save(&self, key: &PersistKey, value: &[u8], tx: Sender<LolbResult<()>>);
    /// Async takes callback until traits can have async functions.
    fn load(&self, key: &PersistKey, tx: Sender<LolbResult<Option<Vec<u8>>>>);
}

/// Save a preauthed reconnect key to persistence.
pub(crate) async fn save_preauthed<P: Persist>(
    persist: &P,
    key: ReconnectKey,
    authed: &Preauthed,
) -> LolbResult<()> {
    let pk = PersistKey::ReconnectKey(key.0);
    let json = serde_json::to_string(authed).expect("Failed to json serialize Preauthed");
    let (tx, rx) = channel::<LolbResult<()>>();
    persist.save(&pk, json.as_bytes(), tx);
    rx.recv().expect("Failed to rx.recv()")
}

/// Read a preauthed reconnect key from persistence.
pub(crate) async fn load_preauthed<P: Persist>(
    persist: &P,
    key: ReconnectKey,
) -> LolbResult<Option<Preauthed>> {
    let (tx, rx) = channel::<LolbResult<Option<Vec<u8>>>>();
    let pk = PersistKey::ReconnectKey(key.0);
    persist.load(&pk, tx);
    rx.recv().expect("Failed to rx.recv()").map(|o| {
        o.map(|b| serde_json::from_slice(&b[..]).expect("Failed to json deserialize Preauthed"))
    })
}
