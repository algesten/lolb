use crate::serv_auth::{PreAuthed, PreAuthedKey};
use crate::LolbResult;
pub use acme_lib::persist::{Persist as AcmePersist, PersistKey as AcmePersistKey};
pub use acme_lib::{Error as AcmeError, Result as AcmeResult};
use std::sync::mpsc::channel;

pub enum PersistKey<'a> {
    Acme(&'a AcmePersistKey<'a>),
    PreAuthedKey(u64),
}

pub trait Persist: AcmePersist + Clone + Send {
    /// Bridge ACME put into our own "save" with callback. This stalls the acme thread worker
    /// thread which is ok cause it's expected by that lib.
    fn put(&self, key: &AcmePersistKey, value: &[u8]) -> AcmeResult<()> {
        let (tx, rx) = channel::<LolbResult<()>>();
        let pk = PersistKey::Acme(key);
        self.save(&pk, value, |r| {
            tx.send(r).expect("Failed to tx.send()");
        });
        rx.recv()
            .expect("Failed to rx.recv()")
            .map_err(|e| AcmeError::Other(format!("Failed to persist acme key {}: {}", key, e)))
    }

    /// Bridge ACME get into our own "load" with callback. This stalls the acme thread worker
    /// thread which is ok cause it's expected by that lib.
    fn get(&self, key: &AcmePersistKey) -> AcmeResult<Option<Vec<u8>>> {
        let (tx, rx) = channel::<LolbResult<Option<Vec<u8>>>>();
        let pk = PersistKey::Acme(key);
        self.load(&pk, |r| {
            tx.send(r).expect("Failed to tx.send()");
        });
        rx.recv()
            .expect("Failed to rx.recv()")
            .map_err(|e| AcmeError::Other(format!("Failed to load acme key {}: {}", key, e)))
    }

    /// Async takes callback until traits can have async functions.
    fn save<C: FnOnce(LolbResult<()>)>(&self, key: &PersistKey, value: &[u8], callback: C);
    /// Async takes callback until traits can have async functions.
    fn load<C: FnOnce(LolbResult<Option<Vec<u8>>>)>(&self, key: &PersistKey, callback: C);
}

pub(crate) async fn save_preauthed<P: Persist>(
    persist: &P,
    key: &PreAuthedKey,
    authed: &PreAuthed,
) -> LolbResult<()> {
    let pk = PersistKey::PreAuthedKey(key.0);
    let json = serde_json::to_string(authed).expect("Failed to json serialize PreAuthed");

    Ok(())
}

pub(crate) async fn load_preauthed<P: Persist>(
    persist: &P,
    key: &PreAuthedKey,
) -> LolbResult<Option<PreAuthed>> {
    let pk = PersistKey::PreAuthedKey(key.0);
    Ok(None)
}
