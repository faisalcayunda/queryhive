//! One server key, and the two names it has: the wire blob and the fingerprint.
//!
//! The blob is the authority everywhere in this crate. A `known_hosts` line carries a
//! key-type token next to the base64 body, but the token is not what OpenSSH compares —
//! it compares the decoded key. Taking the type out of the blob as well (`key_type`)
//! means a line whose token disagrees with its body cannot make us compare the wrong
//! pair of bytes, and it is what lets [`crate::known_hosts::append`] write a line from
//! a blob alone.

use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::Error;

/// A public key as the server presented it in the key exchange.
///
/// Constructed only from bytes that parse as an SSH public key blob, so a caller cannot
/// build one that would be written to `known_hosts` as a line `ssh` cannot read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKey {
    key_type: String,
    blob: Vec<u8>,
}

impl ServerKey {
    /// Wrap the raw SSH public key blob (the decoded base64 body of a `.pub` file).
    ///
    /// # Errors
    /// [`Error::MalformedKeyBlob`] if the blob is not `length-prefixed-type || key data`.
    pub fn from_blob(blob: Vec<u8>) -> Result<Self, Error> {
        let key_type = key_type_of(&blob).ok_or(Error::MalformedKeyBlob)?;
        Ok(Self { key_type, blob })
    }

    /// The algorithm name inside the blob, e.g. `ssh-ed25519`.
    #[must_use]
    pub fn key_type(&self) -> &str {
        &self.key_type
    }

    /// The whole blob, decoded.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// `SHA256:…`, the form `ssh-keygen -lf` prints.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.blob)
    }

    /// The line this key becomes in a `known_hosts` file.
    ///
    /// `host` for port 22, `[host]:port` otherwise — the same spelling OpenSSH uses,
    /// which is also the string a hashed entry is an HMAC of.
    #[must_use]
    pub fn known_hosts_line(&self, host: &str, port: u16) -> String {
        format!(
            "{} {} {}",
            crate::known_hosts::host_spelling(host, port),
            self.key_type,
            base64::engine::general_purpose::STANDARD.encode(&self.blob)
        )
    }
}

/// `SHA256:` followed by the base64 of SHA-256 over the blob, unpadded.
///
/// Unpadded because that is what `ssh-keygen -lf` prints, and this string is compared
/// against it by a human reading a prompt.
#[must_use]
pub fn fingerprint(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// A key is shown by its fingerprint: it is the string a person has been told to
/// compare, and printing the blob instead would be noise.
impl std::fmt::Display for ServerKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.fingerprint())
    }
}

/// Read the algorithm name out of a public key blob.
///
/// The blob starts with a 4-byte big-endian length and then that many bytes of name:
/// `ssh-ed25519`, `ecdsa-sha2-nistp256`, `ssh-rsa`, … A blob that does not have that
/// shape is not something we will write into a known_hosts line, because `ssh` would
/// then refuse the whole file.
pub(crate) fn key_type_of(blob: &[u8]) -> Option<String> {
    let (length, rest) = blob.split_at_checked(4)?;
    let length = usize::try_from(u32::from_be_bytes(length.try_into().ok()?)).ok()?;
    let (name, key_data) = rest.split_at_checked(length)?;
    let name = std::str::from_utf8(name).ok()?;
    // An empty name or a non-printable one would produce a known_hosts line whose
    // fields do not exist; the key data has to be there too, or there is no key.
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_graphic()) || key_data.is_empty() {
        return None;
    }
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same blob `russh`'s own fingerprint test uses, whose expected value was
    // produced by `ssh-keygen -lf`.
    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILagOJFgwaMNhBWQINinKOXmqS4Gh5NgxgriXwdOoINJ";

    #[test]
    fn fingerprint_matches_ssh_keygen() {
        let blob = base64::engine::general_purpose::STANDARD
            .decode(ED25519)
            .expect("test vector is base64");
        assert_eq!(
            fingerprint(&blob),
            "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog"
        );
    }

    #[test]
    fn key_type_comes_out_of_the_blob_not_the_caller() {
        let key = ServerKey::from_blob(
            base64::engine::general_purpose::STANDARD
                .decode(ED25519)
                .expect("test vector is base64"),
        )
        .expect("valid blob");
        assert_eq!(key.key_type(), "ssh-ed25519");
        assert_eq!(
            key.known_hosts_line("db.internal", 22),
            format!("db.internal ssh-ed25519 {ED25519}")
        );
        assert_eq!(
            key.known_hosts_line("db.internal", 2222),
            format!("[db.internal]:2222 ssh-ed25519 {ED25519}")
        );
    }

    #[test]
    fn a_blob_without_a_type_is_refused() {
        assert!(ServerKey::from_blob(vec![]).is_err());
        assert!(ServerKey::from_blob(vec![0, 0, 0, 3, b'a']).is_err());
        assert!(ServerKey::from_blob(vec![0xff, 0xff, 0xff, 0xff, b'a']).is_err());
        // A name with no key material after it is not a key.
        assert!(ServerKey::from_blob(vec![0, 0, 0, 2, b'h', b'i']).is_err());
    }
}
