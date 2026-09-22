//! Reading, matching and extending `~/.ssh/known_hosts`.
//!
//! This is the part of the tunnel that decides whether a person is asked a question, so
//! the rules are the ones `ssh` itself applies, not an approximation of them:
//!
//! - The host is matched under its `known_hosts` spelling — `host` on port 22,
//!   `[host]:port` otherwise — against a comma-separated list on one line, with `*` and
//!   `?` wildcards and `!` negation, case-insensitively.
//! - **Hashed entries are matched**, `|1|salt|hash` being HMAC-SHA1 of the hostname
//!   under that salt. `HashKnownHosts` is on by default in several distributions, and a
//!   checker that skipped hashed entries would report "unknown host" for hosts the user
//!   has already accepted — which is how you train someone to click through the one
//!   prompt that protects them.
//! - A key is compared as **bytes**, base64-decoded, not as text and not by fingerprint.
//! - `@cert-authority` is a CA key, not a host key, and is never matched as one. This
//!   build does not verify host certificates; [`crate::Error::HostCertificateUnsupported`]
//!   is raised when a server offers one, and
//!   [`HostKeyVerdict::Unknown::covered_by_certificate_authority`] tells the caller that
//!   the host *is* covered by a CA they trust, so the refusal can be explained instead
//!   of looking like an oversight.
//! - `@revoked` is a refusal nothing may override. Without it, a key recorded only in a
//!   `@revoked` line would look unknown, and TOFU would cheerfully re-accept it.
//! - A line that cannot be parsed is an error naming the line, not a line to skip:
//!   `ssh` refuses such a file, and skipping it would silently drop a check.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::key::{fingerprint, key_type_of, ServerKey};
use crate::Error;

/// What a `known_hosts` file says about the key a server presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
    /// The exact key is on record for this host.
    Matched,
    /// Nothing is on record for this host. TOFU applies: the caller shows
    /// `fingerprint` and decides whether to accept [`ServerKey`].
    Unknown {
        fingerprint: String,
        covered_by_certificate_authority: bool,
    },
    /// Keys *are* on record for this host and none of them is the presented one.
    Mismatch { recorded: Vec<RecordedKey> },
    /// A `@revoked` line matches this host and this exact key.
    Revoked { line: usize },
}

/// A key already on record, with the line it came from so a person can go and look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedKey {
    key_type: String,
    blob: Vec<u8>,
    line: usize,
}

impl RecordedKey {
    /// A key that is not in a file: a pinned key reported as a mismatch against what
    /// the caller approved. `line` is 0 because there is no line.
    pub(crate) fn pinned(key: &ServerKey) -> Self {
        Self {
            key_type: key.key_type().to_owned(),
            blob: key.blob().to_vec(),
            line: 0,
        }
    }

    /// The algorithm name inside the recorded blob, e.g. `ssh-ed25519`.
    #[must_use]
    pub fn key_type(&self) -> &str {
        &self.key_type
    }

    /// The recorded blob, decoded.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The 1-based line number in the file it was read from.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    /// `SHA256:…`, the form `ssh-keygen -lf` prints.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.blob)
    }
}

/// `~/.ssh/known_hosts`.
///
/// # Errors
/// [`Error::NoHomeDirectory`] when `$HOME` is unset.
pub fn default_path() -> Result<PathBuf, Error> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHomeDirectory)?;
    Ok(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

/// How `ssh` spells a host in a `known_hosts` file, and what a hashed entry is an HMAC
/// of.
#[must_use]
pub fn host_spelling(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Check `blob` against `path`.
///
/// A file that does not exist yields [`HostKeyVerdict::Unknown`]: nothing has been
/// recorded yet, which is the state TOFU exists for. A file that exists but cannot be
/// read is an error, because "cannot read" and "nothing recorded" are different facts
/// and only one of them is a reason to ask a person.
///
/// # Errors
/// [`Error::KnownHostsUnreadable`] or [`Error::MalformedKnownHosts`].
pub fn check(path: &Path, host: &str, port: u16, blob: &[u8]) -> Result<HostKeyVerdict, Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::KnownHostsUnreadable {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    check_text(&text, &path.display().to_string(), host, port, blob)
}

/// [`check`] over text already in hand, for callers that hold the file in memory and
/// for tests that should not need a temp directory.
///
/// `source` only names the text in error messages.
///
/// # Errors
/// [`Error::MalformedKnownHosts`].
pub fn check_text(
    text: &str,
    source: &str,
    host: &str,
    port: u16,
    blob: &[u8],
) -> Result<HostKeyVerdict, Error> {
    let spelling = host_spelling(host, port);
    let mut recorded: Vec<RecordedKey> = Vec::new();
    let mut matched = false;
    let mut covered_by_certificate_authority = false;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let entry = match Entry::parse(raw, source, line)? {
            Some(entry) => entry,
            // Blank lines and `#` comments.
            None => continue,
        };
        if !entry.matches(&spelling) {
            continue;
        }
        match entry.marker {
            // A CA key is not a host key. Recorded here so the "unknown host" prompt can
            // say the host is covered by a CA this build cannot use, then move on.
            Some(Marker::CertAuthority) => covered_by_certificate_authority = true,
            Some(Marker::Revoked) => {
                if entry.blob == blob {
                    return Ok(HostKeyVerdict::Revoked { line });
                }
                // A revoked entry for some other key says nothing about this one; ssh
                // reaches the same conclusion, and it is what lets a rotated key be
                // recorded next to the revocation of the old one.
            }
            None => {
                let key = RecordedKey {
                    key_type: entry.key_type,
                    blob: entry.blob,
                    line,
                };
                if key.blob == blob {
                    matched = true;
                } else {
                    recorded.push(key);
                }
            }
        }
    }

    if matched {
        // The whole match is this: the exact bytes, somewhere in the file. It is decided
        // only after every line has been read, because a `@revoked` line below it is a
        // refusal and line order must not decide whether a key is accepted.
        Ok(HostKeyVerdict::Matched)
    } else if !recorded.is_empty() {
        Ok(HostKeyVerdict::Mismatch { recorded })
    } else {
        Ok(HostKeyVerdict::Unknown {
            fingerprint: fingerprint(blob),
            covered_by_certificate_authority,
        })
    }
}

/// Append the line that records `blob` for `host:port`.
///
/// The file is created with mode 0600 if it is not there. It is not a secret — the keys
/// in it are public — but `ssh` writes it that way and a `known_hosts` anyone can
/// rewrite is an authentication that anyone can rewrite. A file that already exists is
/// appended to and left with the mode it has: which mode that is belongs to its owner.
/// Missing parent directories (usually `~/.ssh`) are created 0700 for the same reason.
///
/// # Errors
/// [`Error::MalformedKeyBlob`] if `blob` is not a public key, plus I/O errors.
pub fn append(path: &Path, host: &str, port: u16, blob: &[u8]) -> Result<(), Error> {
    let key = ServerKey::from_blob(blob.to_vec())?;
    let line = format!("{}\n", key.known_hosts_line(host, port));

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_0700(parent)?;
        }
    }

    let mut options = OpenOptions::new();
    options.append(true).read(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Ignored when the file already exists, which is the behaviour wanted here.
        options.mode(0o600);
    }
    let mut file = options.open(path)?;

    let length = file.metadata()?.len();
    if length > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        file.read_exact(&mut last)?;
        // Appending after a missing final newline would glue two keys into one line and
        // lose both.
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(())
}

/// `create_dir_all`, with `0700` on the directories it creates.
#[cfg(unix)]
fn create_dir_0700(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path).map_err(Error::from)
}

/// `create_dir_all`, on platforms whose file modes this crate does not set.
#[cfg(not(unix))]
fn create_dir_0700(path: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(path).map_err(Error::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    CertAuthority,
    Revoked,
}

/// One parsed line: a marker, the host patterns, and one key.
#[derive(Debug, Clone)]
struct Entry {
    marker: Option<Marker>,
    patterns: String,
    key_type: String,
    blob: Vec<u8>,
}

impl Entry {
    /// Parse one line, or `None` for a blank line or comment.
    fn parse(raw: &str, source: &str, line: usize) -> Result<Option<Self>, Error> {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            return Ok(None);
        }
        let malformed = || Error::MalformedKnownHosts {
            file: source.to_owned(),
            line,
        };
        let mut fields = raw.split_whitespace();
        let mut marker = None;
        let first = fields.next().ok_or_else(malformed)?;
        let patterns = match first {
            "@cert-authority" => {
                marker = Some(Marker::CertAuthority);
                fields.next().ok_or_else(malformed)?
            }
            "@revoked" => {
                marker = Some(Marker::Revoked);
                fields.next().ok_or_else(malformed)?
            }
            // An unknown marker is one whose meaning we do not know, and guessing it
            // would be worse than saying so.
            other if other.starts_with('@') => return Err(malformed()),
            other => other,
        };
        let token = fields.next().ok_or_else(malformed)?;
        let body = fields.next().ok_or_else(malformed)?;
        // Whatever follows the key is a comment, which is why it is not read.
        let blob = decode_base64(body).ok_or_else(malformed)?;

        // The blob has to be a key; its own algorithm name is the one used, so a line
        // whose token disagrees with its body cannot make two different keys compare
        // equal.
        let key_type = key_type_of(&blob).ok_or_else(malformed)?;
        // Keep the token only to check it names the same algorithm, which is what
        // `ssh`'s reader does before trusting the body.
        if key_type != token {
            return Err(malformed());
        }

        Ok(Some(Self {
            marker,
            patterns: patterns.to_owned(),
            key_type,
            blob,
        }))
    }

    /// Whether this line's host pattern list names `host` (already in its
    /// `known_hosts` spelling).
    fn matches(&self, host: &str) -> bool {
        patterns_match(&self.patterns, host)
    }
}

/// One comma-separated host pattern list, with `!` negation, `*`/`?` wildcards and the
/// `|1|salt|hash` hashed form.
fn patterns_match(patterns: &str, host: &str) -> bool {
    // ssh lowercases a hostname before matching and hashing it, so the same folded form
    // is used for both here. A hashed entry written from a mixed-case hostname would not
    // match in ssh either.
    let host = host.to_ascii_lowercase();
    let mut positive = false;
    for pattern in patterns.split(',') {
        if pattern.is_empty() {
            continue;
        }
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        let matched = match pattern.strip_prefix("|1|") {
            Some(hashed) => hashed_matches(hashed, &host),
            None => glob(&pattern.to_ascii_lowercase(), &host),
        };
        if matched {
            if negated {
                // A negated pattern that matches makes the whole line not match,
                // wherever the matching pattern appears in the list.
                return false;
            }
            positive = true;
        }
    }
    positive
}

/// HMAC-SHA1 of the hostname under the salt in a `|1|salt|hash` pattern.
fn hashed_matches(hashed: &str, host: &str) -> bool {
    // The pattern is `|1|salt|hash`, so what arrived here is `salt|hash`.
    let mut parts = hashed.split('|');
    let (Some(salt), Some(hash)) = (parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let (Some(salt), Some(hash)) = (decode_base64(salt), decode_base64(hash)) else {
        return false;
    };
    // The key length is not a choice here: OpenSSH hashes with the salt as the key and
    // SHA-1 as the hash. A salt of any length is accepted by `new_from_slice`.
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(&salt) else {
        return false;
    };
    mac.update(host.as_bytes());
    // Constant-time comparison, because this is a comparison of an expected value
    // against one derived from attacker-influenced input.
    mac.verify_slice(&hash).is_ok()
}

/// `*` and `?`, case already folded, matching OpenSSH's `match_pattern` (which supports
/// nothing else — no character classes).
///
/// Iterative rather than recursive: a pattern full of `*` against a long hostname is
/// exponential work for the naive version, and this input is a file on disk.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut resume = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(star) = star {
            p = star + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// Base64 as it appears in files: mostly with padding, occasionally without.
fn decode_base64(field: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(field)
        .or_else(|_| STANDARD_NO_PAD.decode(field))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const ED25519_OTHER: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G2sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const ED25519_HASHED: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
    const HASHED_EXAMPLE_COM: &str = "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak=";

    fn blob(base64_body: &str) -> Vec<u8> {
        STANDARD.decode(base64_body).expect("base64 vector")
    }

    #[test]
    fn comment_blank_and_marker_lines_are_handled() {
        let text = "\
# a comment

db.internal ssh-ed25519 {ED25519}
";
        let text = text.replace("{ED25519}", ED25519);
        assert_eq!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn port_22_is_spelled_without_brackets_and_other_ports_with_them() {
        let text = format!("db.internal ssh-ed25519 {ED25519}\n");
        // Wrong spelling for the port: not a match, so the host looks unknown.
        assert!(matches!(
            check_text(&text, "test", "db.internal", 2222, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
        let text = format!("[db.internal]:2222 ssh-ed25519 {ED25519}\n");
        assert_eq!(
            check_text(&text, "test", "db.internal", 2222, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn comma_separated_host_lists_match_any_member() {
        let text = format!("db.internal,10.0.0.7,[10.0.0.8]:2222 ssh-ed25519 {ED25519}\n");
        for (host, port) in [("db.internal", 22), ("10.0.0.7", 22), ("10.0.0.8", 2222)] {
            assert_eq!(
                check_text(&text, "test", host, port, &blob(ED25519)).unwrap(),
                HostKeyVerdict::Matched,
                "{host}:{port}"
            );
        }
        assert!(matches!(
            check_text(&text, "test", "other.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn wildcards_and_negation_follow_ssh() {
        let text = format!("*.internal,!bad.internal ssh-ed25519 {ED25519}\n");
        assert_eq!(
            check_text(&text, "test", "good.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
        // The negated pattern matches, so the line does not, even though `*` does.
        assert!(matches!(
            check_text(&text, "test", "bad.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
        let text = format!("host?.internal ssh-ed25519 {ED25519}\n");
        assert_eq!(
            check_text(&text, "test", "host1.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn hashed_entries_match() {
        let text = format!("{HASHED_EXAMPLE_COM} ssh-ed25519 {ED25519_HASHED}\n");
        assert_eq!(
            check_text(&text, "test", "example.com", 22, &blob(ED25519_HASHED)).unwrap(),
            HostKeyVerdict::Matched
        );
        assert!(matches!(
            check_text(&text, "test", "elsewhere.com", 22, &blob(ED25519_HASHED)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn a_different_key_for_a_known_host_is_a_mismatch() {
        let text = format!("db.internal ssh-ed25519 {ED25519}\n");
        let verdict = check_text(&text, "test", "db.internal", 22, &blob(ED25519_OTHER)).unwrap();
        match verdict {
            HostKeyVerdict::Mismatch { recorded } => {
                assert_eq!(recorded.len(), 1);
                assert_eq!(recorded[0].line(), 1);
                assert_eq!(recorded[0].key_type(), "ssh-ed25519");
                assert_eq!(recorded[0].fingerprint(), fingerprint(&blob(ED25519)));
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_match_wins_over_a_mismatch_on_another_line() {
        let text =
            format!("db.internal ssh-ed25519 {ED25519_OTHER}\ndb.internal ssh-ed25519 {ED25519}\n");
        assert_eq!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn a_revoked_key_is_refused_and_is_not_unknown() {
        let text = format!("@revoked db.internal ssh-ed25519 {ED25519}\n");
        assert_eq!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Revoked { line: 1 }
        );
        // Revocation is by key: a different key for the same host is not refused, which
        // is what lets a rotated key be recorded beside the old one's revocation.
        assert!(matches!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519_OTHER)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn a_revocation_beats_a_recorded_key_for_the_same_blob() {
        let text = format!(
            "db.internal ssh-ed25519 {ED25519}\n@revoked db.internal ssh-ed25519 {ED25519}\n"
        );
        assert_eq!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Revoked { line: 2 }
        );
    }

    #[test]
    fn a_certificate_authority_line_is_not_a_host_key() {
        let text = format!("@cert-authority *.internal ssh-ed25519 {ED25519}\n");
        match check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap() {
            HostKeyVerdict::Unknown {
                covered_by_certificate_authority,
                fingerprint: shown,
            } => {
                assert!(covered_by_certificate_authority);
                assert_eq!(shown, fingerprint(&blob(ED25519)));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn a_trailing_comment_does_not_disturb_the_comparison() {
        let text = format!("db.internal ssh-ed25519 {ED25519} trailing comment\n");
        assert_eq!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn a_malformed_line_names_itself_instead_of_being_skipped() {
        for bad in [
            "db.internal ssh-ed25519\n",
            "db.internal ssh-ed25519 !!!not base64!!!\n",
            "@host-cert db.internal ssh-ed25519 AAAA\n",
            "@revoked\n",
        ] {
            match check_text(bad, "test", "db.internal", 22, &blob(ED25519)) {
                Err(Error::MalformedKnownHosts { line, file }) => {
                    assert_eq!(line, 1);
                    assert_eq!(file, "test");
                }
                other => panic!("expected a malformed-line error for {bad:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_key_whose_token_disagrees_with_its_body_is_malformed() {
        // The body is ed25519 while the token claims ecdsa: ssh would not read this
        // line, and neither will we.
        let text = format!("db.internal ecdsa-sha2-nistp256 {ED25519}\n");
        assert!(matches!(
            check_text(&text, "test", "db.internal", 22, &blob(ED25519)),
            Err(Error::MalformedKnownHosts { line: 1, .. })
        ));
    }

    #[test]
    fn append_creates_the_file_0600_and_appends_without_gluing_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("known_hosts");

        append(&path, "db.internal", 2222, &blob(ED25519)).expect("first append");
        assert_eq!(
            check(&path, "db.internal", 2222, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "known_hosts should be user-only");
        }

        // A file with no final newline: the next line must not be glued onto it.
        std::fs::write(&path, format!("db.internal ssh-ed25519 {ED25519}")).unwrap();
        append(&path, "db.internal", 22, &blob(ED25519)).expect("second append");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "{text:?}");
        assert!(text.ends_with('\n'));
        assert_eq!(
            check(&path, "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn a_missing_file_is_unknown_not_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("known_hosts");
        assert!(matches!(
            check(&path, "db.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn glob_does_not_blow_up_on_pathological_patterns() {
        // The naive recursive matcher takes exponential time on this shape; the
        // iterative one is linear. Both directions are checked so the test cannot pass
        // by returning false always.
        let pattern = "*a*a*a*a*a*a*a*a*b";
        assert!(!glob(pattern, &"a".repeat(64)));
        let mut with_b = "a".repeat(64);
        with_b.push('b');
        assert!(glob(pattern, &with_b));
    }

    /// The independent check: `ssh-keygen -H` writes the hashed file and `ssh-keygen
    /// -F` reads it, so agreement with both is agreement with OpenSSH rather than with
    /// our own reading of the format.
    #[test]
    fn a_hashed_file_written_by_ssh_keygen_matches() {
        if which("ssh-keygen").is_none() {
            eprintln!("skipped: ssh-keygen is not on PATH");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("known_hosts");
        std::fs::write(
            &path,
            format!("db.internal,10.0.0.7 ssh-ed25519 {ED25519}\n"),
        )
        .unwrap();
        let status = std::process::Command::new("ssh-keygen")
            .args(["-H", "-f"])
            .arg(&path)
            // It reports the unhashed backup it keeps beside the file, which is noise
            // here: the backup is in a temp directory that is removed with the test.
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run ssh-keygen -H");
        assert!(status.success(), "ssh-keygen -H failed");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("|1|"),
            "ssh-keygen -H did not hash the file: {text:?}"
        );
        for host in ["db.internal", "10.0.0.7"] {
            let openssh = std::process::Command::new("ssh-keygen")
                .args(["-F", host, "-f"])
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("run ssh-keygen -F");
            assert!(openssh.success(), "ssh-keygen -F {host} found nothing");
            assert_eq!(
                check(&path, host, 22, &blob(ED25519)).unwrap(),
                HostKeyVerdict::Matched,
                "we disagree with ssh-keygen -F for {host}"
            );
        }
        let openssh = std::process::Command::new("ssh-keygen")
            .args(["-F", "elsewhere.internal", "-f"])
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run ssh-keygen -F");
        assert!(!openssh.success(), "ssh-keygen -F knew an unknown host");
        assert!(matches!(
            check(&path, "elsewhere.internal", 22, &blob(ED25519)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    fn which(program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }

    #[test]
    fn append_refuses_bytes_that_are_not_a_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("known_hosts");
        assert!(matches!(
            append(&path, "db.internal", 22, b"not a key"),
            Err(Error::MalformedKeyBlob)
        ));
        assert!(!path.exists(), "a rejected append must not create the file");
    }
}
