//! Where Apple state lives: the macOS Keychain, service `com.allternit.sideload`.
//!
//! isideload stores through its `SideloadingStorage` trait: the anisette
//! machine state (`anisette_state`), the signing private key per Apple ID
//! (`<sha256(apple id)>/key`) and, from this crate, the saved session
//! (`session/<sha256(apple id)>`). Each is one generic-password item.
//!
//! Off macOS (the crate only compiles there for workspace builds; Desktop ships
//! it for macOS) state is kept in memory for the life of the process.

use isideload::util::storage::{InMemoryStorage, SideloadingStorage};
use rootcause::prelude::*;

use crate::session::{session_key, StoredSession};

pub const KEYCHAIN_SERVICE: &str = "com.allternit.sideload";

#[cfg(target_os = "macos")]
pub struct KeychainStorage {
    service: String,
}

#[cfg(target_os = "macos")]
impl KeychainStorage {
    pub fn new(service: &str) -> Self {
        KeychainStorage { service: service.to_string() }
    }
}

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SideloadingStorage for KeychainStorage {
    fn store(&self, key: &str, value: &str) -> Result<(), Report> {
        if value.is_empty() {
            return self.delete(key);
        }
        security_framework::passwords::set_generic_password(&self.service, key, value.as_bytes())
            .map_err(|e| report!("Keychain write failed ({})", e.code()))
    }

    fn retrieve(&self, key: &str) -> Result<Option<String>, Report> {
        match security_framework::passwords::get_generic_password(&self.service, key) {
            Ok(bytes) => {
                let value = String::from_utf8(bytes).map_err(|_| report!("Keychain item is not UTF-8"))?;
                Ok((!value.is_empty()).then_some(value))
            }
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(report!("Keychain read failed ({})", e.code())),
        }
    }

    fn delete(&self, key: &str) -> Result<(), Report> {
        match security_framework::passwords::delete_generic_password(&self.service, key) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(report!("Keychain delete failed ({})", e.code())),
        }
    }
}

/// The platform store. Stateless on macOS (each call hits the Keychain), so
/// handing out several instances is fine.
pub fn platform_storage() -> Box<dyn SideloadingStorage> {
    #[cfg(target_os = "macos")]
    {
        Box::new(KeychainStorage::new(KEYCHAIN_SERVICE))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(InMemoryStorage::new())
    }
}

/// A store for tests and for platforms without a keychain.
#[cfg_attr(not(test), allow(dead_code))]
pub fn memory_storage() -> Box<dyn SideloadingStorage> {
    Box::new(InMemoryStorage::new())
}

pub fn load_session(storage: &dyn SideloadingStorage, apple_id: &str) -> Option<StoredSession> {
    let text = zeroize::Zeroizing::new(storage.retrieve(&session_key(apple_id)).ok().flatten()?);
    StoredSession::from_json(&text)
}

pub fn save_session(storage: &dyn SideloadingStorage, apple_id: &str, session: &StoredSession) -> Result<(), Report> {
    let json = zeroize::Zeroizing::new(session.to_json());
    storage.store(&session_key(apple_id), &json)
}

pub fn forget_session(storage: &dyn SideloadingStorage, apple_id: &str) {
    let _ = storage.delete(&session_key(apple_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SESSION_VERSION;

    fn session() -> StoredSession {
        StoredSession {
            version: SESSION_VERSION,
            adsid: "adsid".into(),
            xcode_token: "tok".into(),
            token_expiry_ms: 1,
            spd: None,
            team_id: Some("ABCDE12345".into()),
        }
    }

    #[test]
    fn session_round_trips_per_apple_id() {
        let store = memory_storage();
        assert!(load_session(store.as_ref(), "a@example.com").is_none());
        save_session(store.as_ref(), "a@example.com", &session()).unwrap();
        assert_eq!(load_session(store.as_ref(), "A@example.com"), Some(session()));
        assert!(load_session(store.as_ref(), "b@example.com").is_none());
        forget_session(store.as_ref(), "a@example.com");
        assert!(load_session(store.as_ref(), "a@example.com").is_none());
    }

    #[test]
    fn corrupt_session_reads_as_none() {
        let store = memory_storage();
        store.store(&session_key("a@example.com"), "{not json").unwrap();
        assert!(load_session(store.as_ref(), "a@example.com").is_none());
    }

    #[test]
    fn stored_session_never_contains_a_password_field() {
        // The password is not part of what we persist.
        let json = session().to_json();
        assert!(!json.contains("password"), "{json}");
    }

    /// Touches the real login Keychain, so it only runs on request:
    /// `cargo test -p allternit-sideload -- --ignored keychain`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let store = KeychainStorage::new("com.allternit.sideload.test");
        store.store("probe", "value").unwrap();
        assert_eq!(store.retrieve("probe").unwrap().as_deref(), Some("value"));
        store.delete("probe").unwrap();
        assert_eq!(store.retrieve("probe").unwrap(), None);
        store.delete("probe").unwrap();
    }
}
