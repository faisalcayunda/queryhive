//! Where a saved password lives.
//!
//! One generic-password Keychain item per connection, under the service name the app
//! has used since long before this engine existed, with the connection's UUID as the
//! account:
//!
//! ```text
//! kSecClass       = kSecClassGenericPassword
//! kSecAttrService = id.data-ecosystem.queryhive
//! kSecAttrAccount = <the connection's UUID>
//! kSecValueData   = <the password, UTF-8>
//! ```
//!
//! That is the whole schema, and it is not ours to improve: the passwords already under
//! it are the ones a user expects to keep working, and a service name that differs by a
//! character does not read as "we changed the schema" to them — it reads as "my password
//! is gone".
//!
//! # Two attributes we deliberately do *not* set
//!
//! The app is ad-hoc signed with no entitlements, so it avoids
//! `kSecUseDataProtectionKeychain` and access groups, which both need a real signing
//! team; and it sets no `kSecAttrAccessible`, leaving the login keychain's default in
//! place. [`KeychainStore`] writes the same three attributes and nothing else, so an
//! item written by either program is readable by the other. Adding one here would not
//! harden anything a user could observe — it would fork one item shared by two
//! programs, and the symptom would be a password that saves and then does not load.
//!
//! # Why the account string is normalised
//!
//! Foundation's `UUID.uuidString` is upper case and Rust's `Uuid::to_string()` is lower
//! case, so the same connection spelled by the two sides looks like two accounts to
//! Keychain. [`account_key`] folds a UUID-shaped account to upper case before it is
//! used, which makes either spelling find the same item. A name that is not UUID-shaped
//! is passed through untouched: case can be part of a name, and only the UUID form has
//! a documented canonical spelling to agree on.
//!
//! # Testability
//!
//! Everything above the Keychain goes through [`SecretStore`], and [`MemoryStore`] is
//! the same contract without a platform underneath it. The Keychain itself is exercised
//! by an opt-in test (`QH_TEST_KEYCHAIN=1`), because writing to a user's login keychain
//! during an ordinary `cargo test` is not a thing a test suite should do unasked.

mod keychain;
mod memory;

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

pub use keychain::KeychainStore;
pub use memory::MemoryStore;

/// The Keychain service every saved connection password lives under.
///
/// Written out as a constant rather than assembled from a bundle identifier, because
/// the value is a compatibility contract with items that already exist. Renaming it
/// would lose them quietly.
pub const SERVICE: &str = "id.data-ecosystem.queryhive";

/// Why a secret could not be read or written.
#[derive(Debug, Error)]
pub enum CredentialError {
    /// The Keychain refused the operation, with its own words.
    ///
    /// The status code travels beside the message because it is what a support request
    /// needs: `errSecAuthFailed` and `errSecInteractionNotAllowed` are both "the
    /// Keychain said no", and they call for completely different answers.
    #[error("the Keychain refused to {operation} the credential: {message} (status {status})")]
    Keychain {
        operation: &'static str,
        status: i32,
        message: String,
    },

    /// The item exists but is not text, so it is not a password this engine wrote and
    /// not one it will guess at.
    #[error("the stored credential for {account} is not valid UTF-8")]
    NotUtf8 { account: String },

    /// No Keychain on this platform. The engine is macOS-only, so this is reachable
    /// only from a build that was never meant to store anything.
    #[error("the Keychain is not available on this platform")]
    Unsupported,
}

/// Where secrets are kept.
///
/// A trait rather than a concrete type so that everything which needs a password can be
/// tested without a Keychain, and so the storage layer can be swapped for a
/// self-contained one without touching a driver.
pub trait SecretStore: Send + Sync {
    /// Write the secret, replacing whatever was there.
    ///
    /// Replacing rather than refusing: a user editing a connection expects the new
    /// password to take effect, not to be told an old one exists.
    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError>;

    /// Read the secret, or `None` when nothing is stored.
    ///
    /// "Nothing stored" is not an error, because it is the ordinary state of a
    /// connection that has no password — a password-less database is a real thing, and
    /// an error here would make the UI show a failure for a working connection.
    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError>;

    /// Remove the secret.
    ///
    /// Removing something that is not there is success: the caller asked for it gone,
    /// and it is gone.
    fn delete(&self, account: &str) -> Result<(), CredentialError>;
}

/// The account string one connection's secret lives under.
///
/// See the module note: a UUID is folded to upper case so that this engine and the app
/// agree on one item, and anything else is left exactly as the caller wrote it.
pub fn account_key(account: &str) -> String {
    let account = account.trim();
    if is_uuid_shaped(account) {
        account.to_ascii_uppercase()
    } else {
        account.to_owned()
    }
}

/// Whether this is the 8-4-4-4-12 hex shape a UUID is spelled in.
fn is_uuid_shaped(text: &str) -> bool {
    let mut groups = 0;
    for (index, group) in text.split('-').enumerate() {
        let expected = match index {
            0 => 8,
            1..=3 => 4,
            4 => 12,
            _ => return false,
        };
        if group.len() != expected || !group.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        groups += 1;
    }
    groups == 5
}

/// Turn bytes read back from the Keychain into a secret.
pub(crate) fn secret_from_bytes(
    account: &str,
    bytes: Vec<u8>,
) -> Result<SecretString, CredentialError> {
    match String::from_utf8(bytes) {
        Ok(text) => Ok(SecretString::new(text.into_boxed_str())),
        Err(_) => Err(CredentialError::NotUtf8 {
            account: account.to_owned(),
        }),
    }
}

/// The bytes a secret is stored as: its own text, which is what the app writes.
pub(crate) fn secret_bytes(secret: &SecretString) -> Vec<u8> {
    secret.expose_secret().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_name_is_the_one_the_app_has_always_used() {
        // Pinned because it is the key to items that already exist: a "tidy-up" here
        // would lose every saved password, and the failure would surface as an empty
        // password field rather than as an error anyone could trace back to this line.
        assert_eq!(SERVICE, "id.data-ecosystem.queryhive");
    }

    #[test]
    fn a_uuid_account_is_folded_to_one_spelling() {
        let from_swift = "E621E1F8-C36C-495A-93FC-0C247A3E6E5F";
        let from_rust = "e621e1f8-c36c-495a-93fc-0c247a3e6e5f";
        assert_eq!(account_key(from_swift), from_swift);
        assert_eq!(account_key(from_rust), from_swift);
        assert_eq!(account_key(from_swift), account_key(from_rust));
        // A stray space from a text field is not part of the identity either.
        assert_eq!(account_key(&format!(" {from_rust} ")), from_swift);
    }

    #[test]
    fn a_name_that_is_not_a_uuid_keeps_its_case() {
        // Case can be part of a name, and only the UUID form has a canonical spelling
        // for both programs to agree on.
        assert_eq!(account_key("Production-Replica"), "Production-Replica");
        assert_eq!(account_key(""), "");
        // Shapes that are close but not a UUID are names, not identifiers.
        assert_eq!(
            account_key("e621e1f8-c36c-495a-93fc-0c247a3e6e5"),
            "e621e1f8-c36c-495a-93fc-0c247a3e6e5"
        );
        assert_eq!(
            account_key("e621e1f8c36c495a93fc0c247a3e6e5f"),
            "e621e1f8c36c495a93fc0c247a3e6e5f"
        );
        assert_eq!(
            account_key("zzzzzzzz-c36c-495a-93fc-0c247a3e6e5f"),
            "zzzzzzzz-c36c-495a-93fc-0c247a3e6e5f"
        );
    }

    #[test]
    fn a_secret_does_not_print_itself() {
        // The reason the type exists: a password that reaches a log line is a password
        // that has left the machine.
        let secret = SecretString::new("hunter2".to_owned().into_boxed_str());
        assert_eq!(format!("{secret:?}"), "SecretBox<str>([REDACTED])");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }
}
