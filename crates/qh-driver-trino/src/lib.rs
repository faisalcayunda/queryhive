//! Trino: the client protocol spoken by hand, because there is no connection to hold.
//!
//! How a query runs
//! ----------------
//! `POST /v1/statement` starts it and answers with a `nextUri`; each `GET` on that
//! URI returns another *page* — more rows, or more waiting — until a response
//! carries no `nextUri`. Errors arrive on a page too, not only on the `POST`.
//! Cancelling is a `DELETE` on the current page URI. There is no socket, no
//! session state on our side, and nothing to pool: ADR-0006 decided this, and
//! [`Capabilities::persistent_connection`] is `false` to say so out loud.
//!
//! ## Why `columns()` may be empty until the first batch
//!
//! Every other driver here can promise that column metadata is valid the moment
//! `execute` returns. Trino cannot, and the reason is worth stating because it is
//! the same trap that cost the MySQL driver a day:
//!
//! ```text
//! POST /v1/statement   -> stats.state = QUEUED, and there is no `columns` key at all
//! GET  nextUri         -> still QUEUED, still no `columns`
//! GET  nextUri         -> RUNNING, columns, data
//! ```
//!
//! So waiting for the columns inside `execute` would mean waiting for the result
//! set to *begin*, which for a blocking query is when the query *finishes* — and a
//! caller with no cursor has nothing to cancel. [`TrinoCursor`] therefore carries
//! the columns as soon as a page supplies them and reports an empty slice before
//! that, and `execute` returns immediately. Columns are guaranteed present by the
//! time the first batch is returned, which is when a grid can actually use them.
//!
//! ## The precision the coordinator sends, and who decides it
//!
//! Trino encodes `timestamp`/`time` into the JSON result at the precision the
//! *client says it can read*, and the client says so with one request header,
//! `X-Trino-Client-Capabilities`. Without it, a value the server itself reports
//! as `timestamp(6)` arrives rounded to milliseconds and typed `timestamp`; with
//! `PARAMETRIC_DATETIME` in the header, the declared precision travels and
//! `12:00:00.123456` arrives intact. Measured on 483, same query, same second,
//! nothing else different — [`decode`] carries the two encodings side by side.
//!
//! So the header is not decoration: a request path that forgets it silently gets a
//! downgraded answer back, with no error to notice. It is applied in one private
//! helper, `authorized`, that every statement-protocol request goes through, and
//! that is also the one place a credential is attached (next section).
//!
//! ## A credential goes to one address, and only that one
//!
//! Two of the three URLs a statement uses are chosen by the *server*: the `nextUri`
//! of every page poll, and the `DELETE` that cancels. A coordinator behind a
//! TLS-terminating load balancer that does not forward the scheme answers with
//! `http://` there, and one that names another host or port sends the poll there.
//! Attaching `Authorization` to whatever URL arrives would put a password or a
//! token on the wire in clear, or hand it to another host.
//!
//! So `authorized` compares the URL's scheme, host and port with the address the
//! session was opened on **before** it builds the request, and refuses (a
//! permanent, named error that shows both addresses and no credential) when they
//! differ. A bearer token additionally needs `https`. A session with no password
//! and no token is not checked: nothing could leak, and an unauthenticated
//! coordinator that announces another address keeps working.
//!
//! A client that holds a secret also does not follow redirects. reqwest drops
//! `Authorization` on a redirect only when the host or the port changes, never when
//! only the scheme does, so an `https` to `http` redirect on the same authority
//! would carry the secret into clear text. The 3xx comes back as the error it is.
//! And a `Prefer` connection with a secret never falls back to `http`: the one
//! signal that allows the fallback (a coordinator that is not speaking TLS) is
//! exactly the one where a password would be sent in clear.
//!
//! ## TLS
//!
//! This is HTTP, so unlike the socket drivers the scheme in the URL *is* the TLS
//! decision, and all four [`TlsMode`]s mean here what they mean everywhere else:
//!
//! | Mode | Scheme | Certificate |
//! |---|---|---|
//! | `Disable` | `http` | — |
//! | `Prefer` | `https`, with `http` only when the coordinator does not speak TLS | not checked |
//! | `Require` | `https` | platform trust store, or the connection's own CA file |
//! | `RequireNoVerify` | `https` | not checked |
//!
//! The trust store is the platform's — on macOS the Keychain, through
//! `rustls-platform-verifier` — which is what makes a corporate CA the user
//! installed work without the app having to carry a CA file. A connection that
//! does name a CA file trusts that bundle and nothing else, host name still
//! checked; it is refused in any mode that would not use it.
//!
//! Through an SSH tunnel the driver is handed the tunnel's loopback address while
//! the certificate names the database, so `ConnectionConfig::tls_server_name`
//! becomes the host of the session's URLs and is resolved to the loopback address
//! (`ClientBuilder::resolve`). The TLS name, the SNI and the `Host` header are then
//! the database's own, and the socket still goes to the tunnel.
//!
//! **`Prefer` encrypts but does not verify**, which is a decision and not a
//! leftover. The Python engine's `prefer` (psycopg, pymysql) encrypted and did not
//! check, and an internal coordinator behind a self-signed certificate is ordinary
//! in this product's deployments; a `Prefer` that started verifying would break
//! those on upgrade, and the app's own per-connection answer to that situation is a
//! `verify: false` flag. `Require` is the mode that means "and check it".
//!
//! **`Prefer` falls back only when the coordinator is not speaking TLS at all.**
//! A plain-HTTP coordinator answers the TLS ClientHello with an ordinary HTTP
//! response, which `rustls` rejects as a malformed record; that is the one signal
//! that is allowed to downgrade. A server that *does* speak TLS and then fails —
//! a TLS alert, a truncated record, a connection reset mid-handshake — is an
//! error, because a downgrade an attacker can trigger is precisely the attack the
//! two cases are told apart to avoid. Not verifying the certificate does not blunt
//! this: the check is about what the peer *answered*, not about what it proved. The
//! decision is made once, on the statement's first `POST`, and pinned for the
//! session: a later page poll uses the client that was chosen and never re-decides,
//! so a poll cannot be made to fall back.

mod decode;
pub mod metadata;

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qh_core::{offset_of_line_column, ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ExplainFormat, ExplainOptions, ExplainSupport, MetadataSql, ObjectPath, ObjectsPage, Parameter,
    Session, TlsCa, TlsMode,
};
use qh_sql::{strip_terminator_dialect, Dialect};
use serde::Deserialize;
use serde_json::Value as Json;

// Re-exported so a caller — in practice the tests — can name the store it hands to
// [`client_for`] without having to depend on `rustls` at the same version.
pub use rustls::RootCertStore;

/// The pause the first empty page earns, before the backoff starts doubling.
///
/// Trino's own clients poll; there is no long-poll. A queued page costs the
/// coordinator a read of the query's own state, and Trino's own clients ask again
/// as soon as the answer arrives, so the pause exists to keep this client from
/// spinning rather than to spare the coordinator.
///
/// It is the pause that sets how long a page which has just become ready waits to
/// be noticed, and an empty page does not mean the query is slow: a client that
/// asks faster than workers fill the output buffer sees empty pages throughout a
/// healthy query. A flat half second therefore capped every such query at two
/// pages a second, which on a thousand-row page is two thousand rows a second
/// however fast the cluster was reading.
const POLL_INTERVAL_MIN: Duration = Duration::from_millis(10);

/// The pause the backoff stops growing at.
///
/// A query that has produced nothing for a while is polled at most five times a
/// second, and a page that becomes ready at any point in that wait is picked up
/// within a fifth of a second rather than within half a second.
const POLL_INTERVAL_MAX: Duration = Duration::from_millis(200);

/// How long the connect phase may take before it is given up on.
///
/// The connect phase is DNS, the TCP handshake, and — on the TLS modes — the TLS
/// handshake, and without a bound a coordinator whose port is wrong or filtered does
/// not fail: the SYN is dropped rather than refused, so the only thing that ends the
/// wait is the operating system's own TCP timeout, and the retry policy multiplies it
/// by the number of attempts. Measured against this project's own coordinator, a
/// wrong port left the app's "Test" spinner turning for minutes with nothing to show
/// for it. Ten seconds is long enough for a slow internal handshake and short enough
/// that a typo is a failure the user sees rather than a wait they cannot explain.
///
/// This is a *connect* bound and deliberately not an overall request timeout: a
/// statement's page poll is a short exchange even when the query behind it runs for
/// minutes, and a whole-request timeout would cut off a legitimately slow query.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What this client tells the coordinator it can decode.
///
/// `PARAMETRIC_DATETIME` decides the JSON *encoding* of `timestamp` and `time`: the
/// coordinator rounds them to milliseconds and strips the declared precision from
/// the type name unless the client that asked for the page announced it can read
/// the parametric form. Measured on Trino 483, one header apart and nothing else:
///
/// ```text
/// without: types ['timestamp with time zone', 'time']
///          data  [['2026-01-31 12:00:00.123 +07:00', '00:00:00.000']]
/// with:    types ['timestamp(6) with time zone', 'time(6)']
///          data  [['2026-01-31 12:00:00.123456 +07:00', '23:59:59.999999']]
/// ```
///
/// `23:59:59.999999` arriving as `00:00:00.000` is the same rounding seen from the
/// other side: 999.999 ms carries into the next second.
///
/// The Python engine's client sends `NUMBER,PARAMETRIC_DATETIME,SESSION_AUTHORIZATION`
/// (trino-python-client 0.339), and the two extra names describe that client's own
/// features rather than anything on the wire. Measured on 483: announcing all three
/// produces byte-identical answers to announcing this one, `BOGUS` alone is ignored,
/// and `BOGUS,PARAMETRIC_DATETIME` still buys microseconds. So the honest, smallest
/// set is what is sent.
const CLIENT_CAPABILITIES: &str = "PARAMETRIC_DATETIME";

/// The header [`CLIENT_CAPABILITIES`] travels in.
const CLIENT_CAPABILITIES_HEADER: &str = "X-Trino-Client-Capabilities";

// ---------------------------------------------------------------------------
// The wire types
// ---------------------------------------------------------------------------

/// One page of the client protocol.
///
/// Every field is optional because the protocol says so: a `POST` answer has no
/// `columns`, a queued page has no `data`, and the last page has no `nextUri`.
/// Requiring any of them would fail on a response that is perfectly correct.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    next_uri: Option<String>,
    #[serde(default)]
    columns: Option<Vec<WireColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<Json>>>,
    #[serde(default)]
    error: Option<WireError>,
    /// Rows the statement wrote, when it has a count to report.
    ///
    /// **Top level, not inside `stats`** — measured against Trino 483 rather than
    /// assumed: a `CREATE TABLE AS SELECT` answers with 25, an `INSERT` with 3, and a
    /// `SELECT` or a `DROP` with nothing at all. It is the field the trino client reads
    /// to set `cursor.rowcount`, so reading it is what keeps a write's report the same
    /// number the Python engine would have given.
    #[serde(default)]
    update_count: Option<i64>,
    // `stats` carries the state (QUEUED/RUNNING/FINISHED) and a pile of counters.
    // None of it is read here: whether a query has finished is answered by the
    // absence of `nextUri`, which is the protocol's own rule, and the counters have
    // no home in the Cursor contract. Deserialising it anyway would be a field kept
    // alive by hope, so the extra keys are simply ignored.
}

#[derive(Debug, Deserialize)]
struct WireColumn {
    name: String,
    #[serde(rename = "type")]
    type_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireError {
    message: String,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    error_name: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
    /// Where in the statement the coordinator objected, for a parse or analysis
    /// error. Both numbers are 1-based, and the column counts code points.
    #[serde(default)]
    error_location: Option<WireErrorLocation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireErrorLocation {
    line_number: u32,
    column_number: u32,
}

/// The running query as far as cancel needs to know it.
#[derive(Debug, Clone)]
struct Running {
    /// The query id, which is also the last resort for cancelling.
    id: String,
    /// The page to `DELETE` to stop it. Trino cancels by deleting the current
    /// page URI, so this is refreshed on every poll.
    next_uri: String,
}

/// Shared so `Session::cancel(&self)` can reach a query the cursor is advancing.
type Shared = Arc<Mutex<Option<Running>>>;

// ---------------------------------------------------------------------------
// Building the client
// ---------------------------------------------------------------------------

/// Build the HTTP client for one TLS mode.
///
/// `roots` is where certificates are checked, and only [`TlsMode::Require`] reads
/// it. `None` — what the driver itself uses — is the platform trust store, so a
/// root the user installed in the macOS Keychain is trusted. `Some` is the seam the
/// tests need: verifying a real certificate cannot be exercised in CI, but the
/// verifying path still must be, so a test hands in a store of its own choosing and
/// gets the same client construction.
///
/// `Prefer` takes no store at all, and that is deliberate rather than an omission.
/// It encrypts without verifying, which is what the Python engine's `prefer` did
/// (psycopg encrypts and does not check), and this product's deployments are full of
/// internal coordinators with self-signed certificates. A `Prefer` that verified
/// would break those on upgrade with a message about a TLS handshake — and `Require`
/// is the mode for "and check it".
pub fn client_for(
    tls: TlsMode,
    roots: Option<RootCertStore>,
) -> Result<reqwest::Client, EngineError> {
    build_client(tls, roots, false, None)
}

/// [`client_for`], for a session that holds a password or a token: the same client that
/// follows no redirect.
///
/// reqwest's default follows up to ten, and drops `Authorization` only when the host or
/// the port changes. A redirect from `https` to `http` on the same authority keeps the
/// header, so the credential would leave in clear text. A 3xx is returned as it is and
/// surfaces as the failure it is.
pub fn client_for_secret(
    tls: TlsMode,
    roots: Option<RootCertStore>,
) -> Result<reqwest::Client, EngineError> {
    build_client(tls, roots, true, None)
}

/// The one place a client is built, so the plain, the secret-holding and the tunnelled
/// variants cannot drift apart.
///
/// `resolve` maps a DNS name to an address without asking DNS: the tunnel case, where the
/// URLs carry the database's own name and the socket has to go to the loopback endpoint.
/// reqwest takes the port from the URL, not from this address.
fn build_client(
    tls: TlsMode,
    roots: Option<RootCertStore>,
    no_redirects: bool,
    resolve: Option<(&str, IpAddr)>,
) -> Result<reqwest::Client, EngineError> {
    // The connect bound is set on every mode, because a port that does not answer is
    // not a TLS question: it drops the SYN before any scheme is chosen.
    let mut builder = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
    if no_redirects {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    if let Some((name, address)) = resolve {
        builder = builder.resolve(name, SocketAddr::new(address, 0));
    }
    let built = match tls {
        TlsMode::Disable => builder.build(),
        // `verify: false` in the app's own vocabulary for `RequireNoVerify`, and the
        // same absence of checking inside `Prefer`: both encrypt and neither
        // verifies. What separates them is below — `Prefer` is the only one that may
        // end up in clear, and only when the coordinator has no TLS to speak.
        TlsMode::Prefer | TlsMode::RequireNoVerify => {
            builder.danger_accept_invalid_certs(true).build()
        }
        TlsMode::Require => match roots {
            Some(roots) => {
                let config = tls_builder()?
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                builder
                    .use_preconfigured_tls(with_http1_alpn(config))
                    .build()
            }
            None => {
                use rustls_platform_verifier::BuilderVerifierExt;
                let config = tls_builder()?
                    .with_platform_verifier()
                    .map_err(|error| {
                        client_error(format!(
                            "the platform trust store could not be opened: {error}"
                        ))
                    })?
                    .with_no_client_auth();
                builder
                    .use_preconfigured_tls(with_http1_alpn(config))
                    .build()
            }
        },
    };
    built.map_err(|error| client_error(format!("could not build the HTTP client: {error}")))
}

/// The roots a connection's own CA file makes: exactly its certificates, with no platform
/// store beside them. The bundle was validated when it was read, so a failure here is a
/// certificate `rustls` rejects after `webpki` accepted it, reported and not skipped (a
/// smaller store than the one chosen would silently change what is trusted).
pub fn roots_from(ca: &TlsCa) -> Result<RootCertStore, EngineError> {
    let mut roots = RootCertStore::empty();
    for der in ca.der() {
        roots
            .add(rustls::pki_types::CertificateDer::from(der.clone()))
            .map_err(|error| {
                client_error(format!(
                    "the CA file {} holds a certificate TLS cannot use: {error}",
                    ca.path().display()
                ))
            })?;
    }
    Ok(roots)
}

/// The start of every verifying configuration.
///
/// The crypto provider is named rather than resolved: `ClientConfig::builder()`
/// looks one up from the crate features and the process default, and panics when
/// the answer is ambiguous — a panic at connect time, chosen by whatever else
/// happens to be in the dependency graph. `ring` is the one this crate compiles
/// on purpose, so it is the one named.
fn tls_builder(
) -> Result<rustls::ConfigBuilder<rustls::ClientConfig, rustls::WantsVerifier>, EngineError> {
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|error| client_error(format!("the TLS versions could not be configured: {error}")))
}

/// Advertise HTTP/1.1, because that is the only version this build speaks.
///
/// A preconfigured [`rustls::ClientConfig`] replaces the one reqwest would have
/// built, ALPN included, so the one protocol it would have offered is restored
/// here rather than left to chance.
fn with_http1_alpn(mut config: rustls::ClientConfig) -> rustls::ClientConfig {
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

fn client_error(message: String) -> EngineError {
    EngineError::Connect {
        message,
        kind: FailureKind::Permanent,
    }
}

/// Whether a failed HTTPS attempt means the peer is not speaking TLS at all.
///
/// This is the *only* failure [`TlsMode::Prefer`] may downgrade on, and it is the
/// narrow reading deliberately: a plain-HTTP coordinator answers the TLS
/// ClientHello with an ordinary HTTP response, and `rustls` rejects those bytes as
/// a malformed record (`InvalidMessage`). Everything else — a certificate that
/// does not verify, an alert, a reset, a timeout — means TLS was there and did
/// not complete, and retrying in clear is exactly the downgrade an attacker on the
/// path would like to cause.
fn not_a_tls_server(error: &reqwest::Error) -> bool {
    let mut source = Some(error as &(dyn std::error::Error + 'static));
    while let Some(current) = source {
        if let Some(verdict) = rustls_verdict(current) {
            return verdict;
        }
        source = current.source();
    }
    false
}

/// The rustls error inside one link of a `reqwest` failure, if there is one.
///
/// Written as a second walk rather than a `downcast` in the loop above because an
/// `io::Error` does not hand out its own cause from `source()` — it delegates to
/// the *innermost* cause, so the layer that actually holds the rustls error (a
/// `Custom` wrapping another `Custom` wrapping the error) is stepped over by any
/// ordinary chain walk. `get_ref()` is what reaches it. Skipping this is not
/// academic: the plain-coordinator case reports
/// `Custom { kind: Other, error: Custom { kind: InvalidData, error:
/// InvalidMessage(InvalidContentType) } }` and nothing else.
fn rustls_verdict(error: &(dyn std::error::Error + 'static)) -> Option<bool> {
    let mut current = error;
    loop {
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return Some(matches!(tls, rustls::Error::InvalidMessage(_)));
        }
        current = current.downcast_ref::<std::io::Error>()?.get_ref()?;
    }
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// The Trino driver.
///
/// `connect` builds an HTTP client and nothing else: there is no socket to open
/// and therefore nothing that can fail because a server is unreachable. The first
/// statement is where a bad host or a bad port is found, which is why
/// [`TrinoSession::run`] reports the transport failure as a connect failure rather
/// than an empty result.
pub struct TrinoDriver;

impl TrinoDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TrinoDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for TrinoDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Trino
    }

    fn label(&self) -> &'static str {
        "Trino"
    }

    fn default_port(&self) -> u16 {
        8080
    }

    fn metadata(&self) -> Option<&dyn MetadataSql> {
        Some(&metadata::TrinoMetadata)
    }

    /// JSON without ANALYZE, ANALYZE as text, and no way to have both (measured on 483, the
    /// grammar takes no option list after `EXPLAIN ANALYZE`). No fence: the driver has no
    /// transaction to put an ANALYZE in (`Capabilities::transactions`), so what keeps a write
    /// out of one is the engine's read-only check.
    fn explain_support(&self) -> ExplainSupport {
        ExplainSupport {
            name: "Trino",
            json: true,
            analyze: true,
            analyze_json: false,
            analyze_fence: None,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Trino has no interactive transaction across statements. Each
            // statement is an HTTP exchange, so there is nothing to hold open.
            transactions: false,
            multiple_result_sets: false,
            // `DELETE` on the page URI, which the server acts on. False would mean
            // cancel only stopped reading locally, and the UI would have to say so.
            cancel: true,
            explain: true,
            levels: vec![
                BrowseLevel::Catalog,
                BrowseLevel::Schema,
                BrowseLevel::Table,
            ],
            // Name and Type, and nothing else, because nothing else is true here:
            // Trino's information_schema exposes catalog/schema/name/type and no
            // OID, owner or ACL. Inventing those columns for Trino would be
            // inventing them, so the list matches the Python engine's driver.
            objects_columns: vec!["Name".to_owned(), "Type".to_owned()],
            // No socket is kept between statements, so anything that assumed one
            // -- an idle timeout, a keepalive, a pool -- must ask first.
            persistent_connection: false,
            // The `query_max_run_time` session property, sent on the statement's POST.
            statement_timeout: true,
            // Trino's HTTP protocol has no bind parameters: a query is text and a
            // page of rows. A caller that wants a value out of the statement text
            // must inline it; this driver refuses a non-empty `parameters` rather
            // than send placeholders the coordinator would read literally.
            parameters: None,
            read_only: false,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        // `reqwest::Client` holds the connection pool that makes repeated pages
        // cheap. It is not a *database* connection: nothing here is a session that
        // the server knows about, and the driver still reports
        // `persistent_connection = false`. The client is built once per session,
        // here, from the mode — never per request.
        let roots = config
            .ca_for_verifying_mode()?
            .map(roots_from)
            .transpose()?;
        let secret = config.password.is_some() || config.bearer.is_some();
        if config.bearer.is_some() {
            if config.password.is_some() {
                return Err(EngineError::Usage {
                    message: "a connection sends a password or a bearer token, not both".to_owned(),
                });
            }
            // A token that could travel in clear is a leaked token. `Prefer` is refused
            // too, because it is the mode that may end up on `http`.
            if !matches!(config.tls, TlsMode::Require | TlsMode::RequireNoVerify) {
                return Err(EngineError::Usage {
                    message: "a bearer token is only sent over verified or unverified TLS, never \
                              over http or a mode that may fall back to it"
                        .to_owned(),
                });
            }
        }
        // Through a tunnel the URLs carry the database's own name, resolved to the
        // loopback endpoint the driver was handed as `host`.
        let name = config.tls_server_name.as_deref();
        let resolve = match name {
            Some(name) => Some((name, loopback_of(&config.host)?)),
            None => None,
        };
        let base = format!(
            "{}://{}",
            scheme_for(config.tls),
            authority(name.unwrap_or(&config.host), config.port)
        );

        let client = build_client(config.tls, roots, secret, resolve)?;
        // `Prefer` starts on HTTPS and keeps the plaintext client in reserve for
        // the one case it is allowed to use it. Every other mode has nothing to
        // fall back to, which is what makes "require" mean required. And a session
        // with a password or a token has no reserve at all: the one signal that
        // allows the fallback, a coordinator that is not speaking TLS, is exactly
        // the one where a secret would go out in clear.
        let fallback = if config.tls == TlsMode::Prefer && !secret {
            Some(build_client(TlsMode::Disable, None, false, resolve)?)
        } else {
            None
        };

        let mut credentials = Credentials::new(&config.user, config.password.clone());
        credentials.bearer = config.bearer.clone();
        if secret {
            credentials.origin = Some(Origin::of(&base).map_err(|why| {
                client_error(format!(
                    "the coordinator address {base} is not usable: {why}"
                ))
            })?);
        }

        Ok(Box::new(TrinoSession {
            client,
            fallback,
            base,
            credentials,
            catalog: config.database.clone().unwrap_or_default(),
            schema: config.schema.clone().unwrap_or_default(),
            running: Arc::new(Mutex::new(None)),
            last_id: None,
        }))
    }
}

/// The scheme a mode starts on.
///
/// `Prefer` starts on `https`: falling back to clear is the exception, so it is
/// the attempt that has to fail first, never the default.
fn scheme_for(tls: TlsMode) -> &'static str {
    match tls {
        TlsMode::Disable => "http",
        TlsMode::Prefer | TlsMode::Require | TlsMode::RequireNoVerify => "https",
    }
}

/// `host:port` as a URL spells it: an IPv6 literal needs its brackets.
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// The address a TLS server name is resolved to: the connection's own `host`, which a
/// tunnel sets to its loopback endpoint. A name needs somewhere to go, so a host that is
/// not an address is refused, not looked up.
fn loopback_of(host: &str) -> Result<IpAddr, EngineError> {
    host.trim_matches(['[', ']'])
        .parse()
        .map_err(|_| EngineError::Usage {
            message: "a TLS server name needs the connection address to be an IP address \
                      (the tunnel's loopback endpoint)"
                .to_owned(),
        })
}

/// The same authority under a different scheme, which is all a `Prefer` downgrade
/// changes: the host and the port stay exactly as configured.
fn with_scheme(scheme: &str, base: &str) -> String {
    match base.split_once("://") {
        Some((_, authority)) => format!("{scheme}://{authority}"),
        None => format!("{scheme}://{base}"),
    }
}

fn non_empty(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text.trim().to_owned())
    }
}

/// What every request to the coordinator has to carry: who is asking, and with
/// what secret.
///
/// Trino's statement protocol keeps no session, so there is nothing that can be
/// authenticated once and reused. The identity travels on the request itself, and
/// `password` or `bearer` is `Some` exactly when the connection has one. All of it
/// goes out together, which is why it is passed as one value rather than as separate
/// arguments a call site could get half right.
///
/// It also carries the one address the secret may be sent to (`origin`), because a
/// credential that knows where it may go cannot be attached to a URL somewhere else.
/// [`Credentials::new`] leaves the address unset, and a secret with no address on record
/// is never sent: the driver's `connect` fills it in from the session's base.
#[derive(Clone)]
pub struct Credentials {
    pub user: String,
    pub password: Option<String>,
    /// A bearer token, sent as `Authorization: Bearer` **instead of** Basic.
    pub bearer: Option<String>,
    /// The scheme, host and port of the session's address. `None` with a secret fails
    /// closed: nothing is sent.
    origin: Option<Origin>,
}

impl Credentials {
    /// The user a bare connection runs as, and no secret.
    ///
    /// An empty `user` is not sent as an empty header: Trino answers that with a
    /// 401 in the same shape it answers a missing user, and the Python engine
    /// defaulted to this name too, so a connection with no username keeps working.
    pub fn new(user: &str, password: Option<String>) -> Self {
        Self {
            user: non_empty(user).unwrap_or_else(|| DEFAULT_USER.to_owned()),
            password,
            bearer: None,
            origin: None,
        }
    }

    fn has_secret(&self) -> bool {
        self.password.is_some() || self.bearer.is_some()
    }

    /// Why a request to `url` must not carry the secret, or `None` when it may.
    ///
    /// The message names both addresses (never a path, a query or a credential) and is
    /// the one a person reads, so it says what to check.
    fn refusal_for(&self, url: &str) -> Option<String> {
        if !self.has_secret() {
            return None;
        }
        let Some(session) = &self.origin else {
            return Some(
                "these credentials have no session address on record, so they were not sent"
                    .to_owned(),
            );
        };
        let Ok(target) = Origin::of(url) else {
            return Some(
                "the coordinator sent a URL that could not be read, so the credentials were \
                 not sent"
                    .to_owned(),
            );
        };
        if &target != session {
            return Some(format!(
                "the coordinator sent a page URL on {target}, not on {session} (the address \
                 this connection uses), so the credentials were not sent. Check the \
                 coordinator's external address and its forwarded-header handling."
            ));
        }
        if self.bearer.is_some() && session.scheme != "https" {
            return Some(format!(
                "a bearer token is only sent over https, and this connection uses {session}"
            ));
        }
        None
    }
}

/// Never prints the password or the token.
///
/// A `{:?}` on anything holding credentials is the shortest path from a support
/// request to a secret in a log, so the derived form is replaced with one that
/// reports only whether one is present — the same rule `ConnectionConfig` follows.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let present = |value: bool| if value { "present" } else { "none" };
        write!(
            formatter,
            "Credentials(user: {:?}, password: {}, bearer: {})",
            self.user,
            present(self.password.is_some()),
            present(self.bearer.is_some()),
        )
    }
}

/// Where a URL is: scheme, host and port, compared the way a browser's same-origin rule
/// compares them. `https://h` and `https://h:443` are one origin; `http://h:443` and
/// `https://h:443` are two.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    fn of(url: &str) -> Result<Self, &'static str> {
        let url = reqwest::Url::parse(url).map_err(|_| "it is not a URL")?;
        match (url.host_str(), url.port_or_known_default()) {
            (Some(host), Some(port)) if matches!(url.scheme(), "http" | "https") => Ok(Self {
                scheme: url.scheme().to_owned(),
                // The parser has already lower-cased a domain, and an IPv6 literal keeps
                // its brackets, so two spellings of one host compare equal.
                host: host.to_owned(),
                port,
            }),
            _ => Err("it has no host, port or http scheme"),
        }
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}://{}:{}", self.scheme, self.host, self.port)
    }
}

/// The name a connection runs as when it names no user.
pub const DEFAULT_USER: &str = "queryhive";

/// A request that was not built, because its URL is not where the credentials may go.
struct Refused(String);

impl Refused {
    /// As the failure of the statement's `POST`.
    fn connect(self) -> EngineError {
        EngineError::Connect {
            message: self.0,
            // The same URL would be refused again, so a retry only repeats it.
            kind: FailureKind::Permanent,
        }
    }

    /// As the failure of a page poll or a cancel.
    fn query(self) -> EngineError {
        EngineError::Query {
            message: self.0,
            code: None,
            position: None,
            kind: FailureKind::Permanent,
        }
    }
}

/// Build a request to the coordinator: the headers every one carries, and the credential,
/// **if** `url` is where the credential may go.
///
/// The one function that attaches a secret (a test counts, so a new request path cannot
/// quietly build its own), and the one that checks the destination first. Two omissions are
/// invisible from the outside, and one of them costs the request outright.
///
/// [`CLIENT_CAPABILITIES`] is the quiet one: a request without it is answered with
/// the same types and values downgraded to milliseconds, and nothing reports an
/// error. So the capability is attached where the request is built, not left to
/// each call site to remember — the statement's `POST`, every page poll, and the
/// `DELETE` that cancels all pass through here.
///
/// The credentials are the loud one. `X-Trino-User` alone names the user but
/// authenticates nobody, so a coordinator configured with password-file or LDAP
/// auth answers 401 to every request while the connection is, as far as the user
/// can tell, correctly configured. Sending `Authorization: Basic` as well is what
/// makes a password in the connection mean something; without it, a password only
/// raised [`TlsMode`] and then went nowhere.
///
/// The poll does not *have* to carry the capability on 483: measured, a POST with
/// the header followed by a poll without it still returned `timestamp(6)` and
/// `12:00:00.123456`, because the coordinator settles the encoding when the
/// statement is created. It is sent anyway — the header is part of what this client
/// is, the reference client sends it on every request, and a coordinator that read
/// it per response would otherwise downgrade the middle of a result set.
fn authorized(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    credentials: &Credentials,
) -> Result<reqwest::RequestBuilder, Refused> {
    if let Some(reason) = credentials.refusal_for(url) {
        return Err(Refused(reason));
    }
    let request = client
        .request(method, url)
        .header("X-Trino-User", &credentials.user)
        .header(CLIENT_CAPABILITIES_HEADER, CLIENT_CAPABILITIES);
    Ok(match (&credentials.bearer, &credentials.password) {
        (Some(token), _) => request.bearer_auth(token),
        (None, Some(password)) => request.basic_auth(&credentials.user, Some(password)),
        (None, None) => request,
    })
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

struct TrinoSession {
    client: reqwest::Client,
    /// `Prefer` only, and only until the first statement has settled the scheme:
    /// the plaintext client used when the coordinator turns out not to speak TLS.
    /// Taking it (`None`) is what makes the fallback a decision taken once — after
    /// that there is no client to fall back with, so no later request, page poll
    /// included, can reach for cleartext.
    fallback: Option<reqwest::Client>,
    /// `http://host:port` or `https://host:port`, with no trailing slash. Settled
    /// once, on the first `POST`.
    base: String,
    credentials: Credentials,
    /// Trino's catalog, which is this driver's `database` slot.
    catalog: String,
    schema: String,
    /// What `cancel` needs while a query is in flight, cleared when it ends.
    running: Shared,
    /// The id of the last statement sent, kept after it finishes.
    ///
    /// Separate from `running` because the two answer different questions: `running`
    /// is "is there something to cancel", which stops being true the moment the last
    /// page arrives, and this is "which statement produced the grid the user is
    /// looking at", which is still true afterwards. Trino sends the id with every
    /// statement, so the POST answer is enough to know it.
    last_id: Option<String>,
}

impl TrinoSession {
    /// `DELETE` the page URI of the query in flight, which is how Trino stops one.
    ///
    /// Shared by `cancel` (a user's Stop) and `reset` (a result nobody finished reading).
    async fn delete_running(&self) -> Result<(), EngineError> {
        let running = self.running.lock().expect("running state").clone();

        // Nothing running is not an error: the button can be pressed after the
        // query finished, and reporting that as a failure would be noise.
        let Some(running) = running else {
            return Ok(());
        };

        // `DELETE` on the page URI is how Trino cancels. A non-2xx answer means the
        // query was already gone, which is the outcome that was wanted.
        let target = if running.next_uri.is_empty() {
            format!("{}/v1/statement/{}", self.base, running.id)
        } else {
            running.next_uri
        };
        // The URL is the one the coordinator gave for this query, so it is checked like
        // any other before the credential goes with it; a refused cancel sends nothing
        // and does not fall back to a URL derived from the base, which is not the one
        // the server named.
        let response = authorized(
            &self.client,
            reqwest::Method::DELETE,
            &target,
            &self.credentials,
        )
        .map_err(Refused::query)?
        .send()
        .await
        .map_err(|error| EngineError::Query {
            message: format!("could not send the cancel: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if response.status().is_success() || response.status().as_u16() == 404 {
            *self.running.lock().expect("running state") = None;
            return Ok(());
        }

        Err(EngineError::Query {
            message: format!("the server refused the cancel: {}", response.status()),
            code: Some(response.status().as_u16().to_string()),
            kind: FailureKind::Transient,
            position: None,
        })
    }

    /// Resolve a path's catalog and schema, falling back to the connection's own.
    ///
    /// A browse call may carry a catalog even when the connection did not, which
    /// is the whole point of the catalog level existing.
    fn catalog_and_schema(&self, path: &ObjectPath) -> (String, String) {
        let catalog = path
            .catalog
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| self.catalog.clone());
        let schema = path
            .schema
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| self.schema.clone());
        (catalog, schema)
    }

    /// The statement's request, over whatever client and scheme the session currently
    /// holds, with no fallback of its own.
    ///
    /// Split out so `post` can try it once, decide, and try again if — and only
    /// if — the peer turned out not to speak TLS. `session` is the
    /// `X-Trino-Session` value that carries the statement's own bound, when it has
    /// one; it belongs on the POST and nowhere else, because a session property is
    /// read when the query is created.
    fn post_request(
        &self,
        sql: &str,
        session: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, EngineError> {
        let mut request = authorized(
            &self.client,
            reqwest::Method::POST,
            &format!("{}/v1/statement", self.base),
            &self.credentials,
        )
        .map_err(Refused::connect)?
        .header("Content-Type", "text/plain");
        if let Some(session) = session {
            request = request.header("X-Trino-Session", session);
        }
        if !self.catalog.is_empty() {
            request = request.header("X-Trino-Catalog", &self.catalog);
        }
        if !self.schema.is_empty() {
            request = request.header("X-Trino-Schema", &self.schema);
        }
        Ok(request.body(sql.to_owned()))
    }

    /// POST one statement and return its first page.
    ///
    /// This is where [`TlsMode::Prefer`] settles the scheme, because it is the
    /// first request the session makes and the answer has to hold for every page
    /// after it.
    async fn post(&mut self, sql: &str, timeout: Option<Duration>) -> Result<Page, EngineError> {
        let session = session_header(timeout);
        let response = match self.post_request(sql, session.as_deref())?.send().await {
            Ok(response) => response,
            // The one downgrade that is allowed, and only with a plaintext client
            // still in reserve: the coordinator answered the handshake with
            // something that is not TLS, so it has none to speak.
            Err(failure) if self.fallback.is_some() && not_a_tls_server(&failure) => {
                // Taking the fallback is what makes this a decision taken once:
                // after this there is no plaintext client left to reach for, so a
                // later poll has nothing to downgrade to.
                if let Some(plain) = self.fallback.take() {
                    self.client = plain;
                }
                self.base = with_scheme("http", &self.base);
                self.post_request(sql, session.as_deref())?
                    .send()
                    .await
                    .map_err(|error| EngineError::Connect {
                        message: format!("could not reach {}: {error}", self.base),
                        kind: FailureKind::Transient,
                    })?
            }
            // A connection with a password or a token has no plaintext reserve, so a
            // coordinator that is not speaking TLS is a refusal, and says why: the
            // user chose `prefer` expecting a fallback, and a bare handshake error
            // would not tell them the secret is the reason there is none.
            Err(failure) if self.credentials.has_secret() && not_a_tls_server(&failure) => {
                return Err(EngineError::Connect {
                    message: format!(
                        "{} does not speak TLS, and a connection with a password or a token \
                         never falls back to plain http. Turn TLS on at the coordinator.",
                        self.base
                    ),
                    kind: FailureKind::Permanent,
                })
            }
            Err(error) => {
                return Err(EngineError::Connect {
                    message: format!("could not reach {}: {error}", self.base),
                    kind: FailureKind::Transient,
                })
            }
        };

        let status = response.status();
        let text = response.text().await.map_err(|error| EngineError::Query {
            message: format!("the response body could not be read: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if !status.is_success() {
            // Trino answers 4xx with the same error object as a failed page, so
            // the body is parsed before the status is turned into a message:
            // throwing away a structured error because of a status code loses the
            // reason.
            if let Ok(page) = serde_json::from_str::<Page>(&text) {
                if let Some(error) = page.error {
                    return Err(map_error(error, sql, timeout));
                }
            }
            return Err(EngineError::Query {
                message: format!("{status}: {}", text.trim()),
                code: Some(status.as_u16().to_string()),
                kind: FailureKind::Permanent,
                position: None,
            });
        }

        let page: Page = serde_json::from_str(&text).map_err(|error| EngineError::Query {
            message: format!("the response was not a Trino page: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if let Some(error) = page.error {
            return Err(map_error(error, sql, timeout));
        }
        if let Some(running) = running_from(&page) {
            *self.running.lock().expect("running state") = Some(running);
        }
        Ok(page)
    }

    /// Run a statement to completion and return its rows as text.
    ///
    /// Only for the metadata statements -- `SHOW CATALOGS`, the objects grid --
    /// where the whole result is small and is wanted at once. Anything a user
    /// typed goes through [`Session::execute`] instead, so that it streams and can
    /// be cancelled.
    async fn collect(&mut self, sql: &str) -> Result<Vec<Vec<Value>>, EngineError> {
        let mut cursor = self.execute(sql, &ExecuteOptions::default()).await?;
        let mut rows = Vec::new();
        while let Some(batch) = cursor.next_batch(256).await? {
            let width = batch.columns();
            let height = width.first().map_or(0, Vec::len);
            for index in 0..height {
                rows.push(
                    (0..width.len())
                        .map(|column| width[column][index].clone())
                        .collect(),
                );
            }
        }
        Ok(rows)
    }

    /// The first column of every row, as text.
    ///
    /// `SHOW CATALOGS` and friends return one column, and the tree wants names.
    async fn collect_names(&mut self, sql: &str) -> Result<Vec<String>, EngineError> {
        Ok(self
            .collect(sql)
            .await?
            .into_iter()
            .filter_map(|mut row| {
                if row.is_empty() {
                    return None;
                }
                Some(value_to_text(&row.remove(0)))
            })
            .filter(|name| !name.is_empty())
            .collect())
    }
}

fn running_from(page: &Page) -> Option<Running> {
    let id = page.id.clone()?;
    let next_uri = page.next_uri.clone()?;
    Some(Running { id, next_uri })
}

/// Turn a Trino error into this crate's error.
///
/// `errorType` is what decides whether a retry is worth attempting, and Trino's
/// own vocabulary is used rather than guessed: `USER_ERROR` means the statement or
/// the data is wrong and repeating it will produce the same thing, so it is
/// permanent. Everything else — internal errors, resource exhaustion — is
/// transient, because those are the ones that can pass on a second attempt.
///
/// `sql` is the text the coordinator was sent, which `errorLocation` counts into.
fn map_error(error: WireError, sql: &str, timeout: Option<Duration>) -> EngineError {
    // The time bound comes first, because it is the one failure that is not the
    // server's opinion of the statement. Trino 483 raises two names for it and both
    // are read: `query_max_run_time` — the property this driver sets — produced
    // `EXCEEDED_TIME_LIMIT` when measured against the dev coordinator, and
    // `QUERY_EXCEEDED_MAX_EXECUTION_TIME` is the coordinator-config spelling of the
    // same idea. Matching only the second would have missed every timeout this
    // driver actually causes.
    if is_timeout(&error) {
        return EngineError::statement_timeout(timeout, &error.message);
    }
    // Checked before `errorType`, because the name is more specific and the two
    // disagree: a cancelled query arrives as `USER_CANCELED` with
    // `errorType = USER_ERROR`. Verified against Trino 483 -- `DELETE` on the page
    // URI produced `{"message":"Query was canceled","errorCode":3,
    // "errorName":"USER_CANCELED","errorType":"USER_ERROR"}`. Classifying that as a
    // generic user error would show a failure to the user who asked for the stop.
    let kind = if error.error_name.as_deref() == Some("USER_CANCELED") {
        FailureKind::Cancelled
    } else {
        match error.error_type.as_deref() {
            Some("USER_ERROR") => FailureKind::Permanent,
            Some(_) => FailureKind::Transient,
            // No type at all: treated as permanent, because the safer mistake is
            // retrying too little rather than looping on something the server will
            // reject identically forever.
            None => FailureKind::Permanent,
        }
    };
    let position = error
        .error_location
        .as_ref()
        .and_then(|at| offset_of_line_column(sql, at.line_number, at.column_number))
        .filter(|_| !sql.is_empty());
    let name = error.error_name.unwrap_or_default();
    let message = if name.is_empty() {
        error.message
    } else {
        format!("{name}: {}", error.message)
    };
    EngineError::Query {
        message,
        code: error.error_code.map(|code| code.to_string()),
        kind,
        position,
    }
}

/// Whether Trino is reporting that the query ran past its time bound.
fn is_timeout(error: &WireError) -> bool {
    matches!(
        error.error_name.as_deref(),
        Some("EXCEEDED_TIME_LIMIT" | "QUERY_EXCEEDED_MAX_EXECUTION_TIME")
    )
}

/// The `X-Trino-Session` value that bounds a statement's run time.
///
/// `query_max_run_time` is a session property, and it is sent on the statement's POST
/// because that is when the coordinator reads session properties. The value is
/// milliseconds, which Trino's duration parser accepts (`1500ms` was measured against
/// 483, alongside `2s`).
fn session_header(timeout: Option<Duration>) -> Option<String> {
    timeout.map(|limit| format!("query_max_run_time={}ms", limit.as_millis()))
}

#[async_trait]
impl Session for TrinoSession {
    fn capabilities(&self) -> Capabilities {
        TrinoDriver.capabilities()
    }

    fn query_id(&self) -> Option<String> {
        self.last_id.clone()
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        // Returns as soon as the POST answers, which is normally while the query
        // is still QUEUED. See the module note: waiting here for the columns would
        // mean waiting for the whole query.
        let page = self.post(sql, options.statement_timeout).await?;
        // Recorded here rather than in the cursor: the id arrives with the POST's
        // answer and belongs to the session, which is what `query_id` is asked of.
        if page.id.is_some() {
            self.last_id = page.id.clone();
        }

        let affected = page
            .update_count
            .and_then(|count| u64::try_from(count).ok());
        let columns = columns_of(&page);
        let pending: VecDeque<Vec<Value>> = match (page.columns.as_deref(), page.data.as_deref()) {
            (Some(wire_columns), Some(rows)) => decode_rows(wire_columns, rows).into(),
            _ => VecDeque::new(),
        };
        let finished = page.next_uri.is_none();

        Ok(Box::new(TrinoCursor {
            client: self.client.clone(),
            credentials: self.credentials.clone(),
            catalog: self.catalog.clone(),
            schema: self.schema.clone(),
            running: Arc::clone(&self.running),
            next_uri: page.next_uri,
            columns,
            pending,
            finished,
            affected,
            row_limit: options.row_limit,
            emitted: 0,
            max_batch_rows: options.max_batch_rows,
            // The statement's own answer is the first page, and it is already in
            // `pending`, so the pause is only spent once a poll comes back empty.
            poll_pause: POLL_INTERVAL_MIN,
            timeout: options.statement_timeout,
            sql: sql.to_owned(),
        }))
    }

    async fn execute_bound(
        &mut self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        if parameters.is_empty() {
            return self.execute(sql, options).await;
        }
        // No bind parameters exist on the wire, so the only honest answers are
        // "no values" or an error. Sending `?` or `$1` would hand the coordinator
        // a placeholder as ordinary text and run a statement the caller did not
        // build — the failure mode parameter binding exists to remove.
        Err(EngineError::Usage {
            message: "Trino's HTTP protocol has no bind parameters, so this statement cannot take \
                      values; write them into the SQL text instead (a driver whose \
                      Capabilities::parameters is None must be inlined for)"
                .to_owned(),
        })
    }

    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        // `include_system` is accepted and unused, exactly as in the Python
        // engine: `SHOW CATALOGS` and `SHOW SCHEMAS` already list everything the
        // coordinator has, so "show all" is a no-op here rather than a call that
        // quietly does something different.
        let _ = include_system;
        let (catalog, schema) = self.catalog_and_schema(path);

        let sql = match level {
            BrowseLevel::Catalog => catalogs_sql(),
            BrowseLevel::Schema => schemas_sql(&catalog)?,
            BrowseLevel::Table => tables_sql(&catalog, &schema)?,
            // Trino's tree is catalog, schema, table, with no database level of its
            // own -- the Python engine mapped the `database` slot onto the catalog.
            // So this is answered with what to use instead, rather than with an
            // empty list the caller would read as an empty catalog.
            BrowseLevel::Database => {
                return Err(EngineError::Usage {
                    message: "Trino has no database level: its tree is catalog, schema, table. \
                              Browse the catalog level instead."
                        .to_owned(),
                })
            }
        };
        self.collect_names(&sql).await
    }

    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        let (catalog, schema) = self.catalog_and_schema(path);
        let sql = objects_sql(&catalog, &schema)?;
        let rows = self
            .collect(&sql)
            .await?
            .into_iter()
            .map(|row| row.iter().map(value_to_text).collect())
            .collect();

        Ok(ObjectsPage {
            columns: TrinoDriver.capabilities().objects_columns,
            rows,
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        explain_sql(sql)
    }

    fn explain_statement_with(
        &self,
        sql: &str,
        options: ExplainOptions,
    ) -> Result<String, EngineError> {
        explain_sql_with(sql, options)
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        self.delete_running().await
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        // Nothing to close: no socket is held by us. Cancelling a query that is
        // still running is deliberately NOT done here -- a user who closes a tab
        // while a query runs may well want it to finish and be cached, and
        // silently killing it would be a surprise.
        Ok(())
    }
    /// Nothing on the connection to clean: a Trino session is a client and a few strings.
    /// What a run can leave is a query nobody finished reading (a capped preview), and that
    /// is stopped here so the coordinator does not keep it.
    ///
    /// A `DELETE` the server *refuses* is not swallowed. A Trino query is not abandoned because
    /// a client stopped listening: it keeps scanning until its own run-time ceiling, holding the
    /// coordinator. Reporting a refused cancel as a clean reset would hand a session with a live
    /// straggler back to the pool to be reused, which is the leak. When the coordinator answers
    /// the cancel with an error (`code` is set), that failure is returned so the pool closes the
    /// session instead of parking it (`host/pool.rs::checkin`), and `running` is left in place
    /// rather than cleared -- there is nothing clean to forget.
    ///
    /// A cancel we deliberately did *not* send is a different thing: a `nextUri` pointing at
    /// another origin is refused before any request leaves (ADR-0040), `code` is `None`, and
    /// there is no server to blame and nothing more we may do. That stays the old behaviour --
    /// `reset` reports success, because a failure it cannot act on would only turn every
    /// pooled check-in of such a session into a needless close.
    async fn reset(&mut self) -> Result<(), EngineError> {
        if let Err(error) = self.delete_running().await {
            if matches!(&error, EngineError::Query { code: Some(_), .. }) {
                // The server refused the cancel: the query is still running there. Do not
                // clear `running`, do not report success.
                return Err(error);
            }
        }
        *self.running.lock().expect("running state") = None;
        self.last_id = None;
        Ok(())
    }

    /// The catalog is this driver's `database` slot. Empty means "the connection names none",
    /// which is what a session with no context starts with.
    fn set_context(&mut self, database: Option<&str>, schema: Option<&str>) {
        self.catalog = database.unwrap_or_default().to_owned();
        self.schema = schema.unwrap_or_default().to_owned();
    }
}

// ---------------------------------------------------------------------------
// The cursor
// ---------------------------------------------------------------------------

struct TrinoCursor {
    client: reqwest::Client,
    credentials: Credentials,
    catalog: String,
    schema: String,
    running: Shared,
    next_uri: Option<String>,
    columns: Vec<ColumnMeta>,
    pending: VecDeque<Vec<Value>>,
    finished: bool,
    /// Rows the statement wrote, when the server has said. The count arrives in the
    /// last page, so it is only meaningful once the cursor has been drained.
    affected: Option<u64>,
    row_limit: Option<usize>,
    emitted: usize,
    max_batch_rows: Option<usize>,
    /// The pause the next empty page will get. Starts at [`POLL_INTERVAL_MIN`],
    /// doubles while pages keep arriving without rows, and drops back the moment
    /// one carries rows.
    poll_pause: Duration,
    /// The bound the query runs under, so a server timeout names it.
    timeout: Option<Duration>,
    /// The statement this cursor reads, which a later page's error location counts into.
    sql: String,
}

impl TrinoCursor {
    /// Fetch one more page, appending its rows.
    async fn fetch_page(&mut self) -> Result<(), EngineError> {
        let Some(uri) = self.next_uri.clone() else {
            self.finished = true;
            return Ok(());
        };

        let response = authorized(&self.client, reqwest::Method::GET, &uri, &self.credentials)
            .map_err(Refused::query)?
            .header("X-Trino-Catalog", &self.catalog)
            .header("X-Trino-Schema", &self.schema)
            .send()
            .await
            .map_err(|error| EngineError::Query {
                message: format!("could not fetch the next page: {error}"),
                code: None,
                kind: FailureKind::Transient,
                position: None,
            })?;

        let status = response.status();
        let text = response.text().await.map_err(|error| EngineError::Query {
            message: format!("the page body could not be read: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;
        if !status.is_success() {
            return Err(EngineError::Query {
                message: format!("{status}: {}", text.trim()),
                code: Some(status.as_u16().to_string()),
                kind: FailureKind::Transient,
                position: None,
            });
        }

        let mut page: Page = serde_json::from_str(&text).map_err(|error| EngineError::Query {
            message: format!("a page was not a Trino page: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if let Some(error) = page.error {
            self.finished = true;
            return Err(map_error(error, &self.sql, self.timeout));
        }

        // The columns arrive with the first page that has rows. Taking them here
        // rather than in `execute` is what keeps `execute` from blocking.
        if self.columns.is_empty() {
            if let Some(wire_columns) = page.columns.as_deref() {
                self.columns = wire_columns
                    .iter()
                    .map(|column| ColumnMeta::new(column.name.clone(), column.type_text.clone()))
                    .collect();
            }
        }

        if let (Some(wire_columns), Some(rows)) = (page.columns.as_deref(), page.data.as_deref()) {
            self.pending.extend(decode_rows(wire_columns, rows));
        }

        // Both readings happen before anything is moved out of the page: asking
        // `running_from(&page)` afterwards would borrow a partially moved value.
        let running = running_from(&page);
        let id = page.id.take();
        // A negative count is the protocol's way of saying "nothing to report" rather
        // than -1 rows, so it is not turned into an unsigned one.
        if let Some(count) = page.update_count {
            self.affected = u64::try_from(count).ok();
        }
        self.next_uri = page.next_uri.take();

        if self.next_uri.is_none() {
            self.finished = true;
            *self.running.lock().expect("running state") = None;
        } else if let Some(running) = running {
            *self.running.lock().expect("running state") = Some(running);
        } else if let Some(id) = id {
            *self.running.lock().expect("running state") = Some(Running {
                id,
                next_uri: self.next_uri.clone().unwrap_or_default(),
            });
        }

        Ok(())
    }

    /// How many rows this call should hand back.
    fn budget(&self, max_rows: usize) -> usize {
        let asked = if max_rows == 0 { usize::MAX } else { max_rows };
        let capped = match self.max_batch_rows {
            Some(ceiling) if ceiling > 0 => asked.min(ceiling),
            _ => asked,
        };
        match self.row_limit {
            // The row limit is the caller's ceiling on the *result*, not on the
            // statement: the engine stops reading rather than rewriting the SQL to
            // add a LIMIT, so the query the user sees is the query that ran.
            Some(limit) => capped.min(limit.saturating_sub(self.emitted)),
            None => capped,
        }
    }
}

#[async_trait]
impl Cursor for TrinoCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        loop {
            let budget = self.budget(max_rows);

            if !self.pending.is_empty() && budget > 0 {
                let take = budget.min(self.pending.len());
                let rows: Vec<Vec<Value>> = self.pending.drain(..take).collect();
                self.emitted += take;

                // A page is row-major and a batch is column-major, so the
                // transpose happens here, once per batch. The width is the widest
                // row, not the first: `decode_rows` keeps a value past the declared
                // columns as `Unknown`, and cutting it here would drop it unseen
                // (the store refuses the wider batch instead). A shorter row is
                // padded with NULLs, since a ragged batch would be refused.
                let width = rows
                    .iter()
                    .map(Vec::len)
                    .fold(self.columns.len(), usize::max);
                let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(rows.len()); width];
                for row in rows {
                    let cells = row.len();
                    for (column, value) in columns.iter_mut().zip(row) {
                        column.push(value);
                    }
                    for column in &mut columns[cells..] {
                        column.push(Value::Null);
                    }
                }
                return Ok(Some(ColumnBatch::new(columns).map_err(|error| {
                    EngineError::Internal {
                        message: format!("the server's page could not form a batch: {error}"),
                    }
                })?));
            }

            if budget == 0 {
                // The row limit is reached. Stop reading rather than run the rest
                // of the query for rows nobody asked for.
                self.finished = true;
                return Ok(None);
            }

            if self.finished {
                return Ok(None);
            }

            // Nothing buffered and more to come: wait for the next page. A queued
            // page carries no rows, so the loop keeps its promise to return rows or
            // an end, rather than an empty batch the caller has to interpret.
            self.fetch_page().await?;
            if self.pending.is_empty() {
                if self.finished {
                    return Ok(None);
                }
                // Still queued or running with no rows yet. Every poll returns a
                // *new* page URI, so waiting on the URI changing would never wait
                // and a slow query would be polled in a tight loop; the pause is
                // what makes this a poll rather than a spin. It grows while the
                // pages stay empty and is reset by the first one that is not, so a
                // query that is producing is not held to the rate of a query that
                // is still planning.
                tokio::time::sleep(self.poll_pause).await;
                self.poll_pause = (self.poll_pause * 2).min(POLL_INTERVAL_MAX);
            } else {
                self.poll_pause = POLL_INTERVAL_MIN;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pages into batches
// ---------------------------------------------------------------------------

fn columns_of(page: &Page) -> Vec<ColumnMeta> {
    page.columns
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|column| ColumnMeta::new(column.name.clone(), column.type_text.clone()))
        .collect()
}

fn decode_rows(columns: &[WireColumn], rows: &[Vec<Json>]) -> Vec<Vec<Value>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(index, cell)| match columns.get(index) {
                    Some(column) => decode::decode(&column.type_text, cell),
                    // More values than the page declared columns: keep the value
                    // rather than drop it, because dropping is invisible.
                    None => Value::unknown("unknown", cell.to_string()),
                })
                .collect()
        })
        .collect()
}

/// Cell text for the objects grid and for tree names.
///
/// This grid is a listing, not data, so a NULL is the empty string and every value
/// is rendered rather than carried.
///
/// The rendering itself is `qh-core`'s, which is the point: a `varbinary` is hex
/// here, in the grid, and in an export alike, and there is one place that decides
/// it. An earlier draft of this file had its own copy, along with its own decimal
/// formatter -- a second answer to a question that already had one.
fn value_to_text(value: &Value) -> String {
    qh_core::to_text(value).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The SQL, which is the same SQL the Python engine sent
// ---------------------------------------------------------------------------

/// Quote one name the way Trino does: double quotes, doubled inside.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn catalogs_sql() -> String {
    // Every catalog the coordinator has, system ones included: there is no system
    // set to hide here, which is why `include_system` is a no-op for Trino.
    "SHOW CATALOGS".to_owned()
}

fn schemas_sql(catalog: &str) -> Result<String, EngineError> {
    if catalog.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog is required to list schemas; set the connection's database or \
                      pass the catalog level first"
                .to_owned(),
        });
    }
    Ok(format!("SHOW SCHEMAS FROM {}", quote(catalog)))
}

fn tables_sql(catalog: &str, schema: &str) -> Result<String, EngineError> {
    if catalog.is_empty() || schema.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog and a schema are required to list tables".to_owned(),
        });
    }
    Ok(format!(
        "SHOW TABLES FROM {}.{}",
        quote(catalog),
        quote(schema)
    ))
}

fn objects_sql(catalog: &str, schema: &str) -> Result<String, EngineError> {
    if catalog.is_empty() || schema.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog and a schema are required to list objects".to_owned(),
        });
    }
    // `information_schema.tables` rather than `SHOW TABLES`: both list the same
    // tables, but only this one carries the object's type, which is what makes the
    // grid worth more than the tree beside it.
    Ok(format!(
        "SELECT table_name, table_type FROM {}.information_schema.tables \
         WHERE table_schema = {} ORDER BY 1",
        quote(catalog),
        literal(schema)
    ))
}

/// `EXPLAIN` in front of the statement, with the caller's terminator dropped.
///
/// Trino's plan comes back as one text column, one row per plan line — unlike
/// Postgres, which returns a single `QUERY PLAN` text column holding the whole
/// plan. A free function so it can be asserted without standing up a session.
///
/// The `;` goes before the statement is sent, exactly as it does for PostgreSQL
/// and MySQL and through the same `strip_terminator`: measured on 483,
/// `EXPLAIN SELECT 1;` is answered `SYNTAX_ERROR: line 1:39: mismatched input ';'`,
/// so a caller who ended their statement the way SQL allows got a syntax error for
/// it. Only a real terminator goes — a `;` inside a literal is data.
fn explain_sql(sql: &str) -> String {
    format!("EXPLAIN {}", strip_terminator_dialect(sql, Dialect::Trino))
}

/// [`explain_sql`] under `options`.
///
/// `TYPE DISTRIBUTED` is written out although it is the server's default (measured on 483), so
/// a release that changes the default does not change the shape the app parses. ANALYZE is the
/// text form only; a caller that did not pass the options through
/// [`ExplainSupport::resolve`] and asks for JSON with it is told so.
fn explain_sql_with(sql: &str, options: ExplainOptions) -> Result<String, EngineError> {
    let statement = strip_terminator_dialect(sql, Dialect::Trino);
    match (options.analyze, options.format) {
        (false, ExplainFormat::Text) => Ok(format!("EXPLAIN {statement}")),
        (false, ExplainFormat::Json) => Ok(format!(
            "EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) {statement}"
        )),
        (true, ExplainFormat::Text) => Ok(format!("EXPLAIN ANALYZE {statement}")),
        (true, ExplainFormat::Json) => Err(EngineError::Usage {
            message: "Trino has no JSON form of EXPLAIN ANALYZE".to_owned(),
        }),
    }
}

/// A string literal with single quotes doubled, which is SQL's own escaping.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_say_what_this_driver_is() {
        let capabilities = TrinoDriver.capabilities();
        assert!(
            !capabilities.persistent_connection,
            "there is no socket to keep"
        );
        assert!(!capabilities.transactions);
        assert!(capabilities.cancel, "DELETE reaches the server");
        assert!(
            capabilities.statement_timeout,
            "query_max_run_time is the server's own bound"
        );
        assert_eq!(
            capabilities.levels,
            vec![
                BrowseLevel::Catalog,
                BrowseLevel::Schema,
                BrowseLevel::Table
            ]
        );
        assert_eq!(capabilities.objects_columns, vec!["Name", "Type"]);
        assert_eq!(
            capabilities.parameters, None,
            "the HTTP protocol has no bind parameters, so a value must be inlined"
        );
        assert!(
            !capabilities.read_only,
            "this engine can write when the server allows"
        );
    }

    #[test]
    fn the_metadata_sql_matches_the_python_engine() {
        // These strings decide what the tree and grid show, so they are pinned:
        // the Rust engine has to answer with the same shape the Python engine did.
        assert_eq!(catalogs_sql(), "SHOW CATALOGS");
        assert_eq!(schemas_sql("hive").unwrap(), "SHOW SCHEMAS FROM \"hive\"");
        assert_eq!(
            tables_sql("hive", "default").unwrap(),
            "SHOW TABLES FROM \"hive\".\"default\""
        );
        assert_eq!(
            objects_sql("hive", "default").unwrap(),
            "SELECT table_name, table_type FROM \"hive\".information_schema.tables \
             WHERE table_schema = 'default' ORDER BY 1"
        );
    }

    #[test]
    fn a_name_is_quoted_the_way_trino_quotes_it() {
        assert_eq!(quote("orders"), "\"orders\"");
        assert_eq!(
            quote("ORDER"),
            "\"ORDER\"",
            "case is preserved, never folded"
        );
        // A quote inside a name is doubled, not escaped with a backslash.
        assert_eq!(quote("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(literal("o'brien"), "'o''brien'");
    }

    #[test]
    fn listing_without_the_level_above_is_refused_with_a_usable_message() {
        // Not an empty list and not a query that fails on the server: the caller
        // is told which setting to fill in.
        assert!(matches!(schemas_sql(""), Err(EngineError::Usage { .. })));
        assert!(matches!(
            tables_sql("hive", ""),
            Err(EngineError::Usage { .. })
        ));
        assert!(matches!(
            objects_sql("", "default"),
            Err(EngineError::Usage { .. })
        ));
    }

    #[test]
    fn explain_is_the_servers_own_plan_without_the_callers_terminator() {
        assert_eq!(explain_sql("SELECT 1"), "EXPLAIN SELECT 1");
        // Trino rejects `EXPLAIN SELECT 1;` with a SYNTAX_ERROR on the `;`
        // (measured on 483, `line 1:39`). The caller wrote SQL, so the terminator
        // goes -- and only a real one: `strip_terminator` is what knows a `;`
        // inside a literal is data rather than a separator.
        assert_eq!(explain_sql("SELECT 1;"), "EXPLAIN SELECT 1");
        assert_eq!(explain_sql("SELECT 1;  \n"), "EXPLAIN SELECT 1");
        assert_eq!(explain_sql("SELECT 'a;b;'"), "EXPLAIN SELECT 'a;b;'");
        assert_eq!(
            explain_sql("SELECT 1; -- trailing;"),
            "EXPLAIN SELECT 1 -- trailing"
        );
        // Two statements: the caller asked about both, and stripping would change
        // the question rather than tidy it.
        assert_eq!(
            explain_sql("SELECT 1; SELECT 2"),
            "EXPLAIN SELECT 1; SELECT 2"
        );
    }

    #[test]
    fn the_announced_capability_is_the_one_that_keeps_microseconds() {
        // The coordinator reads this header to decide how to encode `timestamp`
        // and `time`: without it, the same query comes back typed `timestamp` and
        // rounded to milliseconds. Trino 483 acts on `PARAMETRIC_DATETIME` and
        // ignores the names it does not know (measured: `BOGUS` alone changes
        // nothing, `BOGUS,PARAMETRIC_DATETIME` still buys microseconds), so this
        // one name is the whole announcement.
        assert_eq!(CLIENT_CAPABILITIES_HEADER, "X-Trino-Client-Capabilities");
        assert_eq!(CLIENT_CAPABILITIES, "PARAMETRIC_DATETIME");
    }

    #[test]
    fn a_users_error_is_permanent_and_a_resource_error_is_not() {
        // The distinction decides whether a retry loop runs. Retrying a syntax
        // error produces the same syntax error forever.
        let user = map_error(
            WireError {
                message: "mismatched input".to_owned(),
                error_code: Some(1),
                error_name: Some("SYNTAX_ERROR".to_owned()),
                error_type: Some("USER_ERROR".to_owned()),
                error_location: None,
            },
            "",
            None,
        );
        match user {
            EngineError::Query {
                kind,
                code,
                message,
                ..
            } => {
                assert_eq!(kind, FailureKind::Permanent);
                assert_eq!(code.as_deref(), Some("1"));
                assert!(message.contains("SYNTAX_ERROR"), "{message}");
            }
            other => panic!("expected a query error, got {other:?}"),
        }

        for error_type in ["INTERNAL_ERROR", "INSUFFICIENT_RESOURCES"] {
            let error = map_error(
                WireError {
                    message: "busy".to_owned(),
                    error_code: Some(65537),
                    error_name: None,
                    error_type: Some(error_type.to_owned()),
                    error_location: None,
                },
                "",
                None,
            );
            match error {
                EngineError::Query { kind, .. } => assert_eq!(kind, FailureKind::Transient),
                other => panic!("expected a query error, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_error_location_becomes_a_scalar_offset_into_the_statement() {
        // The shape Trino sends: `errorLocation` beside the error, 1-based, columns
        // counted in code points.
        let wire = || {
            serde_json::from_str::<Page>(
                r#"{"id":"q","stats":{},"error":{"message":"line 2:6: mismatched input 'FRM'",
                   "errorCode":1,"errorName":"SYNTAX_ERROR","errorType":"USER_ERROR",
                   "errorLocation":{"lineNumber":2,"columnNumber":6}}}"#,
            )
            .expect("a failed page")
            .error
            .expect("an error")
        };
        let sql = "SELECT '\u{1F600}'\n  x FRM t";
        let error = map_error(wire(), sql, None);
        // Line 1 is ten scalars and a newline, so column 6 of line 2 is the 17th.
        assert_eq!(error.position(), Some(17), "{error:?}");
        assert_eq!(error.code(), Some("1"));
        // Without the text, or with text that has no such place, there is no mark.
        assert_eq!(map_error(wire(), "", None).position(), None);
        assert_eq!(map_error(wire(), "SELECT 1", None).position(), None);
    }

    #[test]
    fn a_cancelled_query_is_a_cancellation_and_not_a_user_error() {
        // The server's own answer to a successful cancel, quoted exactly as Trino
        // 483 sent it. `errorType` says USER_ERROR, which is why the name is read
        // first: the user asked for this, so the UI must not say it failed.
        let error = map_error(
            WireError {
                message: "Query was canceled".to_owned(),
                error_code: Some(3),
                error_name: Some("USER_CANCELED".to_owned()),
                error_type: Some("USER_ERROR".to_owned()),
                error_location: None,
            },
            "",
            None,
        );
        match error {
            EngineError::Query {
                kind,
                code,
                message,
                ..
            } => {
                assert_eq!(kind, FailureKind::Cancelled);
                assert_eq!(code.as_deref(), Some("3"));
                assert!(message.contains("canceled"), "{message}");
            }
            other => panic!("expected a query error, got {other:?}"),
        }
    }

    #[test]
    fn a_timeout_is_typed_and_names_the_limit_in_force() {
        // Measured against Trino 483: `query_max_run_time=2s` produced this error on a
        // page, `EXCEEDED_TIME_LIMIT` rather than the name the plan predicted. Both
        // names are accepted, and the message keeps the coordinator's own sentence.
        for name in ["EXCEEDED_TIME_LIMIT", "QUERY_EXCEEDED_MAX_EXECUTION_TIME"] {
            let error = map_error(
                WireError {
                    message: "Query exceeded maximum time limit of 2.00s".to_owned(),
                    error_code: Some(131075),
                    error_name: Some(name.to_owned()),
                    error_type: Some("INSUFFICIENT_RESOURCES".to_owned()),
                    error_location: None,
                },
                "",
                Some(Duration::from_millis(2_000)),
            );
            match &error {
                EngineError::Timeout { message, limit_ms } => {
                    assert_eq!(*limit_ms, Some(2_000), "{name}");
                    assert!(message.contains("2000 ms"), "{name}: {message}");
                    assert!(
                        message.contains("Query exceeded maximum time limit"),
                        "the coordinator's own sentence is kept: {message}"
                    );
                }
                other => panic!("{name} should be a timeout, got {other:?}"),
            }
            // Not retried: the bound is the caller's.
            assert_eq!(error.failure_kind(), FailureKind::Permanent);
        }
    }

    #[test]
    fn the_session_header_carries_the_bound_in_milliseconds() {
        assert_eq!(session_header(None), None);
        assert_eq!(
            session_header(Some(Duration::from_millis(1_500))).as_deref(),
            Some("query_max_run_time=1500ms")
        );
        assert_eq!(
            session_header(Some(Duration::from_secs(60))).as_deref(),
            Some("query_max_run_time=60000ms")
        );
    }

    #[test]
    fn a_page_with_no_columns_is_not_a_failure_to_decode() {
        // The measured sequence: the first pages are QUEUED and carry no columns
        // and no data at all. Requiring either would reject a correct response.
        let page: Page = serde_json::from_str(
            r#"{"id":"20260922_033254_00000_xc6p3","nextUri":"http://x/next","stats":{"state":"QUEUED"}}"#,
        )
        .expect("a queued page is valid");
        assert!(page.columns.is_none());
        assert!(page.data.is_none());
        assert!(columns_of(&page).is_empty());
        // The page also carried `"stats":{"state":"QUEUED"}`, which is deliberately
        // not modelled. Its being ignored rather than rejected is part of what this
        // test pins: an unmodelled key must not fail a correct response.
        assert!(page.next_uri.is_some());
    }

    #[test]
    fn a_finished_page_has_no_next_uri() {
        let page: Page = serde_json::from_str(
            r#"{"id":"q","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#,
        )
        .expect("a final page is valid");
        assert!(page.next_uri.is_none());
        assert!(running_from(&page).is_none(), "nothing left to cancel");
        assert_eq!(columns_of(&page).len(), 1);
    }

    #[test]
    fn a_grid_cell_is_the_text_form_and_a_null_is_empty() {
        // The grid is a listing, not data: every value is rendered, and a NULL is
        // the empty string rather than a missing cell.
        assert_eq!(value_to_text(&Value::Null), "");
        assert_eq!(value_to_text(&Value::Int(42)), "42");
        // Hex, because that is `qh-core`'s answer for bytes, and it is the same one
        // an export gives.
        assert_eq!(value_to_text(&Value::Bytes(vec![0x00, 0xff])), "00ff");
        assert_eq!(
            value_to_text(&Value::Timestamp {
                micros: 0,
                offset_secs: None
            }),
            "1970-01-01 00:00:00"
        );
    }

    #[test]
    fn a_page_becomes_a_column_major_batch() {
        let columns = vec![
            WireColumn {
                name: "n".to_owned(),
                type_text: "bigint".to_owned(),
            },
            WireColumn {
                name: "t".to_owned(),
                type_text: "varchar(1)".to_owned(),
            },
        ];
        let rows = vec![
            vec![Json::from(1), Json::from("a")],
            vec![Json::from(2), Json::from("b")],
        ];
        let decoded = decode_rows(&columns, &rows);
        assert_eq!(
            decoded,
            vec![
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Text("b".into())],
            ]
        );
    }

    /// A cursor holding already decoded rows, for the page-to-batch step alone.
    fn cursor_over(columns: &[&str], rows: &[Vec<Json>]) -> TrinoCursor {
        let wire: Vec<WireColumn> = columns
            .iter()
            .map(|name| WireColumn {
                name: (*name).to_owned(),
                type_text: "bigint".to_owned(),
            })
            .collect();
        TrinoCursor {
            client: client_for(TlsMode::Disable, None).expect("client"),
            credentials: Credentials::new("t", None),
            catalog: String::new(),
            schema: String::new(),
            running: Arc::new(Mutex::new(None)),
            next_uri: None,
            columns: wire
                .iter()
                .map(|column| ColumnMeta::new(column.name.clone(), column.type_text.clone()))
                .collect(),
            pending: decode_rows(&wire, rows).into(),
            finished: true,
            affected: None,
            row_limit: None,
            emitted: 0,
            max_batch_rows: None,
            poll_pause: POLL_INTERVAL_MIN,
            timeout: None,
            sql: String::new(),
        }
    }

    #[tokio::test]
    async fn a_ragged_row_is_padded_or_kept_never_cut() {
        let ints = |values: &[i64]| values.iter().map(|v| Json::from(*v)).collect::<Vec<_>>();
        // A later row wider than the first used to lose its extra value.
        let mut wide = cursor_over(&["a", "b"], &[ints(&[1, 2]), ints(&[3, 4, 5])]);
        let batch = wide.next_batch(0).await.unwrap().unwrap();
        assert_eq!(
            batch.width(),
            3,
            "the extra value stays, and the store refuses the width"
        );
        assert!(matches!(batch.value(1, 2), Some(Value::Unknown { .. })));
        assert_eq!(batch.value(0, 2), Some(&Value::Null));
        // A shorter row used to fail the whole batch unless a column was empty.
        let mut short = cursor_over(&["a", "b"], &[ints(&[1, 2]), ints(&[3])]);
        let batch = short.next_batch(0).await.unwrap().unwrap();
        assert_eq!((batch.width(), batch.rows()), (2, 2));
        assert_eq!(batch.value(1, 1), Some(&Value::Null));
    }

    #[test]
    fn a_mode_starts_on_the_scheme_it_means() {
        // `Prefer` starts on HTTPS and has HTTP only as the one fallback it is
        // allowed; the other three have one scheme each. This is the mapping the
        // whole module's TLS section describes, pinned where it can be read.
        assert_eq!(scheme_for(TlsMode::Disable), "http");
        assert_eq!(scheme_for(TlsMode::Prefer), "https");
        assert_eq!(scheme_for(TlsMode::Require), "https");
        assert_eq!(scheme_for(TlsMode::RequireNoVerify), "https");
    }

    #[test]
    fn a_downgrade_changes_only_the_scheme() {
        // The host and the port are exactly what the user configured: a fallback
        // that quietly moved the address would be a different connection, not the
        // same one without TLS.
        assert_eq!(
            with_scheme("http", "https://coordinator.internal:8443"),
            "http://coordinator.internal:8443"
        );
    }

    fn origin(url: &str) -> Origin {
        Origin::of(url).unwrap_or_else(|why| panic!("{url}: {why}"))
    }

    #[test]
    fn an_origin_is_scheme_host_and_port_and_nothing_else() {
        // Case does not make a different host.
        assert_eq!(
            origin("https://Trino.Corp/v1/x"),
            origin("https://trino.corp/v1/y")
        );
        // The scheme's own port is the same as no port at all.
        assert_eq!(origin("https://h"), origin("https://h:443/p?q=1"));
        assert_eq!(origin("http://h"), origin("http://h:80"));
        // A port is part of the identity, and so is the scheme: `http://h:443` is
        // not `https://h:443`, which is the whole point of the check.
        assert_ne!(origin("https://h:443"), origin("https://h:8443"));
        assert_ne!(origin("http://h:443"), origin("https://h:443"));
        // `localhost` and `127.0.0.1` are two hosts as far as a credential goes.
        assert_ne!(origin("https://localhost:1"), origin("https://127.0.0.1:1"));
        // IPv6 keeps its brackets, so it is one host in any spelling.
        assert_eq!(
            origin("https://[::1]:8443/x"),
            origin("https://[0:0:0:0:0:0:0:1]:8443")
        );
        assert_eq!(origin("https://h:8443/x").to_string(), "https://h:8443");
        // What is not an http(s) URL with a host has no origin.
        for bad in [
            "",
            "not a url",
            "/v1/statement/x",
            "ftp://h/x",
            "https://:443",
        ] {
            assert!(Origin::of(bad).is_err(), "{bad:?}");
        }
    }

    fn credentials_for(session: &str, password: Option<&str>, bearer: Option<&str>) -> Credentials {
        let mut credentials = Credentials::new("u", password.map(str::to_owned));
        credentials.bearer = bearer.map(str::to_owned);
        credentials.origin = Some(origin(session));
        credentials
    }

    #[test]
    fn a_secret_is_only_sent_to_the_address_the_session_uses() {
        let basic = credentials_for("https://trino.corp:443", Some("pw"), None);
        assert_eq!(
            basic.refusal_for("https://trino.corp/v1/statement/q/1"),
            None
        );
        // The scheme, the host and the port each refuse on their own.
        for foreign in [
            "http://trino.corp:443/v1/statement/q/1",
            "https://other.corp/v1/statement/q/1",
            "https://trino.corp:8443/v1/statement/q/1",
            "not a url",
        ] {
            assert!(basic.refusal_for(foreign).is_some(), "{foreign}");
        }
        // The message names both addresses, and carries neither a path nor a secret.
        let message = basic
            .refusal_for("http://trino.corp:8080/v1/statement/q/1?token=abc")
            .expect("refused");
        assert!(message.contains("http://trino.corp:8080"), "{message}");
        assert!(message.contains("https://trino.corp:443"), "{message}");
        assert!(!message.contains("/v1/statement"), "{message}");
        assert!(
            !message.contains("abc") && !message.contains("pw"),
            "{message}"
        );
    }

    #[test]
    fn a_secret_without_an_address_on_record_is_not_sent_anywhere() {
        let mut credentials = Credentials::new("u", Some("pw".to_owned()));
        assert!(credentials.refusal_for("https://h/x").is_some());
        credentials.password = None;
        credentials.bearer = Some("tok".to_owned());
        assert!(credentials.refusal_for("https://h/x").is_some());
    }

    #[test]
    fn a_session_with_no_secret_is_not_checked_at_all() {
        // Nothing can leak, and a coordinator without authentication that announces
        // another address works today, so it keeps working.
        let open = Credentials::new("u", None);
        assert_eq!(open.refusal_for("http://anywhere:1/x"), None);
        assert_eq!(open.refusal_for("not even a url"), None);
    }

    #[test]
    fn a_bearer_token_needs_https_even_to_its_own_address() {
        let over_http = credentials_for("http://trino.corp:8080", None, Some("tok"));
        let message = over_http
            .refusal_for("http://trino.corp:8080/v1/statement")
            .expect("a token never goes over http");
        assert!(message.contains("only sent over https"), "{message}");
        // A password over http is the existing, config-guarded behaviour, unchanged.
        let basic = credentials_for("http://trino.corp:8080", Some("pw"), None);
        assert_eq!(
            basic.refusal_for("http://trino.corp:8080/v1/statement"),
            None
        );
        let over_https = credentials_for("https://trino.corp:443", None, Some("tok"));
        assert_eq!(
            over_https.refusal_for("https://trino.corp/v1/statement"),
            None
        );
    }

    #[test]
    fn credentials_never_print_a_secret() {
        let both = credentials_for("https://h", Some("hunter2"), Some("SENTINEL-TOKEN"));
        let shown = format!("{both:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("SENTINEL"),
            "{shown}"
        );
        assert!(
            shown.contains("password: present") && shown.contains("bearer: present"),
            "{shown}"
        );
    }

    #[test]
    fn a_refused_request_is_permanent_and_is_not_built() {
        let client = client_for(TlsMode::Disable, None).expect("client");
        let credentials = credentials_for("https://trino.corp:443", Some("pw"), None);
        let refused = authorized(
            &client,
            reqwest::Method::GET,
            "http://trino.corp:80/x",
            &credentials,
        )
        .expect_err("refused");
        // As a poll or a cancel it is a query failure, as the statement it is a
        // connect failure; neither is ever retried.
        assert!(matches!(
            Refused(refused.0.clone()).query(),
            EngineError::Query {
                code: None,
                kind: FailureKind::Permanent,
                ..
            }
        ));
        assert!(matches!(
            refused.connect(),
            EngineError::Connect {
                kind: FailureKind::Permanent,
                ..
            }
        ));
    }

    #[test]
    fn only_authorized_attaches_a_credential() {
        // The promise in the module note is that one function decides where a secret
        // goes. Counting makes it true for the next request path someone adds: it
        // would have to build its own `basic_auth`, and this test would see it.
        let source = include_str!("lib.rs");
        let code = source.split("#[cfg(test)]").next().expect("the code half");
        let code: String = code
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let body_start = code.find("fn authorized(").expect("authorized exists");
        let body_end = body_start + code[body_start..].find("\n}\n").expect("its end");
        let outside = format!("{}{}", &code[..body_start], &code[body_end..]);
        // Not vacuous: the two calls exist, inside the function.
        assert!(code.contains(".basic_auth(") && code.contains(".bearer_auth("));
        for needle in [
            ".basic_auth(",
            ".bearer_auth(",
            "\"Authorization\"",
            "AUTHORIZATION",
        ] {
            assert!(
                !outside.contains(needle),
                "{needle} is used outside `authorized`"
            );
        }
    }

    #[test]
    fn an_ipv6_literal_is_bracketed_in_a_url() {
        assert_eq!(authority("coordinator.corp", 443), "coordinator.corp:443");
        assert_eq!(authority("127.0.0.1", 8080), "127.0.0.1:8080");
        assert_eq!(authority("::1", 8080), "[::1]:8080");
        assert_eq!(authority("[::1]", 8080), "[::1]:8080");
        // And the address a TLS name resolves to is read through the brackets.
        assert_eq!(
            loopback_of("[::1]").expect("an address"),
            "::1".parse::<IpAddr>().unwrap()
        );
        assert!(loopback_of("db.internal").is_err());
    }

    #[tokio::test]
    async fn a_token_is_refused_at_connect_when_it_could_travel_in_clear() {
        let base = ConnectionConfig::new(DriverKind::Trino, "trino.corp", 443, "qh");
        for mode in [TlsMode::Disable, TlsMode::Prefer] {
            let error = TrinoDriver::new()
                .connect(&base.clone().bearer("tok").tls(mode))
                .await
                .err()
                .expect("a token needs a mode that is always TLS");
            assert!(
                matches!(error, EngineError::Usage { .. }),
                "{mode:?}: {error:?}"
            );
        }
        let error = TrinoDriver::new()
            .connect(
                &base
                    .clone()
                    .bearer("tok")
                    .password("pw")
                    .tls(TlsMode::RequireNoVerify),
            )
            .await
            .err()
            .expect("one or the other");
        assert!(matches!(error, EngineError::Usage { .. }), "{error:?}");
        // The two modes that are always TLS connect (nothing is sent until a statement).
        for mode in [TlsMode::Require, TlsMode::RequireNoVerify] {
            assert!(
                TrinoDriver::new()
                    .connect(&base.clone().bearer("tok").tls(mode))
                    .await
                    .is_ok(),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn every_mode_can_build_its_client() {
        // The client is built once per session from the mode, so a mode that cannot
        // build one is a mode that cannot connect at all. `Require` and `Prefer`
        // reach the platform trust store here, which is what a real connection does.
        for mode in [
            TlsMode::Disable,
            TlsMode::Prefer,
            TlsMode::Require,
            TlsMode::RequireNoVerify,
        ] {
            assert!(
                client_for(mode, None).is_ok(),
                "{mode:?} should build a client"
            );
            assert!(
                client_for_secret(mode, None).is_ok(),
                "{mode:?} should build a client that holds a secret"
            );
        }
    }

    #[test]
    fn explain_is_spelled_per_option_and_keeps_its_terminator_rule() {
        let spelled =
            |format, analyze, sql: &str| explain_sql_with(sql, ExplainOptions { format, analyze });
        assert_eq!(
            spelled(ExplainFormat::Text, false, "SELECT 1;").unwrap(),
            "EXPLAIN SELECT 1"
        );
        assert_eq!(
            spelled(ExplainFormat::Json, false, "SELECT 'a;b';").unwrap(),
            "EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) SELECT 'a;b'"
        );
        assert_eq!(
            spelled(ExplainFormat::Text, true, "SELECT 1;\n").unwrap(),
            "EXPLAIN ANALYZE SELECT 1"
        );
        // The grammar has no such form, so it is refused rather than sent.
        assert!(spelled(ExplainFormat::Json, true, "SELECT 1").is_err());
        let support = TrinoDriver.explain_support();
        assert!(support.json && support.analyze && !support.analyze_json);
        // No transaction to put an ANALYZE in.
        assert!(support.analyze_fence.is_none());
        assert!(!TrinoDriver.capabilities().transactions);
    }
}
