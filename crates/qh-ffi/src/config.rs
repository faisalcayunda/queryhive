//! Turning settings into a connection.
//!
//! This is what was `DatabaseConfig.from_env` (`exporter/drivers.py:780`, since deleted)
//! with its rules
//! kept, because they are the ones the app already sends: **`DB_*` wins over its
//! older `TRINO_*` alias**, a blank value means "unset", and a URL is optional —
//! the app sends the parts only, so the parts alone have to build a working
//! connection.
//!
//! # Where the TLS decision lives
//!
//! The Python engine handed `http_scheme` to the trino client and the `sslmode`
//! string to psycopg and pymysql, and each library decided what to do with it. The
//! Rust drivers take a [`TlsMode`] instead, so this module is where that decision
//! now happens, once — in libpq's vocabulary, for all three engines, so that one
//! spelling means one thing:
//!
//! | Setting | Trino | PostgreSQL / MySQL |
//! |---|---|---|
//! | `sslmode=disable` | [`TlsMode::Disable`] | [`TlsMode::Disable`] |
//! | `sslmode=prefer` | [`TlsMode::Prefer`] | [`TlsMode::Prefer`] |
//! | `sslmode=require` | [`TlsMode::RequireNoVerify`] | [`TlsMode::RequireNoVerify`] |
//! | `sslmode=verify-ca`/`verify-full` | [`TlsMode::Require`] | [`TlsMode::Require`] |
//! | unset | the scheme: `http` → `Disable`, `https` → `Require` | [`TlsMode::Prefer`] |
//!
//! `require` landing on [`TlsMode::RequireNoVerify`] is libpq's meaning and not
//! this enum's name: libpq's `require` encrypted **without** checking a
//! certificate, and only `verify-ca`/`verify-full` asked for one to be checked.
//! `require` must not mean "verify" for one engine and "do not verify" for
//! another, so it means "do not verify" everywhere.
//!
//! **Trino is the one engine with two settings for one decision.** The app stores a
//! scheme (`http`/`https`) with a `verify` flag beside it, because that is what its
//! picker offers, and sends `DB_SCHEME` + `DB_INSECURE`; the command line has
//! `sslmode` like the other two. When both are set, **`sslmode` wins**: it names the
//! mode outright, where a scheme can only spell two of the four. So
//! `DB_SCHEME=https DB_SSLMODE=disable` is a plaintext connection and
//! `DB_SCHEME=http DB_SSLMODE=require` is an encrypted one. The one mode no scheme
//! and no app field can ask for is [`TlsMode::Prefer`]; `DB_SSLMODE=prefer` (or a
//! `?sslmode=prefer` in a Trino URL) is the only spelling that reaches it.
//!
//! # Three rules that raise the mode and never lower it
//!
//! 1. **A password implies TLS on Trino.** Trino refuses BasicAuth over plaintext,
//!    so a password (or a port of 443/8443) promotes a plaintext mode to
//!    [`TlsMode::Require`], and a password with neither a port nor a URL moves the
//!    port to 443. Removing this would produce a connection that cannot
//!    authenticate. Note what it means for `DB_SSLMODE=disable`: a password outranks
//!    it, because the coordinator would refuse the connection anyway and refusing it
//!    here would be the same answer with a worse message.
//! 2. **`DB_INSECURE` (or `TRINO_INSECURE`) turned on downgrades a required TLS
//!    connection to [`TlsMode::RequireNoVerify`]** — it never turns TLS *off*, and it
//!    never lowers a mode to plaintext. The same reading the Python engine's
//!    `verify=False` had.
//! 3. **Port 0 means "the driver's default"**, never "port zero". The default
//!    follows the scheme the mode *starts* on: 8080 for [`TlsMode::Disable`], 443 for
//!    the other three. [`TlsMode::Prefer`] gets 443 for that reason, and a downgrade
//!    to http keeps it — the driver's downgrade changes the scheme only.

use thiserror::Error;

use qh_driver::{CaError, ConnectionConfig, DriverKind, TlsCa, TlsMode};

use crate::env::{parse_flag, SettingError, Settings};
use crate::tunnel::{self, TunnelConfigError};

/// The `DB_*` name, then the older `TRINO_*` name it supersedes.
///
/// Both are read because the app's own settings table moved from `TRINO_*` to
/// `DB_*` when the second and third drivers arrived, and a stored connection from
/// before that must keep working.
pub const ALIASES: &[(&str, &str)] = &[
    ("DB_URL", "TRINO_URL"),
    ("DB_HOST", "TRINO_HOST"),
    ("DB_PORT", "TRINO_PORT"),
    ("DB_USER", "TRINO_USER"),
    ("DB_PASSWORD", "TRINO_PASSWORD"),
    ("DB_DATABASE", "TRINO_CATALOG"),
    ("DB_SCHEMA", "TRINO_SCHEMA"),
    ("DB_INSECURE", "TRINO_INSECURE"),
];

/// Every spelling of one setting, `DB_*` first.
pub fn setting_names(key: &str) -> (&str, Option<&str>) {
    match ALIASES.iter().find(|(modern, _)| *modern == key) {
        Some((modern, alias)) => (modern, Some(alias)),
        None => (key, None),
    }
}

/// Every spelling of one setting, for a message a user has to act on.
pub fn setting_label(key: &str) -> String {
    match setting_names(key) {
        (modern, Some(alias)) => format!("{modern} (or {alias})"),
        (modern, None) => modern.to_owned(),
    }
}

/// The kinds this engine can connect to, in the order the usage and error messages
/// list them.
pub const KINDS: [&str; 3] = ["trino", "postgres", "mysql"];

/// Why a connection could not be built from the settings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("unknown {key} '{kind}'; expected one of trino, postgres, mysql")]
    UnknownKind { key: String, kind: String },

    #[error("unknown kind '{0}'; expected one of trino, postgres, mysql")]
    UnknownUrlKind(String),

    #[error("need DB_URL or DB_HOST (or TRINO_URL / TRINO_HOST)")]
    NoHost,

    #[error("DB_SCHEME must be http or https, got '{0}'")]
    BadScheme(String),

    #[error("unknown sslmode '{0}'; expected disable, prefer, require, verify-ca or verify-full")]
    BadSSLMode(String),

    #[error("cannot read a host out of '{0}'")]
    NoUrlHost(String),

    #[error(transparent)]
    Setting(#[from] SettingError),

    /// A combination of settings the engine refuses by name, before the network is touched.
    /// The message never repeats a credential.
    #[error("{0}")]
    Usage(String),

    /// `DB_CA_FILE` named a file that cannot serve as a CA bundle.
    #[error(transparent)]
    Ca(#[from] CaError),

    /// The `SSH_*` settings described no usable bastion. Kept beside the other
    /// config failures rather than raised from `connect`, because they are decided
    /// by reading settings, before the network is touched — the same rule as the
    /// variants above.
    #[error(transparent)]
    Tunnel(#[from] TunnelConfigError),
}

/// Resolve the connection described by `settings`.
pub fn build(settings: &Settings) -> Result<ConnectionConfig, ConfigError> {
    let (kind_key, kind_name) = pick(settings, "DB_KIND");
    // Whether the caller named a driver, as opposed to the trino default. It
    // matters for one thing only: a URL whose scheme names a driver. When
    // `DB_KIND` says nothing, the scheme decides; when it says `postgres`, it
    // decides, scheme or no scheme.
    let declared_kind = !kind_name.is_empty();
    let kind_name = if declared_kind {
        kind_name.to_ascii_lowercase()
    } else {
        "trino".to_owned()
    };
    let kind = DriverKind::parse(&kind_name).ok_or_else(|| ConfigError::UnknownKind {
        key: kind_key,
        kind: kind_name.clone(),
    })?;

    let (_, url) = pick(settings, "DB_URL");

    // Each part is an override: `None` when the caller did not set it, so a URL's
    // own value survives. A blank is an unset, which is why `_pick` and not `get`.
    let mut parts = Parts {
        kind,
        scheme: pick(settings, "DB_SCHEME").1.to_ascii_lowercase(),
        sslmode: pick(settings, "DB_SSLMODE").1.to_ascii_lowercase(),
        host: pick(settings, "DB_HOST").1,
        user: pick(settings, "DB_USER").1,
        password: optional(pick(settings, "DB_PASSWORD").1),
        database: pick(settings, "DB_DATABASE").1,
        schema: pick(settings, "DB_SCHEMA").1,
        port: None,
    };

    let (port_key, port_raw) = pick(settings, "DB_PORT");
    parts.port = if port_raw.is_empty() {
        None
    } else {
        Some(
            port_raw
                .parse::<i64>()
                .map_err(|_| SettingError::NotANumber {
                    key: port_key,
                    value: port_raw,
                })?,
        )
    };

    let insecure = parse_flag(&pick(settings, "DB_INSECURE").1, false);

    let mut resolved = if !url.is_empty() {
        from_url(&url, parts, declared_kind)?
    } else if !parts.host.is_empty() {
        parts
    } else {
        return Err(ConfigError::NoHost);
    };

    // The user falls back to the login name, then to `trino` for Trino alone: a
    // coordinator rejects an anonymous statement with a message that is harder to
    // act on than a name it will accept.
    if resolved.user.is_empty() {
        resolved.user = settings.text("USER", "");
        if resolved.user.is_empty() && resolved.kind == DriverKind::Trino {
            resolved.user = "trino".to_owned();
        }
    }

    // A password with neither a port nor a URL: Trino is about to be spoken to over
    // TLS whatever the scheme said, so the default port moves with it.
    if resolved.kind == DriverKind::Trino && resolved.password.is_some() && resolved.port.is_none()
    {
        resolved.port = Some(443);
    }

    let bearer = bearer(settings, &resolved)?;
    let mut config = resolved.into_config(insecure, bearer.is_some())?;
    config.bearer = bearer;
    // The bastion rides on the config it tunnells: the engine opens the tunnel as
    // part of `connect` (blueprint §3.2), so the description has to arrive where the
    // connect happens. `SSH_HOST` absent leaves this `None` and nothing changes.
    config.tunnel = tunnel::settings(settings)?;
    apply_ca_file(settings, &mut config)?;

    // Through a tunnel the name checked would be `127.0.0.1` (`tunnel::retarget` hands the
    // other two drivers the database's own name; `mysql_async` cannot take one). Refused here,
    // by name, because the handshake failure it would be otherwise explains nothing.
    if config.tunnel.is_some() && config.kind == DriverKind::Mysql && config.tls == TlsMode::Require
    {
        return Err(ConfigError::Usage(
            "MySQL cannot verify a server certificate through an SSH tunnel (the name checked \
             would be 127.0.0.1). Use require (encrypted, not verified: the tunnel already \
             authenticates the bastion), or connect directly"
                .to_owned(),
        ));
    }
    Ok(config)
}

/// The longest bearer token accepted: far past any real JWT, short of a pasted file.
const MAX_BEARER_CHARS: usize = 8192;

/// `DB_JWT`: a Trino bearer token (blueprint W11 §7.1).
///
/// The engine treats it as opaque: it is not decoded or verified, and an expired one shows up as
/// the coordinator's 401. Only its shape is checked, and a refusal never repeats it.
fn bearer(settings: &Settings, parts: &Parts) -> Result<Option<String>, ConfigError> {
    let token = settings.raw("DB_JWT", "").trim().to_owned();
    if token.is_empty() {
        return Ok(None);
    }
    if parts.kind != DriverKind::Trino {
        return Err(ConfigError::Usage(format!(
            "DB_JWT is only for Trino, and this connection is {}; send DB_PASSWORD instead",
            parts.kind.as_str()
        )));
    }
    if parts.password.is_some() {
        return Err(ConfigError::Usage(
            "send either DB_JWT or a password (DB_PASSWORD, or one inside DB_URL), not both"
                .to_owned(),
        ));
    }
    let token68 = |c: char| c.is_ascii_alphanumeric() || "-._~+/=".contains(c);
    if token.chars().count() > MAX_BEARER_CHARS || !token.chars().all(token68) {
        return Err(ConfigError::Usage(
            "DB_JWT is not a valid bearer token".to_owned(),
        ));
    }
    Ok(Some(token))
}

/// `DB_CA_FILE`: a CA bundle this connection trusts instead of the platform store (W11 §7.2).
///
/// Read and validated once, here, so the bytes that are checked are the bytes that are trusted.
fn apply_ca_file(settings: &Settings, config: &mut ConnectionConfig) -> Result<(), ConfigError> {
    let named = settings.text("DB_CA_FILE", "");
    if named.is_empty() {
        return Ok(());
    }
    if config.kind == DriverKind::Mysql {
        return Err(ConfigError::Usage(
            "DB_CA_FILE is not supported for MySQL: mysql_async 0.36 cannot take a CA \
             (docs/tls-modes.md)"
                .to_owned(),
        ));
    }
    let path = match named.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                ConfigError::Usage("DB_CA_FILE starts with ~ and $HOME is not set".to_owned())
            })?;
            std::path::PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => std::path::PathBuf::from(named),
    };
    config.tls_ca = Some(TlsCa::load(&path)?);
    // The driver checks this too; asking here puts the refusal in front of the network and
    // under the name of the setting that caused it.
    config
        .ca_for_verifying_mode()
        .map_err(|error| ConfigError::Usage(error.message().to_owned()))?;
    Ok(())
}

/// The connection as its individual parts, before the TLS and scheme rules run.
struct Parts {
    kind: DriverKind,
    host: String,
    /// `None` means "the driver's default port", never port zero.
    port: Option<i64>,
    user: String,
    password: Option<String>,
    database: String,
    schema: String,
    scheme: String,
    sslmode: String,
}

impl Parts {
    /// Apply the scheme/port rules and produce the config the drivers receive.
    fn into_config(self, insecure: bool, bearer: bool) -> Result<ConnectionConfig, ConfigError> {
        let mut port = self.port.unwrap_or(0);
        // Assigned once in each branch below: the two families of server do not share
        // a spelling for this, so a default here would only be a value to overwrite.
        let tls: TlsMode;

        if self.kind == DriverKind::Trino {
            // Validated whatever else was set: a scheme that is neither spelling is
            // refused rather than ignored because `sslmode` happened to win, so a
            // typo never passes unnoticed.
            if !self.scheme.is_empty() && self.scheme != "http" && self.scheme != "https" {
                return Err(ConfigError::BadScheme(self.scheme));
            }
            // libpq's vocabulary, with the same meanings as the other branch below:
            // `require` encrypts and does not verify, `verify-ca`/`verify-full` do.
            // A spelling nobody recognises is refused rather than treated as a
            // default, for the same reason there.
            let named = match self.sslmode.as_str() {
                "" => None,
                "disable" => Some(TlsMode::Disable),
                "prefer" => Some(TlsMode::Prefer),
                "require" => Some(TlsMode::RequireNoVerify),
                "verify-ca" | "verify-full" => Some(TlsMode::Require),
                other => return Err(ConfigError::BadSSLMode(other.to_owned())),
            };
            // A bearer token that could travel in clear is a leaked token, so a mode the caller
            // named that can end on `http` is refused by name rather than raised: `Prefer`
            // may fall back to it, and `disable` and an `http` scheme are it. Raising an
            // explicit choice behind the caller's back is what the password rule below does,
            // and it is not extended to a credential this much more portable.
            if bearer {
                let plain = match (named, self.scheme.as_str()) {
                    (Some(TlsMode::Disable), _) | (None, "http") => Some("plain http"),
                    (Some(TlsMode::Prefer), _) => Some("prefer, which may fall back to plain http"),
                    _ => None,
                };
                if let Some(plain) = plain {
                    return Err(ConfigError::Usage(format!(
                        "DB_JWT is only sent over TLS, and this connection is {plain}; use \
                         https, or sslmode require, verify-ca or verify-full"
                    )));
                }
            }
            // `sslmode` wins where it named a mode; otherwise the scheme does, and
            // `http` is what an absent one has always meant, except for a bearer token with
            // neither named: that is the password rule's reading, TLS on the HTTPS port.
            let mut mode = match named {
                Some(mode) => mode,
                None if self.scheme == "https" => TlsMode::Require,
                None if bearer && self.scheme.is_empty() => TlsMode::Require,
                None => TlsMode::Disable,
            };
            // The two rules from the module docs: a password or a standard HTTPS port
            // needs TLS on a coordinator that refuses BasicAuth in clear. They raise the
            // mode, and they raise it even over an explicit `sslmode=disable` -- which is
            // the Python engine's rule, from its own source (`exporter/drivers.py`: "a
            // password implies TLS, as do the standard HTTPS ports"). Honouring `disable`
            // here would send the password in clear to a coordinator that refuses it, so
            // the user would get a failure about their credentials instead of the
            // encryption they asked for. `DB_INSECURE` is the other half of the pair: it
            // lowers verification and never turns TLS off.
            if mode == TlsMode::Disable && (self.password.is_some() || port == 443 || port == 8443)
            {
                mode = TlsMode::Require;
            }
            // `DB_INSECURE` drops the certificate check; it never turns TLS off.
            if insecure && mode != TlsMode::Disable {
                mode = TlsMode::RequireNoVerify;
            }
            tls = mode;
            if port == 0 {
                port = if tls == TlsMode::Disable { 8080 } else { 443 };
            }
        } else {
            // The stored vocabulary is libpq's, and it does not mean what our enum's names
            // suggest: libpq's `require` encrypts **without** verifying, and only
            // `verify-ca`/`verify-full` ask for the certificate to be checked. Mapping
            // `require` onto our verifying mode would refuse every internal, self-signed
            // server that works today, which is the one upgrade nobody forgives.
            let required = match self.sslmode.as_str() {
                "disable" => TlsMode::Disable,
                "require" => TlsMode::RequireNoVerify,
                "verify-ca" | "verify-full" => TlsMode::Require,
                // psycopg's own default, and what a driver does when nobody said anything:
                // encrypt when the server offers it.
                "" | "prefer" => TlsMode::Prefer,
                // A spelling nobody recognises is refused rather than treated as a default:
                // a typo in `sslmode` deciding whether the connection is encrypted is not a
                // decision to make silently.
                other => return Err(ConfigError::BadSSLMode(other.to_owned())),
            };
            tls = if insecure && required != TlsMode::Disable {
                TlsMode::RequireNoVerify
            } else {
                required
            };
        }

        let mut config = ConnectionConfig::new(self.kind, self.host, 0, self.user);
        config.port = u16::try_from(port).unwrap_or(0);
        config.password = self.password;
        config.database = optional(self.database);
        config.schema = optional(self.schema);
        config.tls = tls;
        config.insecure = insecure;
        Ok(config)
    }
}

/// A blank part is absent, not empty: `"?schema="` and no schema at all behave
/// alike, which is what the Python engine's `v not in (None, "")` meant.
fn optional(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// `(the name that supplied this setting, its value)`; `DB_*` wins over the alias.
///
/// The name comes back so a validation error can quote the variable the caller
/// actually set instead of one they never used.
fn pick(settings: &Settings, key: &str) -> (String, String) {
    let (modern, alias) = setting_names(key);
    for name in [Some(modern), alias].into_iter().flatten() {
        let value = settings.text(name, "");
        if !value.is_empty() {
            return (name.to_owned(), value);
        }
    }
    (modern.to_owned(), String::new())
}

/// Build from a connection URL, with the `DB_*` parts winning over its own.
///
/// ```text
/// trino://user:secret@trino.internal:8443/hive/analytics   catalog, schema
/// postgresql://user:secret@pg.internal/appdb?sslmode=require
/// mysql://user:secret@mysql.internal:3306/shop
/// trino://coordinator:8080?sslmode=require                 no path, query kept
/// ```
///
/// A URL with no `//` is read as host-first, which is how a caller who pasted
/// `host:8080/hive` expects it to behave.
fn from_url(url: &str, overrides: Parts, declared_kind: bool) -> Result<Parts, ConfigError> {
    let parsed = Parsed::parse(url);

    let scheme_kind = match parsed.scheme.as_str() {
        "trino" | "http" | "https" => DriverKind::Trino,
        "postgres" | "postgresql" => DriverKind::Postgres,
        "mysql" => DriverKind::Mysql,
        // Not a driver name: an unknown scheme is not a reason to refuse the URL,
        // because a URL without one is read as host-first and lands here too.
        _ => DriverKind::Trino,
    };
    // `DB_KIND` wins when it named a driver; otherwise the scheme does. The Python
    // engine let its own trino default shadow the scheme here, so
    // `DB_URL=postgresql://…` with no `DB_KIND` connected to Trino — the app always
    // sends both, so the difference only shows up on the command line, where
    // reading the scheme is what the caller meant.
    let kind = if declared_kind {
        overrides.kind
    } else {
        scheme_kind
    };
    if parsed.host.is_empty() {
        return Err(ConfigError::NoUrlHost(url.to_owned()));
    }

    let mut base = Parts {
        kind,
        host: parsed.host.clone(),
        port: None,
        user: parsed.user.clone(),
        password: parsed.password.clone(),
        database: String::new(),
        schema: String::new(),
        scheme: String::new(),
        sslmode: String::new(),
    };

    if kind == DriverKind::Trino {
        let scheme = if parsed.scheme == "https" {
            "https".to_owned()
        } else {
            "http".to_owned()
        };
        base.scheme = scheme.clone();
        base.port = Some(
            parsed
                .port
                .unwrap_or(if scheme == "https" { 443 } else { 8080 }),
        );
        base.database = parsed.segment(0);
        base.schema = parsed.segment(1);
        // A Trino URL may carry the same `sslmode` the other two read from theirs, and
        // it means the same thing here: it outranks the `http`/`https` the URL's own
        // scheme spells, and `DB_SSLMODE` outranks both (see `overridden_by`).
        base.sslmode = parsed.query("sslmode");
    } else {
        base.port = Some(parsed.port.unwrap_or(0));
        base.database = parsed.segment(0);
        base.sslmode = parsed.query("sslmode");
    }

    Ok(base.overridden_by(overrides))
}

impl Parts {
    /// Every part the caller set replaces the URL's own.
    fn overridden_by(self, overrides: Parts) -> Parts {
        Parts {
            // The driver is already settled above, by `DB_KIND` or the scheme;
            // there is nothing left to override it with here.
            kind: self.kind,
            host: or(self.host, overrides.host),
            port: overrides.port.or(self.port),
            user: or(self.user, overrides.user),
            password: overrides.password.or(self.password),
            database: or(self.database, overrides.database),
            schema: or(self.schema, overrides.schema),
            scheme: or(self.scheme, overrides.scheme),
            sslmode: or(self.sslmode, overrides.sslmode),
        }
    }
}

fn or(base: String, override_value: String) -> String {
    if override_value.is_empty() {
        base
    } else {
        override_value
    }
}

/// The parts of a connection URL.
struct Parsed {
    scheme: String,
    host: String,
    port: Option<i64>,
    user: String,
    password: Option<String>,
    segments: Vec<String>,
    query: Vec<(String, String)>,
}

impl Parsed {
    fn parse(url: &str) -> Self {
        // `urlparse(url if "//" in url else f"//{url}", scheme="http")`: a URL with
        // no scheme of its own is read as host-first against `http`, rather than
        // as a scheme whose remainder cannot be parsed.
        let (scheme, rest) = match url.split_once("://") {
            Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest.to_owned()),
            None => ("http".to_owned(), url.trim_start_matches("//").to_owned()),
        };

        // Authority, then path, then query — `urlparse`'s own order, and the
        // authority ends at the first `/`, `?` or `#` rather than at the first `/`
        // alone. Splitting it only on `/` swallowed the query of a URL that has no
        // path: `trino://host:8080?sslmode=require` came out as the host
        // `host:8080?sslmode=require` with no port and no query, so the mode it
        // names was silently dropped and the connection pointed at a name no DNS
        // has ever resolved. `host:8080?sslmode=require` now splits exactly like
        // the shape the docs give, `host:8080/catalog?sslmode=require`.
        let (authority, tail) = match rest.find(['/', '?', '#']) {
            Some(index) => (rest[..index].to_owned(), rest[index..].to_owned()),
            None => (rest.clone(), String::new()),
        };
        // The fragment goes before either is read, for the same reason: `urlparse`
        // keeps it out of the path, so `host/catalog#frag` is a catalog named
        // `catalog` and not one named `catalog#frag`.
        let tail = match tail.split_once('#') {
            Some((before, _)) => before.to_owned(),
            None => tail,
        };
        let (path, query) = match tail.split_once('?') {
            Some((path, query)) => (path.to_owned(), query.to_owned()),
            None => (tail, String::new()),
        };

        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((userinfo, hostport)) => (Some(userinfo.to_owned()), hostport.to_owned()),
            None => (None, authority),
        };

        let (user, password) = match userinfo.as_deref() {
            Some(userinfo) => match userinfo.split_once(':') {
                Some((user, password)) => (percent_decode(user), Some(percent_decode(password))),
                None => (percent_decode(userinfo), None),
            },
            None => (String::new(), None),
        };

        // A bracketed IPv6 literal keeps its colons; anything else splits on the
        // last colon so `host:5432` works and a bare host does not become a port.
        let (host, port) = if let Some(end) = hostport.rfind(']') {
            match hostport[end + 1..].strip_prefix(':') {
                Some(port) => (hostport[..=end].to_owned(), port.parse::<i64>().ok()),
                None => (hostport.clone(), None),
            }
        } else {
            match hostport.rsplit_once(':') {
                Some((host, port))
                    if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
                {
                    (host.to_owned(), port.parse::<i64>().ok())
                }
                _ => (hostport, None),
            }
        };

        let segments = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(percent_decode)
            .collect();
        let query = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (percent_decode(key), percent_decode(value)),
                None => (percent_decode(pair), String::new()),
            })
            .collect();

        Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            user,
            password,
            segments,
            query,
        }
    }

    fn segment(&self, index: usize) -> String {
        self.segments.get(index).cloned().unwrap_or_default()
    }

    fn query(&self, key: &str) -> String {
        self.query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

/// Percent-decoding, as `urllib.parse.unquote` does it: a `%` that is not followed
/// by two hex digits is left as written rather than dropped.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
    }

    /// The connection these settings describe; every case below names a host, so a
    /// failure here is about TLS and not about a missing one.
    fn config(pairs: &[(&str, &str)]) -> ConnectionConfig {
        build(&settings(pairs)).expect("these settings describe a connection")
    }

    /// The same, for a Trino connection: the host and the driver are already there,
    /// so each case reads as the one or two settings it is about.
    fn trino(pairs: &[(&str, &str)]) -> ConnectionConfig {
        let mut all = vec![("DB_KIND", "trino"), ("DB_HOST", "coordinator")];
        all.extend_from_slice(pairs);
        config(&all)
    }

    /// The mode the settings land on, which is what most of these are about.
    fn tls_config(pairs: &[(&str, &str)]) -> TlsMode {
        trino(pairs).tls
    }

    #[test]
    fn trino_reads_sslmode_in_libpq_vocabulary() {
        // The four modes the settings could not all express before. `require` is the
        // interesting one: encrypt, do not verify — libpq's meaning, and the same one
        // the postgres branch gives that spelling.
        for (spelling, expected) in [
            ("disable", TlsMode::Disable),
            ("prefer", TlsMode::Prefer),
            ("require", TlsMode::RequireNoVerify),
            ("verify-ca", TlsMode::Require),
            ("verify-full", TlsMode::Require),
        ] {
            assert_eq!(
                tls_config(&[("DB_SSLMODE", spelling)]),
                expected,
                "{spelling}"
            );
        }
    }

    #[test]
    fn an_unrecognised_sslmode_is_refused_for_trino_too() {
        // Refused, not treated as a default: a typo in `sslmode` deciding whether the
        // connection is encrypted is not a decision to make silently.
        let error = build(&settings(&[
            ("DB_KIND", "trino"),
            ("DB_HOST", "coordinator"),
            ("DB_SSLMODE", "tls"),
        ]))
        .unwrap_err();
        assert_eq!(error, ConfigError::BadSSLMode("tls".to_owned()));
        assert_eq!(
            error.to_string(),
            "unknown sslmode 'tls'; expected disable, prefer, require, verify-ca or verify-full"
        );
    }

    #[test]
    fn a_scheme_that_is_neither_spelling_is_refused_even_when_sslmode_wins() {
        // `sslmode` outranks the scheme, but the scheme is still read, so a typo in it
        // cannot ride along unnoticed behind a valid `sslmode`.
        let error = build(&settings(&[
            ("DB_KIND", "trino"),
            ("DB_HOST", "coordinator"),
            ("DB_SCHEME", "ftp"),
            ("DB_SSLMODE", "require"),
        ]))
        .unwrap_err();
        assert_eq!(error, ConfigError::BadScheme("ftp".to_owned()));
    }

    #[test]
    fn sslmode_outranks_the_scheme_for_trino() {
        assert_eq!(
            tls_config(&[("DB_SCHEME", "https"), ("DB_SSLMODE", "disable")]),
            TlsMode::Disable
        );
        assert_eq!(
            tls_config(&[("DB_SCHEME", "http"), ("DB_SSLMODE", "require")]),
            TlsMode::RequireNoVerify
        );
        assert_eq!(
            tls_config(&[("DB_SCHEME", "http"), ("DB_SSLMODE", "verify-full")]),
            TlsMode::Require
        );
    }

    #[test]
    fn the_scheme_still_decides_when_no_sslmode_was_given() {
        // What the app sends, and what every stored Trino connection sends until the
        // picker grows a mode: a scheme and nothing else.
        assert_eq!(tls_config(&[("DB_SCHEME", "http")]), TlsMode::Disable);
        assert_eq!(tls_config(&[("DB_SCHEME", "https")]), TlsMode::Require);
        // An absent scheme has always meant http.
        assert_eq!(tls_config(&[]), TlsMode::Disable);
    }

    #[test]
    fn a_password_still_implies_tls() {
        let with_password = trino(&[("DB_SCHEME", "http"), ("DB_PASSWORD", "secret")]);
        assert_eq!(with_password.tls, TlsMode::Require);
        // A password with no port of its own moves it to the HTTPS one.
        assert_eq!(with_password.port, 443);
        // Spelled out too: the coordinator would refuse BasicAuth over plaintext, so
        // `disable` with a password is raised rather than honoured.
        assert_eq!(
            tls_config(&[("DB_SSLMODE", "disable"), ("DB_PASSWORD", "secret")]),
            TlsMode::Require
        );
    }

    #[test]
    fn insecure_never_turns_tls_off() {
        assert_eq!(
            tls_config(&[("DB_SCHEME", "http"), ("DB_INSECURE", "1")]),
            TlsMode::Disable
        );
        assert_eq!(
            tls_config(&[("DB_SCHEME", "https"), ("DB_INSECURE", "1")]),
            TlsMode::RequireNoVerify
        );
        assert_eq!(
            tls_config(&[("DB_SSLMODE", "require"), ("DB_INSECURE", "1")]),
            TlsMode::RequireNoVerify
        );
        // `prefer` already does not verify; `DB_INSECURE` makes that explicit and gives
        // up the fallback, which is what it does for postgres as well.
        assert_eq!(
            tls_config(&[("DB_SSLMODE", "prefer"), ("DB_INSECURE", "1")]),
            TlsMode::RequireNoVerify
        );
    }

    #[test]
    fn the_https_ports_still_imply_tls() {
        for port in ["443", "8443"] {
            assert_eq!(
                tls_config(&[("DB_SCHEME", "http"), ("DB_PORT", port)]),
                TlsMode::Require,
                "{port}"
            );
        }
        // Unset scheme, an HTTPS port.
        assert_eq!(tls_config(&[("DB_PORT", "8443")]), TlsMode::Require);
    }

    #[test]
    fn the_default_port_follows_the_mode_the_connection_starts_on() {
        assert_eq!(trino(&[]).port, 8080);
        assert_eq!(trino(&[("DB_SSLMODE", "disable")]).port, 8080);
        assert_eq!(trino(&[("DB_SSLMODE", "prefer")]).port, 443);
        assert_eq!(trino(&[("DB_SSLMODE", "require")]).port, 443);
        assert_eq!(trino(&[("DB_SCHEME", "https")]).port, 443);
    }

    #[test]
    fn the_apps_scheme_and_verify_are_enough_for_three_of_the_four_modes() {
        // Everything `Connections.swift` can express, sent as `DB_SCHEME` +
        // `DB_INSECURE`. Nothing in the app reaches `Prefer`.
        assert_eq!(tls_config(&[("DB_SCHEME", "http")]), TlsMode::Disable);
        assert_eq!(tls_config(&[("DB_SCHEME", "https")]), TlsMode::Require);
        assert_eq!(
            tls_config(&[("DB_SCHEME", "https"), ("DB_INSECURE", "1")]),
            TlsMode::RequireNoVerify
        );
        // The fourth field combination, `http` with verify off, is the same connection
        // as `http` with verify on: in clear there is nothing to verify.
        assert_eq!(
            tls_config(&[("DB_SCHEME", "http"), ("DB_INSECURE", "1")]),
            TlsMode::Disable
        );
    }

    #[test]
    fn a_trino_url_may_carry_its_own_sslmode() {
        // The catalog segment is what the documented URL shape has.
        assert_eq!(
            config(&[("DB_URL", "trino://coordinator:8080/hive?sslmode=require")]).tls,
            TlsMode::RequireNoVerify
        );
        // The same URL with no path at all. It read as a host of
        // `coordinator:8080?sslmode=require` before the authority was split on the
        // query as well, so the mode the URL named was dropped and the connection
        // went to a host that does not exist.
        assert_eq!(
            config(&[("DB_URL", "trino://coordinator:8080?sslmode=require")]).tls,
            TlsMode::RequireNoVerify
        );
        // `DB_SSLMODE` outranks the URL's own, like every other `DB_*` part does.
        assert_eq!(
            config(&[
                (
                    "DB_URL",
                    "trino://coordinator:8080/hive?sslmode=verify-full"
                ),
                ("DB_SSLMODE", "disable"),
            ])
            .tls,
            TlsMode::Disable
        );
        assert_eq!(
            config(&[
                ("DB_URL", "trino://coordinator:8080?sslmode=verify-full"),
                ("DB_SSLMODE", "disable"),
            ])
            .tls,
            TlsMode::Disable
        );
    }

    #[test]
    fn one_spelling_means_one_thing_across_the_engines() {
        // The point of the table in the module docs: `require` must not mean "verify"
        // for postgres and "do not verify" for trino, or a connection moved between
        // the two would not be the connection the user read.
        for (spelling, expected) in [
            ("disable", TlsMode::Disable),
            ("prefer", TlsMode::Prefer),
            ("require", TlsMode::RequireNoVerify),
            ("verify-ca", TlsMode::Require),
            ("verify-full", TlsMode::Require),
        ] {
            assert_eq!(
                tls_config(&[("DB_SSLMODE", spelling)]),
                expected,
                "trino {spelling}"
            );
            assert_eq!(
                config(&[
                    ("DB_KIND", "postgres"),
                    ("DB_HOST", "db"),
                    ("DB_SSLMODE", spelling),
                ])
                .tls,
                expected,
                "postgres {spelling}"
            );
        }
    }

    #[test]
    fn an_absent_sslmode_keeps_each_engines_own_default() {
        // Trino's is the app's scheme — clear unless something says otherwise, because
        // that is what every stored connection has been doing. Postgres's is psycopg's
        // `prefer`, which is also the driver's own default.
        assert_eq!(tls_config(&[]), TlsMode::Disable);
        assert_eq!(tls_config(&[("DB_SCHEME", "https")]), TlsMode::Require);
        assert_eq!(
            config(&[("DB_KIND", "postgres"), ("DB_HOST", "db")]).tls,
            TlsMode::Prefer
        );
    }

    /// The connection a bare URL describes, with no `DB_*` part beside it.
    fn from(url: &str) -> ConnectionConfig {
        config(&[("DB_URL", url)])
    }

    #[test]
    fn a_url_with_no_path_parses_like_one_with_a_path() {
        // The defect this file carried: the authority was split on `/` alone, so a
        // URL with no path swallowed its query into the host —
        // `trino://host:8080?sslmode=require` came out as the host
        // `host:8080?sslmode=require`, `port=None`, `query=[]`. The query named a
        // TLS mode and the connection ignored it: a URL that asked for encryption
        // was opened in clear to a host no DNS resolves.
        let bare = from("trino://host:8080?sslmode=require");
        assert_eq!(bare.host, "host");
        assert_eq!(bare.port, 8080);
        assert_eq!(bare.tls, TlsMode::RequireNoVerify);

        // The path is the only thing the two shapes can differ in; every part they
        // share has to come out equal, which is what makes one parser safe here.
        let with_path = from("trino://host:8080/catalog?sslmode=require");
        assert_eq!(bare.host, with_path.host);
        assert_eq!(bare.port, with_path.port);
        assert_eq!(bare.tls, with_path.tls);
        assert_eq!(bare.database, None);
        assert_eq!(with_path.database.as_deref(), Some("catalog"));
    }

    #[test]
    fn a_no_path_url_with_credentials_splits_its_userinfo_too() {
        // Measured before the fix: host `host:8080?sslmode=require`, port `None`,
        // and the password rule promoted the mode to `Require` (verify) because the
        // URL's own `sslmode=require` had been lost — libpq's `require` is the
        // opposite: encrypt without verifying.
        let bare = from("trino://user:pass@host:8080?sslmode=require");
        assert_eq!(bare.host, "host");
        assert_eq!(bare.port, 8080);
        assert_eq!(bare.user, "user");
        assert_eq!(bare.password.as_deref(), Some("pass"));
        assert_eq!(bare.tls, TlsMode::RequireNoVerify);

        let with_path = from("trino://user:pass@host:8080/catalog?sslmode=require");
        assert_eq!(bare.host, with_path.host);
        assert_eq!(bare.port, with_path.port);
        assert_eq!(bare.user, with_path.user);
        assert_eq!(bare.tls, with_path.tls);
    }

    #[test]
    fn a_url_with_neither_a_path_nor_a_query_is_unchanged() {
        // The shape every stored URL that has no query already used, and the one the
        // fix must leave alone: the authority still ends where it did.
        let bare = from("trino://host:8080");
        assert_eq!(bare.host, "host");
        assert_eq!(bare.port, 8080);
        assert_eq!(bare.tls, TlsMode::Disable);
        assert_eq!(bare.database, None);
        assert_eq!(bare.schema, None);
    }

    #[test]
    fn a_fragment_is_not_part_of_the_host_nor_of_the_last_segment() {
        // `urlparse` never puts a fragment in the host or the path. Read off the code
        // before the fix, the old splitter kept it in both: `host/catalog#frag` named
        // its catalog `catalog#frag`, and `host:8080#frag` put the whole tail in the
        // host.
        let with_path = from("trino://host:8080/catalog/schema#frag");
        assert_eq!(with_path.host, "host");
        assert_eq!(with_path.port, 8080);
        assert_eq!(with_path.database.as_deref(), Some("catalog"));
        assert_eq!(with_path.schema.as_deref(), Some("schema"));

        let bare = from("trino://host:8080#frag");
        assert_eq!(bare.host, "host");
        assert_eq!(bare.port, 8080);
        assert_eq!(bare.database, None);
    }

    #[test]
    fn a_trailing_slash_parses_like_no_path_at_all() {
        assert_eq!(from("trino://host:8080/").host, "host");
        assert_eq!(from("trino://host:8080/").port, 8080);
        assert_eq!(from("trino://host:8080/").database, None);
        // And with a query behind it: an empty path still ends the authority.
        let trailing = from("trino://host:8080/?sslmode=require");
        assert_eq!(trailing.host, "host");
        assert_eq!(trailing.port, 8080);
        assert_eq!(trailing.tls, TlsMode::RequireNoVerify);
        assert_eq!(trailing.database, None);
    }

    #[test]
    fn an_ipv6_literal_keeps_its_brackets_and_still_finds_its_port() {
        // The parser does support a bracketed literal: the brackets are carried into
        // the host as written, where Python's `urlparse` would strip them — a
        // difference older than this fix and left alone here, because changing it
        // would move the host of every IPv6 connection.
        let with_path = from("trino://[::1]:8080/catalog?sslmode=require");
        assert_eq!(with_path.host, "[::1]");
        assert_eq!(with_path.port, 8080);
        assert_eq!(with_path.tls, TlsMode::RequireNoVerify);

        // The shape that used to lose both the port and the query: the colon after
        // the brackets was inside the swallowed query, so the port did not parse.
        let bare = from("trino://[::1]:8080?sslmode=require");
        assert_eq!(bare.host, "[::1]");
        assert_eq!(bare.port, 8080);
        assert_eq!(bare.tls, TlsMode::RequireNoVerify);

        // No port at all, which the brackets have to keep working for.
        let no_port = from("trino://[::1]?sslmode=require");
        assert_eq!(no_port.host, "[::1]");
        assert_eq!(no_port.port, 8080);
        assert_eq!(no_port.tls, TlsMode::RequireNoVerify);
    }

    #[test]
    fn an_absent_port_takes_each_engines_own_default() {
        // Trino's follows the scheme; postgres and mysql pass 0 through, which is
        // `ConnectionConfig`'s "the driver's default" and never port zero.
        assert_eq!(from("trino://host?sslmode=require").port, 8080);
        assert_eq!(from("https://host?sslmode=require").port, 443);
        assert_eq!(
            config(&[
                ("DB_KIND", "postgres"),
                ("DB_URL", "postgres://host?sslmode=require")
            ])
            .port,
            0
        );
        assert_eq!(
            config(&[
                ("DB_KIND", "mysql"),
                ("DB_URL", "mysql://host?sslmode=require")
            ])
            .port,
            0
        );
        // `DB_PORT` outranks the URL's own absence and its own port alike.
        assert_eq!(
            config(&[
                ("DB_URL", "trino://host?sslmode=require"),
                ("DB_PORT", "8443")
            ])
            .port,
            8443
        );
    }

    #[test]
    fn percent_encoded_credentials_are_decoded_in_every_shape() {
        let bare = from("trino://us%40er:p%3Ass@host:8080?sslmode=require");
        assert_eq!(bare.host, "host");
        assert_eq!(bare.user, "us@er");
        assert_eq!(bare.password.as_deref(), Some("p:ss"));
        // A percent-encoded catalog segment in the shape that has one.
        let with_path = from("trino://user@host:8080/h%69ve?sslmode=require");
        assert_eq!(with_path.database.as_deref(), Some("hive"));
    }

    #[test]
    fn an_empty_query_matches_no_query_and_a_valueless_one_matches_a_blank() {
        // The two shapes a hand-written splitter gets wrong: `?` with nothing behind
        // it, and one pair with no `=`. Both have to leave the connection exactly as
        // the URL with no query at all would.
        let none = from("trino://host:8080");
        for url in ["trino://host:8080?", "trino://host:8080?sslmode"] {
            let parsed = from(url);
            assert_eq!(parsed.host, none.host, "{url}");
            assert_eq!(parsed.port, none.port, "{url}");
            assert_eq!(parsed.tls, none.tls, "{url}");
        }
        // And the pair after a valueless one is still found.
        assert_eq!(
            from("trino://host:8080?x&sslmode=require").tls,
            TlsMode::RequireNoVerify
        );
    }

    #[test]
    fn the_other_two_engines_read_the_same_no_path_shape() {
        // One parser, three engines: the postgres and mysql branches read the query
        // from the same place, and their host is what the query used to be swallowed
        // into.
        let postgres = config(&[
            ("DB_KIND", "postgres"),
            (
                "DB_URL",
                "postgresql://user:pass@pg:5432/appdb?sslmode=require",
            ),
        ]);
        assert_eq!(postgres.host, "pg");
        assert_eq!(postgres.port, 5432);
        assert_eq!(postgres.user, "user");
        assert_eq!(postgres.database.as_deref(), Some("appdb"));
        assert_eq!(postgres.tls, TlsMode::RequireNoVerify);

        let postgres_bare = config(&[
            ("DB_KIND", "postgres"),
            ("DB_URL", "postgres://pg:5432?sslmode=require"),
        ]);
        assert_eq!(postgres_bare.host, "pg");
        assert_eq!(postgres_bare.port, 5432);
        assert_eq!(postgres_bare.tls, TlsMode::RequireNoVerify);

        let mysql = config(&[
            ("DB_KIND", "mysql"),
            ("DB_URL", "mysql://user:pass@db:3306/shop?sslmode=require"),
        ]);
        assert_eq!(mysql.host, "db");
        assert_eq!(mysql.port, 3306);
        assert_eq!(mysql.database.as_deref(), Some("shop"));
        assert_eq!(mysql.tls, TlsMode::RequireNoVerify);

        let mysql_bare = config(&[
            ("DB_KIND", "mysql"),
            ("DB_URL", "mysql://db:3306?sslmode=require"),
        ]);
        assert_eq!(mysql_bare.host, "db");
        assert_eq!(mysql_bare.port, 3306);
        assert_eq!(mysql_bare.tls, TlsMode::RequireNoVerify);
    }

    #[test]
    fn db_parts_still_outrank_every_shape_of_url() {
        // The ordering the module docs promise, checked against the shape that was
        // just repaired as well as the one that always worked.
        for url in [
            "trino://user:pass@host:8080/hive/analytics?sslmode=require",
            "trino://user:pass@host:8080?sslmode=require",
        ] {
            let overridden = config(&[
                ("DB_URL", url),
                ("DB_HOST", "other"),
                ("DB_PORT", "9000"),
                ("DB_USER", "someone"),
                ("DB_PASSWORD", "else"),
                ("DB_DATABASE", "othercatalog"),
                ("DB_SCHEMA", "otherschema"),
                // `verify-full` is the URL's `require` raised, and the only pair of
                // spellings the two branches cannot land on the same mode by accident.
                ("DB_SSLMODE", "verify-full"),
            ]);
            assert_eq!(overridden.host, "other", "{url}");
            assert_eq!(overridden.port, 9000, "{url}");
            assert_eq!(overridden.user, "someone", "{url}");
            assert_eq!(overridden.password.as_deref(), Some("else"), "{url}");
            assert_eq!(
                overridden.database.as_deref(),
                Some("othercatalog"),
                "{url}"
            );
            assert_eq!(overridden.schema.as_deref(), Some("otherschema"), "{url}");
            assert_eq!(overridden.tls, TlsMode::Require, "{url}");
        }

        // And the same parts reach a bare URL with no path and no query at all.
        let bare = config(&[
            ("DB_URL", "trino://host:8080"),
            ("DB_HOST", "other"),
            ("DB_PORT", "9000"),
            ("DB_DATABASE", "othercatalog"),
        ]);
        assert_eq!(bare.host, "other");
        assert_eq!(bare.port, 9000);
        assert_eq!(bare.database.as_deref(), Some("othercatalog"));
    }

    #[test]
    fn an_unrecognised_sslmode_in_a_urls_query_is_refused_too() {
        // The rule that a spelling nobody recognises is refused rather than treated
        // as a default, reached through a URL instead of a setting. It has to hold
        // for the shape with no path, which is where the query used to be invisible.
        for url in [
            "trino://host:8080/hive?sslmode=tls",
            "trino://host:8080?sslmode=tls",
        ] {
            assert_eq!(
                build(&settings(&[("DB_URL", url)])).unwrap_err(),
                ConfigError::BadSSLMode("tls".to_owned()),
                "{url}"
            );
        }
    }

    #[test]
    fn a_url_with_no_host_is_refused_rather_than_read_as_a_host() {
        // A blank authority is a `ConfigError::NoUrlHost`, never a host made out of
        // what followed it: `trino://?sslmode=require` used to read as a host of
        // `?sslmode=require`, because the query was never split off.
        assert_eq!(
            build(&settings(&[("DB_URL", "trino://?sslmode=require")])).unwrap_err(),
            ConfigError::NoUrlHost("trino://?sslmode=require".to_owned())
        );
        assert_eq!(
            build(&settings(&[("DB_URL", "trino:///hive")])).unwrap_err(),
            ConfigError::NoUrlHost("trino:///hive".to_owned())
        );
    }

    // ------------------------------------------------------------------- //
    // DB_JWT (blueprint W11 §7.1)
    // ------------------------------------------------------------------- //

    const JWT: &str = "eyJhbGciOi.SENTINEL-payload_x.sig";

    fn refusal(pairs: &[(&str, &str)]) -> String {
        match build(&settings(pairs)).expect_err("these settings must be refused") {
            ConfigError::Usage(message) => message,
            other => panic!("expected a usage refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_jwt_becomes_the_bearer_and_raises_the_mode_when_nothing_was_named() {
        // The app sends `DB_SCHEME` with its picker, so the bare-host command line is the case
        // that needs the rule: no scheme and no sslmode means TLS on the HTTPS port.
        let bare = trino(&[("DB_JWT", JWT)]);
        assert_eq!(bare.bearer.as_deref(), Some(JWT));
        assert_eq!((bare.tls, bare.port), (TlsMode::Require, 443));
        assert_eq!(bare.password, None);

        // https, and require without verification, are both TLS and both allowed.
        assert_eq!(
            trino(&[("DB_JWT", JWT), ("DB_SCHEME", "https")]).tls,
            TlsMode::Require
        );
        assert_eq!(
            trino(&[("DB_JWT", JWT), ("DB_SSLMODE", "require")]).tls,
            TlsMode::RequireNoVerify
        );
        assert_eq!(
            trino(&[
                ("DB_JWT", JWT),
                ("DB_SCHEME", "https"),
                ("DB_INSECURE", "1")
            ])
            .tls,
            TlsMode::RequireNoVerify
        );
        // Surrounding whitespace from a paste is not part of the token.
        assert_eq!(
            trino(&[("DB_JWT", &format!("  {JWT}\n"))])
                .bearer
                .as_deref(),
            Some(JWT)
        );
        // Blank is unset.
        assert_eq!(trino(&[("DB_JWT", "  ")]).bearer, None);
    }

    #[test]
    fn a_jwt_that_could_travel_in_clear_is_refused_by_the_mode_it_names() {
        let base = [
            ("DB_KIND", "trino"),
            ("DB_HOST", "coordinator"),
            ("DB_JWT", JWT),
        ];
        let with = |extra: (&str, &str)| refusal(&[base[0], base[1], base[2], extra]);
        assert!(with(("DB_SCHEME", "http")).contains("plain http"));
        assert!(with(("DB_SSLMODE", "disable")).contains("plain http"));
        assert!(with(("DB_SSLMODE", "prefer")).contains("prefer"));
        // The refusal does not repeat the token.
        for message in [with(("DB_SCHEME", "http")), with(("DB_SSLMODE", "prefer"))] {
            assert!(!message.contains("SENTINEL"), "{message}");
        }
        // A URL's own scheme counts as named.
        assert!(
            refusal(&[("DB_URL", "http://coordinator:8080"), ("DB_JWT", JWT)])
                .contains("plain http")
        );
    }

    #[test]
    fn a_jwt_is_refused_with_a_password_for_another_engine_and_when_malformed() {
        assert!(refusal(&[
            ("DB_KIND", "trino"),
            ("DB_HOST", "c"),
            ("DB_JWT", JWT),
            ("DB_PASSWORD", "pw"),
        ])
        .contains("not both"));
        // A password inside the URL is a password.
        assert!(refusal(&[
            ("DB_URL", "trino://u:secret@c:443?sslmode=require"),
            ("DB_JWT", JWT),
        ])
        .contains("not both"));
        for kind in ["postgres", "mysql"] {
            let message = refusal(&[("DB_KIND", kind), ("DB_HOST", "h"), ("DB_JWT", JWT)]);
            assert!(message.contains("only for Trino"), "{message}");
            assert!(message.contains(kind), "{message}");
        }
        for bad in ["has space", "semi;colon", "quote\"", "new\nline"] {
            let message = refusal(&[
                ("DB_KIND", "trino"),
                ("DB_HOST", "c"),
                ("DB_JWT", &format!("abc{bad}def")),
            ]);
            assert_eq!(message, "DB_JWT is not a valid bearer token");
        }
        let too_long = "a".repeat(8193);
        assert_eq!(
            refusal(&[
                ("DB_KIND", "trino"),
                ("DB_HOST", "c"),
                ("DB_JWT", &too_long)
            ]),
            "DB_JWT is not a valid bearer token"
        );
        assert!(build(&settings(&[
            ("DB_KIND", "trino"),
            ("DB_HOST", "c"),
            ("DB_JWT", &"a".repeat(8192))
        ]))
        .is_ok());
    }

    // ------------------------------------------------------------------- //
    // DB_CA_FILE (blueprint W11 §7.2)
    // ------------------------------------------------------------------- //

    fn fixture(name: &str) -> String {
        format!(
            "{}/../qh-driver-postgres/tests/tls/{name}",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn a_ca_file_is_read_once_and_carried_for_the_two_engines_that_can_use_it() {
        let ca = fixture("ca.crt");
        let postgres = config(&[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full"),
            ("DB_CA_FILE", &ca),
        ]);
        assert_eq!(
            postgres.tls_ca.as_ref().unwrap().path().to_string_lossy(),
            ca
        );
        let trino = trino(&[("DB_SCHEME", "https"), ("DB_CA_FILE", &ca)]);
        assert!(trino.tls_ca.is_some());
        assert_eq!(trino.tls, TlsMode::Require);
    }

    #[test]
    fn a_ca_file_where_it_would_be_ignored_or_cannot_be_used_is_refused_by_name() {
        let ca = fixture("ca.crt");
        // MySQL cannot take one, and says why.
        let message = refusal(&[
            ("DB_KIND", "mysql"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full"),
            ("DB_CA_FILE", &ca),
        ]);
        assert!(message.contains("not supported for MySQL"), "{message}");

        // Every mode but the verifying one would silently not use it.
        for extra in [
            ("DB_SSLMODE", "prefer"),
            ("DB_SSLMODE", "require"),
            ("DB_SSLMODE", "disable"),
            ("DB_INSECURE", "1"),
        ] {
            let mut all = vec![
                ("DB_KIND", "postgres"),
                ("DB_HOST", "db"),
                ("DB_CA_FILE", ca.as_str()),
            ];
            if extra.0 == "DB_INSECURE" {
                all.push(("DB_SSLMODE", "verify-full"));
            }
            all.push(extra);
            let message = refusal(&all);
            assert!(message.contains(&ca), "{extra:?}: {message}");
        }

        // A private key, a missing file and a file that is no PEM are the file's own errors.
        let key = fixture("ca.key");
        let error = build(&settings(&[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full"),
            ("DB_CA_FILE", &key),
        ]))
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::Ca(CaError::HoldsPrivateKey { .. })),
            "{error:?}"
        );
        let error = build(&settings(&[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full"),
            ("DB_CA_FILE", "/nonexistent/ca.pem"),
        ]))
        .unwrap_err();
        assert!(
            matches!(error, ConfigError::Ca(CaError::Unreadable { .. })),
            "{error:?}"
        );
    }

    #[test]
    fn mysql_through_a_tunnel_cannot_verify_a_certificate() {
        let through = |mode: &str| {
            build(&settings(&[
                ("DB_KIND", "mysql"),
                ("DB_HOST", "db"),
                ("DB_SSLMODE", mode),
                ("SSH_HOST", "bastion"),
                ("SSH_USER", "d"),
            ]))
        };
        let ConfigError::Usage(message) = through("verify-full").unwrap_err() else {
            panic!("a usage refusal");
        };
        assert!(message.contains("MySQL cannot verify"), "{message}");
        // The encrypted-not-verified mode is what the message points at, and it works.
        assert!(through("require").is_ok());
        // Direct, and the other engines through a tunnel, are unchanged.
        assert!(build(&settings(&[
            ("DB_KIND", "mysql"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full")
        ]))
        .is_ok());
        assert!(build(&settings(&[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "db"),
            ("DB_SSLMODE", "verify-full"),
            ("SSH_HOST", "bastion"),
            ("SSH_USER", "d"),
        ]))
        .is_ok());
    }
}
