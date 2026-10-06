//! `~/.ssh/config` aliases, resolved without ever guessing.
//!
//! A connection can name a bastion by alias (`SSH_HOST=prod-bastion`) and have the host,
//! user, port and key file filled in from the user's own config. The rule that keeps this
//! honest is: **a directive we do not apply is a refusal that names it, never a silent
//! skip.** `ssh` would obey `ProxyJump`, `HostKeyAlias`, `UserKnownHostsFile`,
//! `StrictHostKeyChecking no` or `IdentitiesOnly yes`, and each one changes where we
//! connect, who we trust or which credentials we offer. Ignoring one would connect to
//! somewhere the user did not intend, so resolution fails with the directive name and
//! line instead (and the message tells the user to fill the fields in explicitly).
//!
//! Supported: `HostName` (`%h`, `%%`), `User`, `Port`, `IdentityFile` (`~/`, `%d`, `%h`,
//! `%%`; `%h` there is the resolved `HostName`, as in OpenSSH) and `Include`. A short
//! list of cosmetic directives is ignored because none of them can change the
//! destination, the trust or the credentials. `Match` anywhere in
//! the loaded files refuses every resolution: its criteria are not evaluated and could
//! add any of the refused directives.
//!
//! OpenSSH semantics that are copied: lines apply in the order they are read, the first
//! value wins per keyword, `IdentityFile` accumulates, and a `Host` line matches when a
//! positive pattern matches and no `!` pattern does. An `Include` is read where it
//! stands and in the scope of the line it is on: inside a `Host` block that does not
//! apply, nothing in the included file does (not even its own `Host` lines), and when the
//! included file ends the surrounding block is as it was (a `Host` inside it does not leak
//! into the parent). `tests/ssh_config.rs` checks all of this against `ssh -G`.
//!
//! This module reads files and nothing else. It never opens a network connection.

use std::path::{Path, PathBuf};

use crate::pattern::glob;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LINES: usize = 20_000;
const MAX_FILES: usize = 32;
const MAX_DEPTH: usize = 4;
const MAX_ALIASES: usize = 500;

/// Directives that cannot change the destination, the trust, the credentials or the
/// algorithms. Adding one needs that argument written down in the change.
const COSMETIC: &[&str] = &[
    "addkeystoagent",
    "usekeychain",
    "serveraliveinterval",
    "serveralivecountmax",
    "tcpkeepalive",
    "compression",
    "forwardagent",
    "forwardx11",
    "forwardx11trusted",
    "loglevel",
    "sendenv",
    "setenv",
    "hashknownhosts",
    "controlmaster",
    "controlpath",
    "controlpersist",
    "visualhostkey",
    "connecttimeout",
    "connectionattempts",
    "batchmode",
    "numberofpasswordprompts",
    "addressfamily",
    "requesttty",
    "tag",
    "ipqos",
];

/// What an alias resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The name to connect to, and the name `known_hosts` records. The alias itself when
    /// no `HostName` applies.
    pub host_name: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// In file order. OpenSSH tries all of them; the caller decides how many it uses.
    pub identity_files: Vec<PathBuf>,
}

/// Why a config could not be loaded or an alias could not be resolved. Messages carry the
/// file, the line and the directive name, and never a value: values can be commands or
/// key paths.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SshConfigError {
    /// No `Host` block names the alias. Not "use it as a host name": that would hide a
    /// typo until the connection fails somewhere else.
    #[error("no Host entry in the ssh config names the alias \"{alias}\"")]
    NotFound { alias: String },

    /// A directive in a block that applies to the alias is one we do not honour.
    #[error("{} line {line}: \"{directive}\" is not supported by QueryHive, so alias \"{alias}\" cannot be used. Fill in host, user and key explicitly instead.", file.display())]
    Unsupported {
        directive: String,
        alias: String,
        file: PathBuf,
        line: usize,
    },

    /// A `%` token we do not expand, in `HostName` or `IdentityFile`.
    #[error("{} line {line}: \"{directive}\" uses the token \"{token}\", which QueryHive does not expand, so alias \"{alias}\" cannot be used. Fill in host, user and key explicitly instead.", file.display())]
    UnsupportedToken {
        directive: String,
        token: String,
        alias: String,
        file: PathBuf,
        line: usize,
    },

    /// More `Include`d files than the limit, nested too deep, or a cycle.
    #[error("the ssh config includes too many files, or includes itself")]
    TooManyIncludes,

    /// A file or the whole set is past the size limits.
    #[error("{} is too large to read as an ssh config", file.display())]
    TooLarge { file: PathBuf },

    /// A line that is not valid config syntax, or a value of the wrong shape.
    #[error("{} line {line} is not valid ssh config syntax", file.display())]
    Malformed { file: PathBuf, line: usize },

    /// `~` or a relative `Include` with no `$HOME` to anchor it.
    #[error("$HOME is not set, so ~/.ssh cannot be located")]
    NoHomeDirectory,

    /// The file exists but cannot be read. A file that is not there is an empty config.
    #[error("cannot read {}: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug)]
struct Directive {
    /// As written, for messages.
    name: String,
    key: String,
    args: Vec<String>,
    file: PathBuf,
    line: usize,
}

/// What a line of the config means to a resolution, in the order OpenSSH reads them, with
/// every `Include` expanded where it stands.
#[derive(Debug)]
enum Item {
    /// A `Host` line. Before the first one, directives apply to every alias. An empty
    /// list is a block that can never match (what follows `Match`).
    Host(Vec<String>),
    /// An `Include` begins. What follows, up to the matching [`Item::Leave`], is read in
    /// the state this line is in.
    Enter,
    /// The included file ended: the surrounding block is as it was before the `Include`.
    Leave,
    Directive(Directive),
}

/// A parsed config: every file, `Include`s followed, nothing evaluated yet.
#[derive(Debug)]
pub struct SshConfig {
    items: Vec<Item>,
    match_at: Option<(PathBuf, usize)>,
    /// First `Include` whose argument we cannot expand faithfully (tokens, env vars,
    /// `~user`, classes, wildcards outside the last component).
    unsupported_include_at: Option<(PathBuf, usize)>,
    home: Option<PathBuf>,
}

/// Load `path` (usually `~/.ssh/config`) and everything it includes. `$HOME` anchors `~`
/// and relative `Include`s, which are relative to `~/.ssh` as in OpenSSH. A missing
/// `path` is an empty config, as it is for `ssh`.
///
/// # Errors
/// [`SshConfigError::Unreadable`], [`SshConfigError::Malformed`],
/// [`SshConfigError::TooManyIncludes`] or [`SshConfigError::TooLarge`].
pub fn load(path: &Path) -> Result<SshConfig, SshConfigError> {
    load_in(path, std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// [`load`] with the home directory given, so a test or a caller with its own notion of
/// home does not depend on the environment.
///
/// # Errors
/// As [`load`].
pub fn load_in(path: &Path, home: Option<&Path>) -> Result<SshConfig, SshConfigError> {
    let mut loader = Loader {
        home,
        files: 0,
        lines: 0,
        config: SshConfig {
            items: Vec::new(),
            match_at: None,
            unsupported_include_at: None,
            home: home.map(Path::to_path_buf),
        },
    };
    loader.read(path, 0)?;
    Ok(loader.config)
}

struct Loader<'a> {
    home: Option<&'a Path>,
    files: usize,
    lines: usize,
    config: SshConfig,
}

impl Loader<'_> {
    fn read(&mut self, path: &Path, depth: usize) -> Result<(), SshConfigError> {
        if depth > MAX_DEPTH || self.files >= MAX_FILES {
            return Err(SshConfigError::TooManyIncludes);
        }
        let unreadable = |source| SshConfigError::Unreadable {
            path: path.to_path_buf(),
            source,
        };
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(unreadable(source)),
        };
        if !meta.is_file() {
            return Ok(());
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(SshConfigError::TooLarge {
                file: path.to_path_buf(),
            });
        }
        self.files += 1;
        let text = std::fs::read_to_string(path).map_err(unreadable)?;

        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            self.lines += 1;
            if self.lines > MAX_LINES {
                return Err(SshConfigError::TooLarge {
                    file: path.to_path_buf(),
                });
            }
            let malformed = || SshConfigError::Malformed {
                file: path.to_path_buf(),
                line,
            };
            let Some((name, args)) = tokenize(raw).map_err(|()| malformed())? else {
                continue;
            };
            let key = name.to_ascii_lowercase();
            match key.as_str() {
                "host" => {
                    if args.is_empty() {
                        return Err(malformed());
                    }
                    self.config.items.push(Item::Host(args));
                }
                "match" => {
                    if self.config.match_at.is_none() {
                        self.config.match_at = Some((path.to_path_buf(), line));
                    }
                    self.config.items.push(Item::Host(Vec::new()));
                }
                "include" => {
                    if args.is_empty() {
                        return Err(malformed());
                    }
                    for arg in &args {
                        if include_is_unsupported(arg) {
                            if self.config.unsupported_include_at.is_none() {
                                self.config.unsupported_include_at =
                                    Some((path.to_path_buf(), line));
                            }
                            continue;
                        }
                        for file in self.expand_include(arg)? {
                            // What follows the Include in this file belongs to the block
                            // it was in, whatever `Host` lines the included file has.
                            self.config.items.push(Item::Enter);
                            self.read(&file, depth + 1)?;
                            self.config.items.push(Item::Leave);
                        }
                    }
                }
                _ => self.config.items.push(Item::Directive(Directive {
                    name,
                    key,
                    args,
                    file: path.to_path_buf(),
                    line,
                })),
            }
        }
        Ok(())
    }

    /// The files an `Include` argument names: `~` expanded, relative to `~/.ssh`, `*`
    /// and `?` only in the last component, sorted. Nothing matching is not an error.
    fn expand_include(&self, arg: &str) -> Result<Vec<PathBuf>, SshConfigError> {
        let home = |home: Option<&Path>| {
            home.map(Path::to_path_buf)
                .ok_or(SshConfigError::NoHomeDirectory)
        };
        let path = if let Some(rest) = arg.strip_prefix("~/") {
            home(self.home)?.join(rest)
        } else if arg == "~" {
            home(self.home)?
        } else if Path::new(arg).is_absolute() {
            PathBuf::from(arg)
        } else {
            home(self.home)?.join(".ssh").join(arg)
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(vec![path]);
        };
        if !name.contains(['*', '?']) {
            return Ok(vec![path]);
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let Ok(entries) = std::fs::read_dir(parent) else {
            return Ok(Vec::new());
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|candidate| {
                    // A leading dot is only matched by a pattern that spells it, as in a
                    // shell glob.
                    (!candidate.starts_with('.') || name.starts_with('.')) && glob(name, candidate)
                })
            })
            .map(|entry| entry.path())
            .collect();
        found.sort();
        Ok(found)
    }
}

/// True for an `Include` argument OpenSSH expands in ways we do not: `$`/`%` tokens,
/// `~user`, `[...]` classes, or `*`/`?` before the last path component.
fn include_is_unsupported(arg: &str) -> bool {
    arg.contains(['$', '%', '['])
        || (arg.starts_with('~') && arg != "~" && !arg.starts_with("~/"))
        || arg
            .rsplit_once('/')
            .is_some_and(|(dir, _)| dir.contains(['*', '?']))
}

/// Split one config line into its keyword and arguments, or `None` for a blank or
/// comment line. `Keyword value`, `Keyword=value` and `Keyword = value` are the same;
/// double quotes group a value with spaces; an unquoted `#` starts a comment.
fn tokenize(raw: &str) -> Result<Option<(String, Vec<String>)>, ()> {
    let line = raw.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let name_end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    if name_end == 0 {
        return Err(());
    }
    let (name, mut rest) = line.split_at(name_end);
    rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix('=') {
        rest = after;
    }

    let mut args = Vec::new();
    let mut chars = rest.trim_start().chars().peekable();
    while let Some(&next) = chars.peek() {
        if next.is_whitespace() {
            chars.next();
        } else if next == '#' {
            break;
        } else if next == '"' {
            chars.next();
            let mut value = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => return Err(()),
                }
            }
            args.push(value);
        } else {
            let mut value = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                value.push(c);
                chars.next();
            }
            args.push(value);
        }
    }
    Ok(Some((name.to_owned(), args)))
}

impl SshConfig {
    /// The names a person can pick: every `Host` pattern with no `*`, `?` or `!`, once
    /// each, in file order, at most 500. Never fails because of `Match`: listing does
    /// not resolve anything.
    #[must_use]
    pub fn aliases(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for pattern in self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Host(patterns) => Some(patterns),
                _ => None,
            })
            .flatten()
        {
            if !pattern.contains(['*', '?', '!']) && !names.contains(pattern) {
                names.push(pattern.clone());
                if names.len() == MAX_ALIASES {
                    break;
                }
            }
        }
        names
    }

    /// Resolve `alias` the way `ssh` would for the directives we honour, and refuse by
    /// name for the ones we do not.
    ///
    /// # Errors
    /// [`SshConfigError::Unsupported`] (including `Match` anywhere in the loaded files),
    /// [`SshConfigError::UnsupportedToken`], [`SshConfigError::NotFound`],
    /// [`SshConfigError::Malformed`] or [`SshConfigError::NoHomeDirectory`].
    pub fn resolve(&self, alias: &str) -> Result<Resolved, SshConfigError> {
        let unsupported = |directive: &str, file: &Path, line: usize| SshConfigError::Unsupported {
            directive: directive.to_owned(),
            alias: alias.to_owned(),
            file: file.to_path_buf(),
            line,
        };
        if let Some((file, line)) = &self.match_at {
            return Err(unsupported("Match", file, *line));
        }
        if let Some((file, line)) = &self.unsupported_include_at {
            return Err(unsupported("Include", file, *line));
        }

        // Walk the lines the way OpenSSH reads them. An alias is found when some `Host`
        // line names it by something more specific than a lone `*`; otherwise a typo
        // would resolve to itself and fail elsewhere.
        let mut named = false;
        let mut applying: Vec<&Directive> = Vec::new();
        let mut active = true;
        // The state of each enclosing `Include` line: a `Host` line inside an included
        // file can only apply when the line that included it did.
        let mut scopes: Vec<bool> = Vec::new();
        for item in &self.items {
            match item {
                Item::Host(patterns) => {
                    let in_scope = scopes.last().copied().unwrap_or(true);
                    match block_matches(patterns, alias).filter(|_| in_scope) {
                        Some(specific) => {
                            named |= specific;
                            active = true;
                        }
                        None => active = false,
                    }
                }
                Item::Enter => scopes.push(active),
                Item::Leave => active = scopes.pop().unwrap_or(true),
                Item::Directive(directive) if active => applying.push(directive),
                Item::Directive(_) => {}
            }
        }
        if !named {
            return Err(SshConfigError::NotFound {
                alias: alias.to_owned(),
            });
        }

        let mut host_name = None;
        let mut user = None;
        let mut port = None;
        // Expanded after the loop: `%h` in an `IdentityFile` is the host name the
        // connection resolves to, wherever `HostName` stands relative to it.
        let mut identity_raw: Vec<(&str, &Directive)> = Vec::new();
        for directive in applying {
            let malformed = || SshConfigError::Malformed {
                file: directive.file.clone(),
                line: directive.line,
            };
            let single = || match directive.args.as_slice() {
                [value] => Ok(value.as_str()),
                _ => Err(malformed()),
            };
            let token_error = |token: String| SshConfigError::UnsupportedToken {
                directive: directive.name.clone(),
                token,
                alias: alias.to_owned(),
                file: directive.file.clone(),
                line: directive.line,
            };
            match directive.key.as_str() {
                "hostname" => {
                    let value = expand(single()?, alias, None).map_err(token_error)?;
                    host_name.get_or_insert(value);
                }
                "user" => {
                    let value = single()?.to_owned();
                    user.get_or_insert(value);
                }
                "port" => {
                    let value = single()?.parse::<u16>().map_err(|_| malformed())?;
                    port.get_or_insert(value);
                }
                "identityfile" => identity_raw.push((single()?, directive)),
                key if COSMETIC.contains(&key) => {}
                // Only the settings that are no weaker than our own behaviour pass.
                "stricthostkeychecking"
                    if matches!(single()?.to_ascii_lowercase().as_str(), "yes" | "ask") => {}
                "identitiesonly" if single()?.eq_ignore_ascii_case("no") => {}
                _ => {
                    return Err(unsupported(
                        &directive.name,
                        &directive.file,
                        directive.line,
                    ))
                }
            }
        }

        let host_name = host_name.unwrap_or_else(|| alias.to_owned());
        let home = self.home.as_deref();
        let mut identity_files = Vec::new();
        for (raw, directive) in identity_raw {
            let token_error = |token: String| SshConfigError::UnsupportedToken {
                directive: directive.name.clone(),
                token,
                alias: alias.to_owned(),
                file: directive.file.clone(),
                line: directive.line,
            };
            identity_files.push(if let Some(rest) = raw.strip_prefix("~/") {
                home.ok_or(SshConfigError::NoHomeDirectory)?
                    .join(expand(rest, &host_name, home).map_err(token_error)?)
            } else {
                PathBuf::from(expand(raw, &host_name, home).map_err(token_error)?)
            });
        }

        Ok(Resolved {
            host_name,
            user,
            port,
            identity_files,
        })
    }
}

/// `Some(specific)` when the block matches `alias`: a positive pattern matches and no
/// `!` pattern does. `specific` is false when the only match is a bare `*`.
fn block_matches(patterns: &[String], alias: &str) -> Option<bool> {
    // OpenSSH folds both sides to lowercase before matching a `Host` line.
    let alias = alias.to_ascii_lowercase();
    let mut matched = false;
    let mut specific = false;
    for pattern in patterns {
        let pattern = pattern.to_ascii_lowercase();
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob(negated, &alias) {
                return None;
            }
        } else if glob(&pattern, &alias) {
            matched = true;
            specific |= pattern != "*";
        }
    }
    matched.then_some(specific)
}

/// Expand `%%`, `%h` (`host`) and, when `home` is given, `%d` (home). Any other token is
/// returned as the error. `host` is the name as typed for `HostName` and the resolved
/// `HostName` for `IdentityFile`, which is what OpenSSH substitutes in each.
fn expand(value: &str, host: &str, home: Option<&Path>) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match (chars.next(), home) {
            (Some('%'), _) => out.push('%'),
            (Some('h'), _) => out.push_str(host),
            (Some('d'), Some(home)) => out.push_str(&home.display().to_string()),
            (Some(other), _) => return Err(format!("%{other}")),
            (None, _) => return Err("%".to_owned()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir(dir.path().join(".ssh")).expect("mkdir");
            Self { dir }
        }

        fn home(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.home().join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, text).expect("write");
            path
        }

        fn load(&self, text: &str) -> Result<SshConfig, SshConfigError> {
            let path = self.write(".ssh/config", text);
            load_in(&path, Some(self.home()))
        }

        fn resolve(&self, text: &str, alias: &str) -> Result<Resolved, SshConfigError> {
            self.load(text)?.resolve(alias)
        }
    }

    fn refused(error: SshConfigError) -> (String, usize) {
        match error {
            SshConfigError::Unsupported {
                directive, line, ..
            } => (directive, line),
            other => panic!("expected Unsupported, got {other}"),
        }
    }

    #[test]
    fn an_alias_resolves_with_cosmetics_ignored_and_first_value_winning() {
        let f = Fixture::new();
        let resolved = f
            .resolve(
                "\
Host prod
  HostName bastion.corp
  User deploy
  Port 2222
  IdentityFile ~/.ssh/id_ed25519
  IdentityFile ~/.ssh/id_rsa

Host *
  ServerAliveInterval 30
  AddKeysToAgent yes
  UseKeychain yes
  User ignored
  StrictHostKeyChecking ask
  IdentitiesOnly no
",
                "prod",
            )
            .expect("resolves");
        assert_eq!(resolved.host_name, "bastion.corp");
        assert_eq!(resolved.user.as_deref(), Some("deploy"));
        assert_eq!(resolved.port, Some(2222));
        assert_eq!(
            resolved.identity_files,
            vec![
                f.home().join(".ssh/id_ed25519"),
                f.home().join(".ssh/id_rsa")
            ]
        );
    }

    #[test]
    fn without_hostname_the_alias_is_the_host() {
        let f = Fixture::new();
        let resolved = f.resolve("Host db\n  User qh\n", "db").expect("resolves");
        assert_eq!(resolved.host_name, "db");
        assert_eq!(resolved.port, None);
        assert!(resolved.identity_files.is_empty());
    }

    #[test]
    fn syntax_variants_are_the_same_directive() {
        let f = Fixture::new();
        let resolved = f
            .resolve(
                "host=prod\n  hostname = \"bastion.corp\" # a comment\n  PORT=2200\n",
                "prod",
            )
            .expect("resolves");
        assert_eq!(resolved.host_name, "bastion.corp");
        assert_eq!(resolved.port, Some(2200));
    }

    #[test]
    fn a_typo_is_not_found_not_used_as_a_host() {
        let f = Fixture::new();
        let text = "Host prod\n  HostName a\nHost *\n  User x\n";
        assert!(matches!(
            f.resolve(text, "prdo"),
            Err(SshConfigError::NotFound { .. })
        ));
        // Nothing at all, or a missing file, is the same answer.
        let config = load_in(&f.home().join("nope"), Some(f.home())).expect("missing = empty");
        assert!(config.aliases().is_empty());
        assert!(matches!(
            config.resolve("prod"),
            Err(SshConfigError::NotFound { .. })
        ));
    }

    #[test]
    fn host_matching_ignores_ascii_case_on_both_sides() {
        let f = Fixture::new();
        let text = "Host Bastion *.CORP !Skip.Corp\n  User folded\n";
        for alias in ["bastion", "BASTION", "db.corp", "DB.Corp"] {
            assert_eq!(
                f.resolve(text, alias).expect("resolves").user.as_deref(),
                Some("folded"),
                "{alias}"
            );
        }
        assert!(matches!(
            f.resolve(text, "SKIP.corp"),
            Err(SshConfigError::NotFound { .. })
        ));
    }

    #[test]
    fn wildcard_and_negated_patterns_pick_the_blocks() {
        let f = Fixture::new();
        let text = "\
Host *.corp !skip.corp
  User corp
Host skip.corp
  User skipped
";
        assert_eq!(
            f.resolve(text, "db.corp")
                .expect("resolves")
                .user
                .as_deref(),
            Some("corp")
        );
        assert_eq!(
            f.resolve(text, "skip.corp")
                .expect("resolves")
                .user
                .as_deref(),
            Some("skipped")
        );
    }

    #[test]
    fn tokens_h_and_d_expand_and_others_are_refused_by_name() {
        let f = Fixture::new();
        let resolved = f
            .resolve(
                "Host web\n  HostName %h.example.com\n  IdentityFile %d/keys/%h\n",
                "web",
            )
            .expect("resolves");
        assert_eq!(resolved.host_name, "web.example.com");
        assert_eq!(
            resolved.identity_files,
            // OpenSSH substitutes the resolved HostName for %h in IdentityFile, and the
            // name as typed only in HostName itself (checked against `ssh` in
            // tests/ssh_config.rs).
            vec![PathBuf::from(format!(
                "{}/keys/web.example.com",
                f.home().display()
            ))]
        );
        match f.resolve("Host web\n  IdentityFile %u/key\n", "web") {
            Err(SshConfigError::UnsupportedToken {
                directive,
                token,
                line,
                ..
            }) => {
                assert_eq!(
                    (directive.as_str(), token.as_str(), line),
                    ("IdentityFile", "%u", 2)
                );
            }
            other => panic!("expected UnsupportedToken, got {other:?}"),
        }
    }

    #[test]
    fn match_anywhere_refuses_every_alias_but_listing_still_works() {
        let f = Fixture::new();
        let config = f
            .load("Host prod\n  HostName a\nMatch host prod exec \"true\"\n  ProxyJump evil\n")
            .expect("loads");
        assert_eq!(config.aliases(), vec!["prod".to_owned()]);
        let (directive, line) = refused(config.resolve("prod").expect_err("refused"));
        assert_eq!((directive.as_str(), line), ("Match", 3));
    }

    #[test]
    fn trust_and_routing_directives_are_refused_with_name_and_line() {
        let f = Fixture::new();
        for (directive, value) in [
            ("ProxyJump", "jump"),
            ("ProxyCommand", "nc %h %p"),
            ("HostKeyAlias", "other"),
            ("UserKnownHostsFile", "/dev/null"),
            ("GlobalKnownHostsFile", "/dev/null"),
            ("StrictHostKeyChecking", "no"),
            ("StrictHostKeyChecking", "accept-new"),
            ("VerifyHostKeyDNS", "yes"),
            ("IdentitiesOnly", "yes"),
            ("PreferredAuthentications", "password"),
            ("CertificateFile", "c.pub"),
            ("IdentityAgent", "none"),
            ("HostKeyAlgorithms", "ssh-rsa"),
            ("LocalForward", "1 h:2"),
            ("RemoteCommand", "ls"),
            ("SomethingUnheardOf", "1"),
        ] {
            let text = format!("Host prod\n  HostName a\n  {directive} {value}\n");
            let (refused_directive, line) = refused(f.resolve(&text, "prod").expect_err(directive));
            assert_eq!((refused_directive.as_str(), line), (directive, 3));
        }
    }

    #[test]
    fn the_message_names_the_directive_and_never_the_value() {
        let f = Fixture::new();
        let error = f
            .resolve("Host prod\n  ProxyCommand secret-token-123\n", "prod")
            .expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("line 2"), "{message}");
        assert!(message.contains("\"ProxyCommand\""), "{message}");
        assert!(message.contains("alias \"prod\""), "{message}");
        assert!(!message.contains("secret-token-123"), "{message}");
    }

    #[test]
    fn a_refused_directive_in_a_block_that_does_not_apply_is_harmless() {
        let f = Fixture::new();
        let text = "Host other\n  ProxyJump jump\nHost prod\n  HostName a\n";
        assert_eq!(f.resolve(text, "prod").expect("resolves").host_name, "a");
        // In `Host *` it applies to everything: the known cost of refusing.
        let text = "Host prod\n  HostName a\nHost *\n  ProxyJump jump\n";
        assert!(matches!(
            f.resolve(text, "prod"),
            Err(SshConfigError::Unsupported { .. })
        ));
    }

    #[test]
    fn an_include_we_cannot_expand_refuses_instead_of_being_skipped() {
        let f = Fixture::new();
        for arg in [
            "${HOME}/x",
            "conf.d/*/config",
            "%d/x",
            "~bob/x",
            "conf[12].d",
        ] {
            let text = format!("Include {arg}\nHost prod\n  HostName a\n");
            let (directive, line) = refused(f.resolve(&text, "prod").expect_err(arg));
            assert_eq!((directive.as_str(), line), ("Include", 1), "{arg}");
            // Listing still works.
            assert_eq!(f.load(&text).expect("loads").aliases(), ["prod"]);
        }
        // Last-component glob with no match stays a quiet skip.
        let text = "Include conf.d/*.conf\nHost prod\n  HostName a\n";
        assert_eq!(f.resolve(text, "prod").expect("ok").host_name, "a");
    }

    #[test]
    fn include_follows_relative_paths_globs_sorted_and_skips_missing_files() {
        let f = Fixture::new();
        f.write(".ssh/conf.d/b.conf", "Host prod\n  Port 2\n");
        f.write(
            ".ssh/conf.d/a.conf",
            "Host prod\n  Port 1\n  HostName from-a\n",
        );
        f.write(".ssh/conf.d/.hidden", "Host prod\n  Port 3\n");
        f.write("other/extra", "Host extra\n  HostName x\n");
        let config = f
            .load(
                "Include conf.d/*.conf\nInclude missing/*\nInclude nofile\nInclude ~/other/extra\nHost last\n",
            )
            .expect("loads");
        let resolved = config.resolve("prod").expect("resolves");
        // a.conf sorts first, and the first value wins.
        assert_eq!(resolved.port, Some(1));
        assert_eq!(resolved.host_name, "from-a");
        assert_eq!(config.resolve("extra").expect("resolves").host_name, "x");
        assert_eq!(config.aliases(), ["prod", "extra", "last"]);
    }

    #[test]
    fn an_include_is_read_in_the_scope_of_its_line_and_hands_the_block_back() {
        let f = Fixture::new();
        f.write(
            ".ssh/inc",
            "Host inner\n  User inner-user\nHost *\n  Port 7\n",
        );

        // Inside `Host outer`: the file's `Host *` applies to `outer` and nothing else, its
        // `Host inner` is inert (the line that included it does not apply to `inner`),
        // and when the file ends we are back in `outer`.
        let text = "Host outer\n  Include inc\n  User outer-user\n";
        let outer = f.resolve(text, "outer").expect("resolves");
        assert_eq!(outer.user.as_deref(), Some("outer-user"));
        assert_eq!(outer.port, Some(7));
        assert!(matches!(
            f.resolve(text, "inner"),
            Err(SshConfigError::NotFound { .. })
        ));

        // At the top of the file nothing gates the include, so the file's `Host` lines
        // work, and `Host outer` after it does not see what `Host inner` set.
        let top = "Include inc\nHost outer\n  User outer-user\n";
        assert_eq!(
            f.resolve(top, "inner").expect("resolves").user.as_deref(),
            Some("inner-user")
        );
        assert_eq!(
            f.resolve(top, "outer").expect("resolves").user.as_deref(),
            Some("outer-user")
        );
    }

    #[test]
    fn lines_after_an_include_come_after_what_the_file_set() {
        // First value wins in the order the lines are read, an included file's lines
        // included: the file's `Host *` block is read before the `User` below the
        // Include, although the block that holds that `User` started earlier.
        let f = Fixture::new();
        f.write(".ssh/inc", "Host *\n  User from-file\n");
        let text = "Host a\n  Include inc\n  User later\n";
        assert_eq!(
            f.resolve(text, "a").expect("resolves").user.as_deref(),
            Some("from-file")
        );
    }

    #[test]
    fn include_cycles_and_depth_are_bounded() {
        let f = Fixture::new();
        f.write(".ssh/loop", "Include loop\n");
        assert!(matches!(
            f.load("Include loop\n"),
            Err(SshConfigError::TooManyIncludes)
        ));
        // A chain one past the limit.
        for depth in 0..=MAX_DEPTH + 1 {
            let next = format!("Include chain{}\n", depth + 1);
            f.write(&format!(".ssh/chain{depth}"), &next);
        }
        assert!(matches!(
            f.load("Include chain0\n"),
            Err(SshConfigError::TooManyIncludes)
        ));
        // Too many files, even without nesting.
        for index in 0..=MAX_FILES {
            f.write(&format!(".ssh/many/{index:02}"), "# nothing\n");
        }
        assert!(matches!(
            f.load("Include many/*\n"),
            Err(SshConfigError::TooManyIncludes)
        ));
    }

    #[test]
    fn malformed_lines_and_values_are_errors_with_a_line_number() {
        let f = Fixture::new();
        for text in [
            "Host prod\n  Port nope\n",
            "Host prod\n  Port 70000\n",
            "Host prod\n  HostName \"unterminated\n",
            "Host\n",
            "Host prod\n  User a b\n",
            "=value\n",
        ] {
            let error = f.resolve(text, "prod").expect_err(text);
            assert!(
                matches!(error, SshConfigError::Malformed { .. }),
                "{text:?} gave {error}"
            );
        }
    }

    #[test]
    fn limits_on_size_and_alias_count() {
        let f = Fixture::new();
        let big = "#".repeat(usize::try_from(MAX_FILE_BYTES).expect("fits") + 1);
        assert!(matches!(f.load(&big), Err(SshConfigError::TooLarge { .. })));
        let many_lines = "# x\n".repeat(MAX_LINES + 1);
        assert!(matches!(
            f.load(&many_lines),
            Err(SshConfigError::TooLarge { .. })
        ));
        let hosts: String = (0..MAX_ALIASES + 10)
            .map(|index| format!("Host h{index}\n"))
            .collect();
        assert_eq!(f.load(&hosts).expect("loads").aliases().len(), MAX_ALIASES);
    }

    #[test]
    fn aliases_leave_out_patterns_and_repeats() {
        let f = Fixture::new();
        let config = f
            .load("Host a b *.x !c\nHost a\nHost d? e\nHost *\n")
            .expect("loads");
        assert_eq!(config.aliases(), ["a", "b", "e"]);
    }

    #[test]
    fn many_stars_do_not_blow_up() {
        let f = Fixture::new();
        let text = format!("Host {}b\n  User x\n", "*a".repeat(40));
        let started = std::time::Instant::now();
        assert!(matches!(
            f.resolve(&text, &"a".repeat(200)),
            Err(SshConfigError::NotFound { .. })
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_an_empty_config() {
        use std::os::unix::fs::PermissionsExt as _;
        let f = Fixture::new();
        let path = f.write(".ssh/config", "Host a\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let result = load_in(&path, Some(f.home()));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        // Root can read anything; the assertion only means something for a normal user.
        if rustix::process::geteuid().as_raw() != 0 {
            assert!(matches!(result, Err(SshConfigError::Unreadable { .. })));
        }
    }
}
