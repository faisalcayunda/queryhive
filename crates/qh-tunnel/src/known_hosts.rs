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

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::key::{fingerprint, key_type_of, ServerKey};
use crate::pattern::glob;
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
    /// A `@revoked` line matches this host and this exact key. `file` is the file the
    /// line is in (`None` for text checked in memory).
    Revoked { line: usize, file: Option<PathBuf> },
}

/// A key already on record, with the line it came from so a person can go and look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedKey {
    key_type: String,
    blob: Vec<u8>,
    line: usize,
    file: Option<PathBuf>,
    origin: Option<Origin>,
}

/// Whose file a record came from. Only the app's own file is ever written, and only
/// by [`append_if_absent`]; the other two are the user's and the administrator's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    App,
    User,
    System,
}

impl Origin {
    /// The wire spelling used in host-key details: `app`, `user` or `system`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::App => "app",
            Origin::User => "user",
            Origin::System => "system",
        }
    }
}

/// One `known_hosts` file and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFile {
    pub path: PathBuf,
    pub origin: Origin,
}

impl StoreFile {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, origin: Origin) -> Self {
        Self {
            path: path.into(),
            origin,
        }
    }
}

/// The path OpenSSH reads as `GlobalKnownHostsFile`.
pub const SYSTEM_KNOWN_HOSTS: &str = "/etc/ssh/ssh_known_hosts";

impl RecordedKey {
    /// A key that is not in a file: a pinned key reported as a mismatch against what
    /// the caller approved. `line` is 0 because there is no line.
    pub(crate) fn pinned(key: &ServerKey) -> Self {
        Self {
            key_type: key.key_type().to_owned(),
            blob: key.blob().to_vec(),
            line: 0,
            file: None,
            origin: None,
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

    /// The file it was read from, when it came from one.
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Whose file it was read from, when it came from one.
    #[must_use]
    pub fn origin(&self) -> Option<Origin> {
        self.origin
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
    let mut scan = Scan::new(host, port, blob);
    scan.add_text(text, source, None)?;
    Ok(match scan.finish() {
        Finished::Revoked { file, line } => HostKeyVerdict::Revoked { line, file },
        Finished::Matched => HostKeyVerdict::Matched,
        Finished::Mismatch(recorded) => HostKeyVerdict::Mismatch { recorded },
        Finished::Unknown { ca_covered } => HostKeyVerdict::Unknown {
            fingerprint: fingerprint(blob),
            covered_by_certificate_authority: ca_covered,
        },
    })
}

/// What reading every line for one host and one key adds up to. Shared by the one-file
/// [`check_text`] and the many-file [`check_all`], so the two cannot disagree.
struct Scan {
    spelling: String,
    blob: Vec<u8>,
    recorded: Vec<RecordedKey>,
    matched: bool,
    ca_covered: bool,
    revoked: Option<(Option<PathBuf>, usize)>,
}

enum Finished {
    Revoked { file: Option<PathBuf>, line: usize },
    Matched,
    Mismatch(Vec<RecordedKey>),
    Unknown { ca_covered: bool },
}

impl Scan {
    fn new(host: &str, port: u16, blob: &[u8]) -> Self {
        Self {
            spelling: host_spelling(host, port),
            blob: blob.to_vec(),
            recorded: Vec::new(),
            matched: false,
            ca_covered: false,
            revoked: None,
        }
    }

    /// Fold in one file's text. `origin` is the file and whose it is, when it is a file.
    fn add_text(
        &mut self,
        text: &str,
        source: &str,
        origin: Option<(&Path, Origin)>,
    ) -> Result<(), Error> {
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let Some(entry) = Entry::parse(raw, source, line)? else {
                // Blank lines and `#` comments.
                continue;
            };
            if !entry.matches(&self.spelling) {
                continue;
            }
            match entry.marker {
                // A CA key is not a host key. Recorded here so the caller can refuse a
                // plain key for a host a CA is supposed to cover.
                Some(Marker::CertAuthority) => self.ca_covered = true,
                Some(Marker::Revoked) => {
                    // A revoked entry for some other key says nothing about this one;
                    // ssh reaches the same conclusion, and it is what lets a rotated
                    // key be recorded next to the revocation of the old one.
                    if entry.blob == self.blob && self.revoked.is_none() {
                        self.revoked = Some((origin.map(|(path, _)| path.to_path_buf()), line));
                    }
                }
                None => {
                    if entry.blob == self.blob {
                        self.matched = true;
                    } else {
                        self.recorded.push(RecordedKey {
                            key_type: entry.key_type,
                            blob: entry.blob,
                            line,
                            file: origin.map(|(path, _)| path.to_path_buf()),
                            origin: origin.map(|(_, origin)| origin),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Decided only after every line of every file has been read, because a `@revoked`
    /// line below (or in another file than) a matching one is a refusal: neither line
    /// order nor file order may decide whether a key is accepted.
    fn finish(self) -> Finished {
        if let Some((file, line)) = self.revoked {
            Finished::Revoked { file, line }
        } else if self.matched {
            Finished::Matched
        } else if !self.recorded.is_empty() {
            Finished::Mismatch(self.recorded)
        } else {
            Finished::Unknown {
                ca_covered: self.ca_covered,
            }
        }
    }
}

/// [`check`] over several files at once: the verdict is the sum of all of them, and
/// neither file order nor line order changes it.
///
/// An [`Origin::App`] file is opened with [`open_app_store`]'s checks, so one that is a
/// symlink, owned by someone else or writable by group or others is
/// [`Error::HostKeyStoreUnsafe`], never `Matched` and never `Unknown`. A file that does
/// not exist is empty. A line that cannot be parsed is an error naming its file.
///
/// # Errors
/// [`Error::KnownHostsUnreadable`], [`Error::MalformedKnownHosts`] or
/// [`Error::HostKeyStoreUnsafe`].
pub fn check_all(
    files: &[StoreFile],
    host: &str,
    port: u16,
    blob: &[u8],
) -> Result<HostKeyVerdict, Error> {
    check_all_as(files, host, port, blob, current_euid())
}

/// [`check_all`] with the effective uid injected, so a test does not need to be root.
///
/// # Errors
/// As [`check_all`].
pub fn check_all_as(
    files: &[StoreFile],
    host: &str,
    port: u16,
    blob: &[u8],
    euid: u32,
) -> Result<HostKeyVerdict, Error> {
    let mut scan = Scan::new(host, port, blob);
    for file in files {
        let text = read_store(file, euid)?;
        scan.add_text(
            &text,
            &file.path.display().to_string(),
            Some((&file.path, file.origin)),
        )?;
    }
    Ok(match scan.finish() {
        Finished::Revoked { file, line } => HostKeyVerdict::Revoked { line, file },
        Finished::Matched => HostKeyVerdict::Matched,
        Finished::Mismatch(recorded) => HostKeyVerdict::Mismatch { recorded },
        Finished::Unknown { ca_covered } => HostKeyVerdict::Unknown {
            fingerprint: fingerprint(blob),
            covered_by_certificate_authority: ca_covered,
        },
    })
}

fn read_store(file: &StoreFile, euid: u32) -> Result<String, Error> {
    let unreadable = |source| Error::KnownHostsUnreadable {
        path: file.path.clone(),
        source,
    };
    let mut text = String::new();
    if file.origin == Origin::App {
        if let Some(mut open) = open_app_store(&file.path, euid, false)? {
            open.read_to_string(&mut text).map_err(unreadable)?;
        }
        return Ok(text);
    }
    match std::fs::read_to_string(&file.path) {
        Ok(text) => Ok(text),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(text),
        Err(source) => Err(unreadable(source)),
    }
}

/// The key types on record for `host:port`, in file order, without repeats.
///
/// Only unmarked lines count: a `@revoked` or `@cert-authority` line is not a recorded
/// host key. Hashed entries do count. The caller puts these algorithms first in the
/// negotiation, so a server that offers Ed25519 while the user's file holds its RSA key
/// is compared on RSA instead of being reported as a changed key.
///
/// # Errors
/// The same as [`check_all`].
pub fn recorded_key_types(
    files: &[StoreFile],
    host: &str,
    port: u16,
) -> Result<Vec<String>, Error> {
    let spelling = host_spelling(host, port);
    let euid = current_euid();
    let mut types: Vec<String> = Vec::new();
    for file in files {
        let text = read_store(file, euid)?;
        let name = file.path.display().to_string();
        for (index, raw) in text.lines().enumerate() {
            let Some(entry) = Entry::parse(raw, &name, index + 1)? else {
                continue;
            };
            if entry.marker.is_none()
                && entry.matches(&spelling)
                && !types.contains(&entry.key_type)
            {
                types.push(entry.key_type);
            }
        }
    }
    Ok(types)
}

/// The effective uid of this process, which must own the app's `known_hosts`.
#[must_use]
pub fn current_euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Open the app's known_hosts and check it is safe to trust, on the open descriptor.
///
/// `O_NOFOLLOW` makes a symlink in the last component fail at open; `fstat` on the
/// descriptor, not `lstat` on the path, must then show a regular file owned by `euid`
/// and not writable by group or others. A file that does not exist is `None` when
/// `create` is false, and is created `0600` (appending) when it is true.
///
/// # Errors
/// [`Error::HostKeyStoreUnsafe`], [`Error::KnownHostsUnreadable`] or, when creating,
/// [`Error::Io`].
pub fn open_app_store(path: &Path, euid: u32, create: bool) -> Result<Option<File>, Error> {
    let unsafe_store = |reason| Error::HostKeyStoreUnsafe {
        path: path.to_path_buf(),
        reason,
    };
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(
        i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).expect("O_NOFOLLOW fits an i32"),
    );
    if create {
        options.append(true).create(true).mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(source) if source.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            return Err(unsafe_store("is a symbolic link"))
        }
        Err(source) if create => return Err(Error::Io(source)),
        Err(source) => {
            return Err(Error::KnownHostsUnreadable {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let meta = file
        .metadata()
        .map_err(|source| Error::KnownHostsUnreadable {
            path: path.to_path_buf(),
            source,
        })?;
    if !meta.is_file() {
        return Err(unsafe_store("is not a regular file"));
    }
    if meta.uid() != euid {
        return Err(unsafe_store("is owned by another user"));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(unsafe_store("is writable by group or others"));
    }
    Ok(Some(file))
}

/// Quote one argument for a POSIX shell: single quotes, with `'` written `'\''`. The
/// value is shown to a person who pastes it into a terminal, and a host or path can
/// come from a file someone else wrote.
#[must_use]
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The command that removes a changed key: `ssh-keygen -R '<host>' -f '<file>'`, host
/// spelled as in the file (`[host]:port` off port 22).
#[must_use]
pub fn remove_command(host: &str, port: u16, file: &Path) -> String {
    format!(
        "ssh-keygen -R {} -f {}",
        shell_quote(&host_spelling(host, port)),
        shell_quote(&file.display().to_string())
    )
}

/// Whether `host` is safe to write into a `known_hosts` line and a copied command:
/// 1 to 253 of ASCII letters, digits, `.`, `-`, `_` and `:` (IPv6), not starting with
/// `-`. Anything else (quotes, spaces, commas, `*`, newlines) is column or shell
/// injection once the name leaves the settings it came from.
#[must_use]
pub fn is_valid_host(host: &str) -> bool {
    (1..=253).contains(&host.len())
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
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

/// Record `blob` for `host:port` in the app's own file, unless it is already there.
///
/// One descriptor does the lot: open (`0600`, `O_NOFOLLOW`), the safety checks of
/// [`open_app_store`], the read that decides idempotence, and the append. The line is
/// written with a single `write_all`, host lowercased, unhashed, ending in an
/// `# accepted by QueryHive <UTC time>` comment. Returns whether a line was written.
/// Nothing here ever removes or rewrites a line.
///
/// # Errors
/// [`Error::MalformedKeyBlob`], [`Error::HostKeyStoreUnsafe`],
/// [`Error::MalformedKnownHosts`] (a broken file is not appended to), I/O errors.
pub fn append_if_absent(path: &Path, host: &str, port: u16, blob: &[u8]) -> Result<bool, Error> {
    append_if_absent_as(path, host, port, blob, current_euid())
}

/// [`append_if_absent`] with the effective uid injected.
///
/// # Errors
/// As [`append_if_absent`].
pub fn append_if_absent_as(
    path: &Path,
    host: &str,
    port: u16,
    blob: &[u8],
    euid: u32,
) -> Result<bool, Error> {
    if !is_valid_host(host) {
        return Err(Error::Usage(
            "the host name has characters that cannot be written to known_hosts",
        ));
    }
    let key = ServerKey::from_blob(blob.to_vec())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_0700(parent)?;
        }
    }
    let Some(mut file) = open_app_store(path, euid, true)? else {
        return Err(Error::Io(std::io::ErrorKind::NotFound.into()));
    };
    let mut text = String::new();
    file.read_to_string(&mut text)?;

    let host = host.to_ascii_lowercase();
    let mut scan = Scan::new(&host, port, blob);
    scan.add_text(
        &text,
        &path.display().to_string(),
        Some((path, Origin::App)),
    )?;
    if scan.matched {
        return Ok(false);
    }

    let mut line = String::new();
    // A missing final newline would glue two keys into one line and lose both.
    if !text.is_empty() && !text.ends_with('\n') {
        line.push('\n');
    }
    line.push_str(&format!(
        "{} # accepted by QueryHive {}\n",
        key.known_hosts_line(&host, port),
        utc_iso8601(std::time::SystemTime::now())
    ));
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(true)
}

/// `2026-10-06T12:34:56Z`, from days-since-epoch arithmetic so no time crate is needed.
fn utc_iso8601(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
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
            HostKeyVerdict::Revoked {
                line: 1,
                file: None
            }
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
            HostKeyVerdict::Revoked {
                line: 2,
                file: None
            }
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

#[cfg(test)]
mod trust_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn blob(name: &str, data: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(name.len())
            .expect("short")
            .to_be_bytes()
            .to_vec();
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        out
    }

    fn key(seed: u8) -> Vec<u8> {
        blob("ssh-ed25519", &[seed; 32])
    }

    fn rsa(seed: u8) -> Vec<u8> {
        blob("ssh-rsa", &[seed; 64])
    }

    fn line(hosts: &str, blob: &[u8]) -> String {
        format!(
            "{hosts} {} {}\n",
            key_type_of(blob).expect("type"),
            STANDARD.encode(blob)
        )
    }

    struct Dir(tempfile::TempDir);

    impl Dir {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("tempdir"))
        }

        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.path().join(name);
            std::fs::write(&path, text).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
            path
        }
    }

    fn files(user: &Path, system: &Path, app: &Path) -> Vec<StoreFile> {
        vec![
            StoreFile::new(user, Origin::User),
            StoreFile::new(system, Origin::System),
            StoreFile::new(app, Origin::App),
        ]
    }

    #[test]
    fn a_match_in_the_app_file_wins_over_a_different_key_in_the_user_file() {
        let dir = Dir::new();
        let user = dir.file("user", &line("bastion", &key(1)));
        let system = dir.path_missing("system");
        let app = dir.file("app", &line("bastion", &key(2)));
        // The user's file holds another key for this host, so there is a record that
        // disagrees; but the presented key is on record in the app file.
        assert_eq!(
            check_all(&files(&user, &system, &app), "bastion", 22, &key(2)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    impl Dir {
        fn path_missing(&self, name: &str) -> PathBuf {
            self.0.path().join(name)
        }
    }

    #[test]
    fn revoked_in_any_file_beats_a_match_in_another() {
        let dir = Dir::new();
        let revoked = format!(
            "@revoked bastion {} {}\n",
            "ssh-ed25519",
            STANDARD.encode(key(1))
        );
        let user = dir.file("user", &revoked);
        let system = dir.path_missing("system");
        let app = dir.file("app", &line("bastion", &key(1)));
        let verdict = check_all(&files(&user, &system, &app), "bastion", 22, &key(1)).unwrap();
        assert_eq!(
            verdict,
            HostKeyVerdict::Revoked {
                line: 1,
                file: Some(user.clone())
            }
        );
        // The same in the system file, and with the revocation read before the match.
        let system = dir.file("system", &revoked);
        let user = dir.path_missing("user2");
        let verdict = check_all(&files(&user, &system, &app), "bastion", 22, &key(1)).unwrap();
        assert!(matches!(verdict, HostKeyVerdict::Revoked { file: Some(f), .. } if f == system));
    }

    #[test]
    fn a_changed_key_recorded_in_one_file_is_a_mismatch_with_its_origin() {
        let dir = Dir::new();
        let user = dir.file("user", "");
        let system = dir.file("system", &format!("# pinned\n{}", line("bastion", &key(7))));
        let app = dir.path_missing("app");
        let HostKeyVerdict::Mismatch { recorded } =
            check_all(&files(&user, &system, &app), "bastion", 22, &key(8)).unwrap()
        else {
            panic!("expected a mismatch");
        };
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].origin(), Some(Origin::System));
        assert_eq!(recorded[0].file(), Some(system.as_path()));
        assert_eq!(recorded[0].line(), 2);
        assert_eq!(Origin::System.as_str(), "system");
        assert_eq!(Origin::App.as_str(), "app");
        assert_eq!(Origin::User.as_str(), "user");
    }

    #[test]
    fn records_from_every_file_are_listed_in_file_order() {
        let dir = Dir::new();
        let user = dir.file("user", &line("bastion", &key(1)));
        let system = dir.path_missing("system");
        let app = dir.file("app", &line("bastion", &key(2)));
        let HostKeyVerdict::Mismatch { recorded } =
            check_all(&files(&user, &system, &app), "bastion", 22, &key(3)).unwrap()
        else {
            panic!("expected a mismatch");
        };
        let origins: Vec<_> = recorded.iter().map(RecordedKey::origin).collect();
        assert_eq!(origins, [Some(Origin::User), Some(Origin::App)]);
    }

    #[test]
    fn nothing_anywhere_is_unknown_and_missing_files_are_empty() {
        let dir = Dir::new();
        let all = files(
            &dir.path_missing("a"),
            &dir.path_missing("b"),
            &dir.path_missing("c"),
        );
        assert_eq!(
            check_all(&all, "bastion", 22, &key(1)).unwrap(),
            HostKeyVerdict::Unknown {
                fingerprint: fingerprint(&key(1)),
                covered_by_certificate_authority: false
            }
        );
    }

    #[test]
    fn a_ca_line_in_any_file_marks_the_host_as_covered() {
        let dir = Dir::new();
        let ca = format!(
            "@cert-authority *.corp {} {}\n",
            "ssh-ed25519",
            STANDARD.encode(key(9))
        );
        let user = dir.file("user", &ca);
        let all = files(&user, &dir.path_missing("s"), &dir.path_missing("a"));
        assert_eq!(
            check_all(&all, "db.corp", 22, &key(1)).unwrap(),
            HostKeyVerdict::Unknown {
                fingerprint: fingerprint(&key(1)),
                covered_by_certificate_authority: true
            }
        );
        // A CA key is never matched as a host key, even when it is the presented key.
        assert!(matches!(
            check_all(&all, "db.corp", 22, &key(9)).unwrap(),
            HostKeyVerdict::Unknown {
                covered_by_certificate_authority: true,
                ..
            }
        ));
    }

    #[test]
    fn ports_hashed_hosts_and_case_are_matched_in_every_file() {
        let dir = Dir::new();
        let salt = [5u8; 20];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"[bastion.corp]:2222");
        let hashed = format!(
            "|1|{}|{}",
            STANDARD.encode(salt),
            STANDARD.encode(mac.finalize().into_bytes())
        );
        let user = dir.file("user", &line(&hashed, &key(1)));
        let app = dir.file("app", &line("[other.corp]:2222", &key(2)));
        let all = files(&user, &dir.path_missing("s"), &app);
        assert_eq!(
            check_all(&all, "Bastion.Corp", 2222, &key(1)).unwrap(),
            HostKeyVerdict::Matched
        );
        // Port 22 is a different name from port 2222.
        assert!(matches!(
            check_all(&all, "bastion.corp", 22, &key(1)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
        assert_eq!(
            check_all(&all, "other.corp", 2222, &key(2)).unwrap(),
            HostKeyVerdict::Matched
        );
        assert!(matches!(
            check_all(&all, "other.corp", 22, &key(2)).unwrap(),
            HostKeyVerdict::Unknown { .. }
        ));
    }

    #[test]
    fn a_broken_line_in_any_file_is_an_error_naming_that_file() {
        let dir = Dir::new();
        let user = dir.file("user", &line("a", &key(1)));
        let system = dir.file("system", "bastion ssh-ed25519 not-base64!!\n");
        let all = files(&user, &system, &dir.path_missing("a"));
        match check_all(&all, "other", 22, &key(1)) {
            Err(Error::MalformedKnownHosts { file, line }) => {
                assert!(file.ends_with("system"), "{file}");
                assert_eq!(line, 1);
            }
            other => panic!("expected MalformedKnownHosts, got {other:?}"),
        }
        let unreadable = StoreFile::new(dir.0.path(), Origin::User);
        assert!(matches!(
            check_all(&[unreadable], "x", 22, &key(1)),
            Err(Error::KnownHostsUnreadable { .. })
        ));
    }

    #[test]
    fn append_if_absent_writes_one_lowercase_line_with_a_comment_and_is_idempotent() {
        let dir = Dir::new();
        let path = dir.0.path().join("nested").join("known_hosts");
        assert!(append_if_absent(&path, "Bastion.Corp", 2222, &key(1)).unwrap());
        assert!(!append_if_absent(&path, "bastion.corp", 2222, &key(1)).unwrap());
        assert!(!append_if_absent(&path, "BASTION.CORP", 2222, &key(1)).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        let first = text.lines().next().unwrap();
        assert!(
            first.starts_with("[bastion.corp]:2222 ssh-ed25519 "),
            "{first}"
        );
        assert!(first.contains(" # accepted by QueryHive 20"), "{first}");
        assert!(first.ends_with('Z'), "{first}");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        // The trailing comment does not disturb matching, and another key is added.
        assert_eq!(
            check(&path, "bastion.corp", 2222, &key(1)).unwrap(),
            HostKeyVerdict::Matched
        );
        assert!(append_if_absent(&path, "bastion.corp", 2222, &key(2)).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
    }

    #[test]
    fn append_if_absent_fixes_a_missing_newline_and_refuses_corrupt_files_and_non_keys() {
        let dir = Dir::new();
        let path = dir.file("known_hosts", line("a", &key(1)).trim_end());
        assert!(append_if_absent(&path, "b", 22, &key(2)).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");

        let corrupt = dir.file("corrupt", "this is not a known_hosts line\n");
        assert!(matches!(
            append_if_absent(&corrupt, "b", 22, &key(2)),
            Err(Error::MalformedKnownHosts { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&corrupt).unwrap(),
            "this is not a known_hosts line\n"
        );

        let never = dir.path_missing("never");
        assert!(matches!(
            append_if_absent(&never, "b", 22, b"junk"),
            Err(Error::MalformedKeyBlob)
        ));
        assert!(!never.exists());
    }

    #[test]
    fn append_if_absent_refuses_a_host_that_would_inject_a_column_or_a_line() {
        let dir = Dir::new();
        for bad in ["a b", "a,b", "a\nb", "*.corp", "a?b", "", "-h", "a'b"] {
            let path = dir.path_missing("never");
            assert!(
                matches!(
                    append_if_absent(&path, bad, 22, &key(1)),
                    Err(Error::Usage(_))
                ),
                "{bad:?}"
            );
            assert!(!path.exists(), "{bad:?}");
        }
    }

    #[test]
    fn append_if_absent_never_removes_or_rewrites_a_line() {
        let dir = Dir::new();
        let original = format!("# mine\n{}", line("a", &key(1)));
        let path = dir.file("known_hosts", &original);
        append_if_absent(&path, "b", 22, &key(2)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(&original), "{text}");
    }

    #[test]
    fn the_app_store_must_be_a_regular_file_we_own_that_others_cannot_write() {
        let dir = Dir::new();
        let euid = current_euid();
        for mode in [0o600, 0o644, 0o640] {
            let path = dir.file(&format!("ok{mode:o}"), &line("a", &key(1)));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                open_app_store(&path, euid, false).unwrap().is_some(),
                "{mode:o}"
            );
        }
        for mode in [0o666, 0o620, 0o602, 0o660] {
            let path = dir.file(&format!("bad{mode:o}"), &line("a", &key(1)));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                matches!(
                    open_app_store(&path, euid, false),
                    Err(Error::HostKeyStoreUnsafe { .. })
                ),
                "{mode:o}"
            );
        }
        // Someone else's file, with an injected owner so the test needs no root.
        let owned = dir.file("owned", "");
        assert!(matches!(
            open_app_store(&owned, euid.wrapping_add(1), false),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        let missing = dir.path_missing("missing");
        assert!(open_app_store(&missing, euid, false).unwrap().is_none());
        assert!(matches!(
            open_app_store(dir.0.path(), euid, false),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
    }

    #[test]
    fn a_symlink_is_refused_for_reading_and_for_writing() {
        let dir = Dir::new();
        let target = dir.file("target", &line("bastion", &key(1)));
        let link = dir.path_missing("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            open_app_store(&link, current_euid(), false),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert!(matches!(
            append_if_absent(&link, "bastion", 22, &key(2)),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            line("bastion", &key(1))
        );
        // The user's own file may be a symlink (dotfile managers do that).
        let all = [StoreFile::new(&link, Origin::User)];
        assert_eq!(
            check_all(&all, "bastion", 22, &key(1)).unwrap(),
            HostKeyVerdict::Matched
        );
    }

    #[test]
    fn an_unsafe_app_file_is_never_matched_or_unknown_when_read() {
        let dir = Dir::new();
        let app = dir.file("app", &line("bastion", &key(1)));
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o666)).unwrap();
        let all = [StoreFile::new(&app, Origin::App)];
        assert!(matches!(
            check_all(&all, "bastion", 22, &key(1)),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert!(matches!(
            check_all(&all, "bastion", 22, &key(2)),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert!(matches!(
            recorded_key_types(&all, "bastion", 22),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert!(matches!(
            append_if_absent(&app, "bastion", 22, &key(3)),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
    }

    #[test]
    fn the_checks_run_on_the_descriptor_not_the_path() {
        let dir = Dir::new();
        let path = dir.file("app", &line("bastion", &key(1)));
        let mut open = open_app_store(&path, current_euid(), false)
            .unwrap()
            .unwrap();
        // Swap the path for a symlink to something else after the open.
        let elsewhere = dir.file("elsewhere", &line("bastion", &key(2)));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let mut text = String::new();
        open.read_to_string(&mut text).unwrap();
        assert_eq!(text, line("bastion", &key(1)));
    }

    #[test]
    fn recorded_key_types_lists_plain_lines_only_including_hashed_ones() {
        let dir = Dir::new();
        let salt = [3u8; 20];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(b"bastion");
        let hashed = format!(
            "|1|{}|{}",
            STANDARD.encode(salt),
            STANDARD.encode(mac.finalize().into_bytes())
        );
        let text = [
            line(&hashed, &rsa(1)),
            format!("@revoked bastion ssh-ed25519 {}\n", STANDARD.encode(key(1))),
            format!(
                "@cert-authority bastion ssh-ed25519 {}\n",
                STANDARD.encode(key(2))
            ),
            line("bastion", &rsa(2)),
            line("elsewhere", &key(3)),
        ]
        .concat();
        let user = dir.file("user", &text);
        let app = dir.file(
            "app",
            &line("bastion", &blob("ecdsa-sha2-nistp256", &[1; 8])),
        );
        let all = [
            StoreFile::new(&user, Origin::User),
            StoreFile::new(&app, Origin::App),
        ];
        assert_eq!(
            recorded_key_types(&all, "bastion", 22).unwrap(),
            ["ssh-rsa", "ecdsa-sha2-nistp256"]
        );
        assert!(recorded_key_types(&all, "nobody", 22).unwrap().is_empty());
        // A host with only an unnegotiable type still has a record: a different key is a
        // Mismatch, never Unknown.
        let dss = dir.file("dss", &line("old", &blob("ssh-dss", &[1; 8])));
        assert!(matches!(
            check_all(&[StoreFile::new(&dss, Origin::User)], "old", 22, &key(1)).unwrap(),
            HostKeyVerdict::Mismatch { .. }
        ));
    }

    #[test]
    fn timestamps_are_utc_iso_8601() {
        let at = |secs| utc_iso8601(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(at(1_791_245_696), "2026-10-06T00:14:56Z");
    }

    #[test]
    fn shell_quote_round_trips_through_sh() {
        for value in [
            "plain",
            "it's",
            "a b",
            "x;rm -rf /",
            "$(touch pwned)",
            "`id`",
            "line\nbreak",
            "-rf",
            "/Users/O'Brien/Library/known_hosts",
            "''",
        ] {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", shell_quote(value)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), value);
        }
        assert_eq!(
            remove_command("bastion", 22, Path::new("/Users/O'Brien/kh")),
            "ssh-keygen -R 'bastion' -f '/Users/O'\\''Brien/kh'"
        );
        assert_eq!(
            remove_command("bastion", 2222, Path::new("/kh")),
            "ssh-keygen -R '[bastion]:2222' -f '/kh'"
        );
    }

    #[test]
    fn host_validation_rejects_anything_that_could_inject() {
        for good in [
            "bastion",
            "db-1.corp_x",
            "10.0.0.1",
            "::1",
            "fe80::1",
            &"a".repeat(253),
        ] {
            assert!(is_valid_host(good), "{good}");
        }
        for bad in [
            "",
            "-oProxyCommand=x",
            "it's",
            "a b",
            "a;b",
            "a/b",
            "a,b",
            "a*",
            "a\nb",
            "$(x)",
            "ünï",
            &"a".repeat(254),
        ] {
            assert!(!is_valid_host(bad), "{bad:?}");
        }
    }
}
