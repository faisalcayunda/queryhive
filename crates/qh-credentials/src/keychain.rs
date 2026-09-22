//! The login Keychain, one generic-password item per connection.
//!
//! This is `ConnectionKeychain` from
//! `app/Sources/TrinoExporter/Models/Connections.swift:363`, reimplemented against the
//! same three attributes so that either program can read what the other wrote. The
//! Swift version reaches for `SecItemAdd`/`SecItemCopyMatching`/`SecItemDelete`
//! directly; the crate this engine links offers the same calls behind `passwords`, and
//! — checked in that crate's source rather than assumed — it queries
//! `kSecClassGenericPassword` with the service and the account and **nothing else**, on
//! the legacy login keychain. That is exactly the item, so reproducing it needs no
//! hand-rolled `unsafe` here.

use secrecy::SecretString;

use crate::{account_key, CredentialError, SecretStore};

/// The login Keychain.
///
/// A unit struct: the item is named by its attributes, so there is no handle to hold and
/// nothing to configure. Two of these are the same store.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
mod macos {
    use security_framework::base::Error;
    use security_framework_sys::base::errSecItemNotFound;

    use crate::CredentialError;

    /// The Keychain says "there is no such item", which is an answer rather than a
    /// failure.
    pub(super) fn is_missing(error: &Error) -> bool {
        error.code() == errSecItemNotFound
    }

    /// The Keychain's own words, with the status code beside them.
    ///
    /// The message is taken before the code because `code()` consumes the error, and
    /// because a bare `-25300` tells a user nothing while the message tells them which
    /// of "the item is missing" and "the login keychain is locked" happened.
    pub(super) fn refused(operation: &'static str, error: Error) -> CredentialError {
        let message = error.to_string();
        CredentialError::Keychain {
            operation,
            status: error.code(),
            message,
        }
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError> {
        let account = account_key(account);
        // One call that adds or updates: `SecItemAdd` answers `errSecDuplicateItem` for
        // an item that exists, and the crate retries with `SecItemUpdate`. The Swift
        // version wrote that retry by hand, so this is the same behaviour rather than a
        // coincidence of naming.
        security_framework::passwords::set_generic_password(
            crate::SERVICE,
            &account,
            &crate::secret_bytes(secret),
        )
        .map_err(|error| macos::refused("save", error))
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError> {
        let account = account_key(account);
        let options = security_framework::passwords::PasswordOptions::new_generic_password(
            crate::SERVICE,
            &account,
        );
        match security_framework::passwords::generic_password(options) {
            Ok(bytes) => crate::secret_from_bytes(&account, bytes).map(Some),
            Err(error) if macos::is_missing(&error) => Ok(None),
            Err(error) => Err(macos::refused("load", error)),
        }
    }

    fn delete(&self, account: &str) -> Result<(), CredentialError> {
        let account = account_key(account);
        match security_framework::passwords::delete_generic_password(crate::SERVICE, &account) {
            Ok(()) => Ok(()),
            // Deleting what is not there leaves the caller with what they asked for.
            Err(error) if macos::is_missing(&error) => Ok(()),
            Err(error) => Err(macos::refused("delete", error)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn set(&self, _account: &str, _secret: &SecretString) -> Result<(), CredentialError> {
        Err(CredentialError::Unsupported)
    }

    fn get(&self, _account: &str) -> Result<Option<SecretString>, CredentialError> {
        Err(CredentialError::Unsupported)
    }

    fn delete(&self, _account: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unsupported)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::{account_key, secret_bytes};

    #[test]
    fn the_options_are_the_item_the_app_writes() {
        // Not a Keychain call: this asserts the *shape* of the query this crate builds,
        // which is the part that has to match the app. Reaching into the private
        // dictionary is not possible through the crate's API, so what is checked here
        // is the half that is ours — that the account is normalised before it is used —
        // and the matching of the three attributes is a fact about the crate's source
        // recorded in the module note above.
        assert_eq!(
            account_key("e621e1f8-c36c-495a-93fc-0c247a3e6e5f"),
            "E621E1F8-C36C-495A-93FC-0C247A3E6E5F"
        );
        let secret = SecretString::new("hunter2".to_owned().into_boxed_str());
        assert_eq!(secret_bytes(&secret), b"hunter2");
        assert_eq!(crate::SERVICE, "id.data-ecosystem.queryhive");
    }
}
