//! The same contract without a platform underneath it.
//!
//! Two callers: tests that need a `SecretStore` and must not touch a user's login
//! keychain, and any future caller that wants the connection flow to work without
//! persistence. The rules are the Keychain's rules, so a test that passes here is
//! testing the contract and not a shortcut — `get` after `delete` answers `None`
//! because nothing is stored, not because a Keychain error was swallowed.

use std::collections::BTreeMap;
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};

use crate::{account_key, CredentialError, SecretStore};

/// Secrets in this process's memory, and nowhere else.
///
/// The secrets are `SecretString`s here too, not `String`s: a store that quietly copied
/// passwords into an ordinary string would be the one place the zeroize discipline
/// lapses, and it is the store most likely to be used in a test that later prints its
/// state.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<BTreeMap<String, SecretString>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many secrets are held. For tests and for a diagnostic that must not read one.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The accounts held, sorted. Never the secrets themselves.
    pub fn accounts(&self) -> Vec<String> {
        self.entries
            .lock()
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default()
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError> {
        // Cloned rather than moved because the caller keeps ownership of theirs; the
        // clone is a `SecretString` too, so it is zeroed when this store is dropped.
        let secret = SecretString::new(secret.expose_secret().to_owned().into_boxed_str());
        let mut entries = self.lock()?;
        entries.insert(account_key(account), secret);
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError> {
        let entries = self.lock()?;
        Ok(entries
            .get(&account_key(account))
            .map(|secret| SecretString::new(secret.expose_secret().to_owned().into_boxed_str())))
    }

    fn delete(&self, account: &str) -> Result<(), CredentialError> {
        let mut entries = self.lock()?;
        entries.remove(&account_key(account));
        Ok(())
    }
}

impl MemoryStore {
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, SecretString>>, CredentialError> {
        // A poisoned lock means another thread panicked while holding the map, so the
        // map may be half-written; reading it could answer with a stale secret. Failing
        // closed is the only answer that does not leak.
        self.entries.lock().map_err(|_| CredentialError::Keychain {
            operation: "read from memory",
            status: 0,
            message: "the store was poisoned by a panic in another thread".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_key;

    fn secret(text: &str) -> SecretString {
        SecretString::new(text.to_owned().into_boxed_str())
    }

    fn reveal(secret: &SecretString) -> String {
        secret.expose_secret().to_owned()
    }

    #[test]
    fn a_secret_survives_a_write_and_a_read() {
        let store = MemoryStore::new();
        assert!(store.get("conn").unwrap().is_none(), "nothing stored yet");
        store.set("conn", &secret("hunter2")).unwrap();
        assert_eq!(reveal(&store.get("conn").unwrap().unwrap()), "hunter2");
    }

    #[test]
    fn writing_again_replaces_rather_than_refusing() {
        let store = MemoryStore::new();
        store.set("conn", &secret("old")).unwrap();
        store.set("conn", &secret("new")).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(reveal(&store.get("conn").unwrap().unwrap()), "new");
    }

    #[test]
    fn deleting_what_is_not_there_is_success() {
        let store = MemoryStore::new();
        store.delete("never-written").unwrap();
        store.set("conn", &secret("hunter2")).unwrap();
        store.delete("conn").unwrap();
        assert!(store.get("conn").unwrap().is_none());
        store.delete("conn").unwrap();
    }

    #[test]
    fn the_account_is_the_same_key_whichever_side_spells_it() {
        let store = MemoryStore::new();
        store
            .set(
                "E621E1F8-C36C-495A-93FC-0C247A3E6E5F",
                &secret("from the app"),
            )
            .unwrap();
        assert_eq!(
            reveal(
                &store
                    .get("e621e1f8-c36c-495a-93fc-0c247a3e6e5f")
                    .unwrap()
                    .unwrap()
            ),
            "from the app"
        );
        assert_eq!(
            store.accounts(),
            vec![account_key("e621e1f8-c36c-495a-93fc-0c247a3e6e5f")]
        );
    }

    #[test]
    fn a_debug_rendering_of_the_store_does_not_hold_a_password() {
        let store = MemoryStore::new();
        store.set("conn", &secret("hunter2")).unwrap();
        let rendered = format!("{store:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("REDACTED"), "{rendered}");
    }
}
