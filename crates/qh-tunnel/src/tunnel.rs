//! The tunnel: a bastion connection, the host-key check, and a local listener.
//!
//! The order in [`Tunnel::open`] is the point of the whole crate: the server's key is
//! checked **during the key exchange, before anything authenticating is sent**. A
//! bastion that cannot be identified never gets a public key signature or a password
//! from us. `russh` gives exactly that ordering through `Handler::check_server_key`,
//! which runs while the transport keys are still being agreed.
//!
//! Trust on first use is split across two calls rather than hidden in a callback,
//! because the decision belongs to a person and this crate must not wait on one:
//!
//! 1. `open` with [`HostKeyPolicy::Strict`] refuses an unknown host with
//!    [`Error::HostKeyUnknown`], which carries the fingerprint to show and the exact
//!    [`ServerKey`] that would be accepted;
//! 2. the caller asks, and on "yes" calls [`crate::known_hosts::append`] and calls
//!    `open` again — or, when the file is not ours to write, passes that same key back
//!    as [`HostKeyPolicy::TrustNew`], which accepts that one key and nothing else.
//!
//! Neither path can accept a host key on its own: there is no `AcceptAnywhere`, and the
//! handler's default in `russh` (reject) is the one that would apply if it were ever
//! missed.
//!
//! Reading an error, one thing about `russh`'s session loop is worth knowing: an error
//! this crate raises during the handshake comes back as itself, so
//! [`Error::HostKeyUnknown`], [`Error::HostKeyMismatch`] and [`Error::HostKeyRevoked`]
//! reach the caller intact. A connection the *server* drops — `MaxStartups`, a host key
//! the server refuses — comes back as [`Error::Ssh`] wrapping `russh::Error::Disconnect`
//! with nothing more specific, because that is all the transport was told; the reason is
//! in the server's log, not in the protocol.
//!
//! The connection is bounded in time: [`BastionConfig::connect_timeout`] covers the TCP
//! connect, the version exchange and the key exchange with its host-key check, so an
//! address that drops packets or a port that accepts and says nothing ends in
//! [`Error::ConnectTimeout`] instead of whatever the OS gives up after (minutes).
//! Authentication is outside that limit on purpose: an agent may be waiting on a person.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::client::AuthResult;
use russh::client::{self, Config};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::Algorithm;
use russh::keys::{decode_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{kex, MethodSet, Preferred};
use secrecy::{ExposeSecret, SecretString};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::known_hosts::{self, HostKeyVerdict, Origin, StoreFile};
use crate::{Error, ServerKey};

/// How the server is told who we are.
#[derive(Debug)]
pub enum Auth {
    /// A private key file, with its passphrase if it has one.
    Key {
        path: PathBuf,
        passphrase: Option<SecretString>,
    },
    /// Whatever `ssh-agent` already holds. `SSH_AUTH_SOCK` must be set.
    Agent,
    /// A password for the bastion account.
    Password(SecretString),
}

/// How long [`Tunnel::open`] waits for the bastion to connect and finish the key exchange.
///
/// Long enough for a slow link and a 4096-bit RSA host key, short enough that a wrong
/// address does not look like a hung app. A caller that knows better sets
/// [`BastionConfig::connect_timeout`].
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// What to do when the bastion's key is not in `known_hosts`.
#[derive(Debug, Clone)]
pub enum HostKeyPolicy {
    /// Refuse it and report the fingerprint. The caller decides what happens next.
    Strict,
    /// Accept this one key and no other: the answer to a previous refusal, replayed.
    TrustNew(ServerKey),
    /// Trust the key whose `SHA256:…` fingerprint this is, if and only if the server
    /// presents exactly it and the host is unknown. The key is **recorded first**, in
    /// [`BastionConfig::record_to`], and only then accepted. Needs `record_to`.
    ///
    /// The fingerprint must come from the person who saw it, for this one attempt:
    /// never from a config file or a saved setting.
    TrustFingerprint(String),
}

/// Where the bastion is, how to authenticate to it, and what to check it against.
#[derive(Debug)]
pub struct BastionConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    /// The `known_hosts` files to check against, read only and never written: the
    /// user's first, then the system's ([`BastionConfig::with_system_known_hosts`]).
    /// Read by this crate rather than by a transport library, which is why the app
    /// ships without a sandbox (ADR-0007).
    pub known_hosts: Vec<PathBuf>,
    /// The app's own file: checked like the others, and the only one ever written, and
    /// only by [`HostKeyPolicy::TrustFingerprint`].
    pub record_to: Option<PathBuf>,
    pub host_key_policy: HostKeyPolicy,
    /// The time allowed for the TCP connect and the key exchange, host-key check
    /// included; [`DEFAULT_CONNECT_TIMEOUT`] unless changed.
    pub connect_timeout: Duration,
}

impl BastionConfig {
    /// A strict configuration: the host key must already be recorded.
    #[must_use]
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        auth: Auth,
        known_hosts: impl Into<PathBuf>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            auth,
            known_hosts: vec![known_hosts.into()],
            record_to: None,
            host_key_policy: HostKeyPolicy::Strict,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// Also read `/etc/ssh/ssh_known_hosts`, so a host pinned by an administrator is
    /// not "unknown" here.
    #[must_use]
    pub fn with_system_known_hosts(mut self) -> Self {
        self.known_hosts
            .push(PathBuf::from(known_hosts::SYSTEM_KNOWN_HOSTS));
        self
    }

    /// Where an accepted key is recorded: the app's own file.
    #[must_use]
    pub fn record_to(mut self, path: impl Into<PathBuf>) -> Self {
        self.record_to = Some(path.into());
        self
    }

    /// Accept the unknown host key with exactly this fingerprint, recording it first.
    #[must_use]
    pub fn trust_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.host_key_policy = HostKeyPolicy::TrustFingerprint(fingerprint.into());
        self
    }

    /// Give up on the bastion after `timeout` instead of [`DEFAULT_CONNECT_TIMEOUT`].
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Every file the check reads, with whose it is.
    fn store_files(&self) -> Vec<StoreFile> {
        let mut files: Vec<StoreFile> = self
            .known_hosts
            .iter()
            .map(|path| {
                let origin = if path == Path::new(known_hosts::SYSTEM_KNOWN_HOSTS) {
                    Origin::System
                } else {
                    Origin::User
                };
                StoreFile::new(path, origin)
            })
            .collect();
        if let Some(path) = &self.record_to {
            files.push(StoreFile::new(path, Origin::App));
        }
        files
    }

    /// Accept one specific key that is not recorded yet, and nothing else.
    #[must_use]
    pub fn trust_new(mut self, key: ServerKey) -> Self {
        self.host_key_policy = HostKeyPolicy::TrustNew(key);
        self
    }
}

/// Where the tunnel sends what it receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// A running tunnel. Dropping it stops forwarding; [`Tunnel::close`] also waits.
pub struct Tunnel {
    local_port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    /// A second handle on the bastion connection, kept only to ask whether it is still open.
    /// It is dropped with the tunnel, after the accept loop has let go of its own.
    session: Arc<Mutex<client::Handle<HostKeyVerifier>>>,
}

impl std::fmt::Debug for Tunnel {
    // By hand: the session handle has no `Debug`, and the port is what a log line wants.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Tunnel")
            .field("local_port", &self.local_port)
            .finish_non_exhaustive()
    }
}

impl Tunnel {
    /// Open an SSH connection to the bastion and listen on `127.0.0.1` for connections
    /// to be forwarded to `target`.
    ///
    /// The listener is bound to the loopback address on purpose: it is a port that
    /// reaches a database the caller is authenticated to, and binding it anywhere else
    /// would hand that access to the network.
    ///
    /// # Errors
    /// [`Error::HostKeyUnknown`], [`Error::HostKeyMismatch`], [`Error::HostKeyRevoked`],
    /// [`Error::HostCertificateUnsupported`] from the key check;
    /// [`Error::ConnectTimeout`] if the handshake did not finish in
    /// [`BastionConfig::connect_timeout`];
    /// [`Error::AuthenticationRejected`] if no authentication method worked.
    pub async fn open(config: &BastionConfig, target: Target) -> Result<Self, Error> {
        // Everything that can be refused before the network is touched, is.
        if let HostKeyPolicy::TrustFingerprint(pin) = &config.host_key_policy {
            if !is_fingerprint(pin) {
                return Err(Error::Usage(
                    "the host key to trust must be a SHA256:<43 base64 characters> fingerprint",
                ));
            }
            if config.record_to.is_none() {
                return Err(Error::Usage(
                    "a host key cannot be trusted without a file to record it in",
                ));
            }
        }
        let files = config.store_files();
        let recorded = known_hosts::recorded_key_types(&files, &config.host, config.port)?;
        let ssh_config = Arc::new(client_config(&recorded));

        let verified = Arc::new(AtomicBool::new(false));
        let abandoned = Arc::new(AtomicBool::new(false));
        let verifier = HostKeyVerifier {
            host: config.host.clone(),
            port: config.port,
            files,
            fallback_path: config
                .record_to
                .clone()
                .or_else(|| config.known_hosts.first().cloned())
                .unwrap_or_default(),
            record_to: config.record_to.clone(),
            policy: config.host_key_policy.clone(),
            verified: Arc::clone(&verified),
            abandoned: Arc::clone(&abandoned),
        };
        let handle = connect_verified(config, ssh_config, verifier, &verified, &abandoned).await?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let handle = Arc::new(Mutex::new(handle));
        let session = Arc::clone(&handle);

        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => accepted,
                };
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    // One failed accept is not a reason to tear down every other
                    // forwarded connection.
                    Err(_) => continue,
                };
                let handle = Arc::clone(&handle);
                let target = target.clone();
                tokio::spawn(async move {
                    // Nothing to report into: a connection that cannot be forwarded
                    // closes, and the client sees its own socket end.
                    let _ = forward(handle, stream, &target, peer).await;
                });
            }
        });

        Ok(Self {
            local_port,
            shutdown: Some(shutdown),
            task: Some(task),
            session,
        })
    }

    /// Whether the bastion connection is still open.
    ///
    /// A pool that keeps a tunnel across runs asks this before reusing it: an SSH connection
    /// that idled out or whose bastion restarted is closed, and every database connection
    /// forwarded through it is dead with it. A lock that happens to be held (a channel being
    /// opened) means the connection is in use, so it is reported alive rather than waited for.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        match self.session.try_lock() {
            Ok(handle) => !handle.is_closed(),
            Err(_) => true,
        }
    }

    /// The loopback port a driver connects to: `127.0.0.1:<local_port>`.
    #[must_use]
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Stop forwarding and wait for the accept loop to finish.
    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        // Stopping the accept loop drops the session handle, which closes the SSH
        // connection. Without this a dropped tunnel would leave its forwarder and its
        // bastion connection running for as long as the process lives.
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Forward one accepted connection through one SSH channel.
async fn forward(
    handle: Arc<Mutex<client::Handle<HostKeyVerifier>>>,
    mut stream: TcpStream,
    target: &Target,
    peer: std::net::SocketAddr,
) -> Result<(), Error> {
    let channel = {
        // The lock is held only to open the channel: `russh`'s handle is shared state in
        // the session loop, and a channel is independent of it once opened.
        let session = handle.lock().await;
        session
            .channel_open_direct_tcpip(
                target.host.clone(),
                u32::from(target.port),
                peer.ip().to_string(),
                u32::from(peer.port()),
            )
            .await?
    };
    let mut channel = channel.into_stream();
    tokio::io::copy_bidirectional(&mut stream, &mut channel).await?;
    Ok(())
}

/// Connect within the time limit, require that the host-key check accepted the server,
/// and only then authenticate. Generic over the handler so a test can stand in a handler
/// that says yes without verifying anything.
async fn connect_verified<H>(
    config: &BastionConfig,
    ssh_config: Arc<Config>,
    handler: H,
    verified: &AtomicBool,
    abandoned: &AtomicBool,
) -> Result<client::Handle<H>, Error>
where
    H: client::Handler<Error = Error> + Send + 'static,
{
    let connecting = client::connect(ssh_config, (config.host.as_str(), config.port), handler);
    let mut handle = match tokio::time::timeout(config.connect_timeout, connecting).await {
        Ok(connected) => connected?,
        Err(_) => {
            // `russh` runs the session on a task of its own, and dropping this future
            // does not stop a handshake that is half done. Tell the handler the caller
            // is gone, so a late host key is not recorded for an attempt that already
            // reported failure.
            abandoned.store(true, Ordering::SeqCst);
            return Err(Error::ConnectTimeout {
                host: config.host.clone(),
                port: config.port,
                after: config.connect_timeout,
            });
        }
    };

    // Belt and braces: `russh` skips the handler when a key exchange carries no
    // host key at all, and nothing authenticating may follow that.
    require_verified(verified)?;
    authenticate(&mut handle, config).await?;
    Ok(handle)
}

/// `SHA256:` and the 43 base64 characters of an unpadded SHA-256.
fn is_fingerprint(text: &str) -> bool {
    text.strip_prefix("SHA256:").is_some_and(|rest| {
        rest.len() == 43
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
    })
}

fn require_verified(verified: &AtomicBool) -> Result<(), Error> {
    if verified.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(Error::HostKeyNotVerified)
    }
}

/// The SSH client settings: the default offer, with host-key algorithms the user has
/// already recorded moved to the front (so a server holding several keys presents the
/// recorded one instead of looking like a changed key), no `none` key exchange, and no
/// certificate algorithms.
fn client_config(recorded_key_types: &[String]) -> Config {
    let mut key: Vec<Algorithm> = Preferred::DEFAULT.key.to_vec();
    // Stable: recorded kinds first, the default order otherwise.
    key.sort_by_key(|algorithm| !is_recorded(algorithm, recorded_key_types));
    let preferred = Preferred {
        key: Cow::Owned(key),
        kex: Cow::Owned(
            Preferred::DEFAULT
                .kex
                .iter()
                .copied()
                .filter(|name| *name != kex::NONE)
                .collect(),
        ),
        host_key_certificates: Cow::Borrowed(&[]),
        ..Preferred::DEFAULT
    };
    Config {
        preferred,
        // A database session can idle for minutes between a reader's batches, so the
        // SSH session must not be collected for being quiet. `inactivity_timeout`
        // stays `None` for that reason; keepalives are what notice a bastion that
        // went away, and three unanswered ones close the connection.
        keepalive_interval: Some(Duration::from_secs(30)),
        // Small interactive traffic over a tunnel pays Nagle's delay on every round
        // trip, which is the same reason the drivers set it on their own sockets.
        nodelay: true,
        ..Config::default()
    }
}

fn is_recorded(algorithm: &Algorithm, recorded: &[String]) -> bool {
    recorded.iter().any(|name| match algorithm {
        Algorithm::Ed25519 => name == "ssh-ed25519",
        Algorithm::Ecdsa { curve } => *name == format!("ecdsa-sha2-{}", curve.as_str()),
        Algorithm::Rsa { .. } => name == "ssh-rsa",
        _ => false,
    })
}

/// The host-key check, run by `russh` during the key exchange.
struct HostKeyVerifier {
    host: String,
    port: u16,
    files: Vec<StoreFile>,
    /// Named in errors that have no file of their own.
    fallback_path: PathBuf,
    record_to: Option<PathBuf>,
    policy: HostKeyPolicy,
    /// Set only when a key was matched or accepted-and-recorded.
    verified: Arc<AtomicBool>,
    /// Set when the caller gave up (the connect timeout), so nothing is decided or
    /// recorded afterwards on a session nobody is waiting for.
    abandoned: Arc<AtomicBool>,
}

impl HostKeyVerifier {
    /// The decision, with no network in it (blueprint W11 §5.3 and §5.5).
    fn decide(&self, key: ServerKey) -> Result<bool, Error> {
        if self.abandoned.load(Ordering::SeqCst) {
            return Err(Error::HostKeyNotVerified);
        }
        // `check_all` reads the files and walks them. That is blocking work inside the
        // session loop, and it is deliberate: the files are small, and the alternative
        // — checking after the exchange — would be checking after authenticating.
        match known_hosts::check_all(&self.files, &self.host, self.port, key.blob())? {
            HostKeyVerdict::Matched => {}
            HostKeyVerdict::Revoked { line, file } => {
                return Err(Error::HostKeyRevoked {
                    host: self.host.clone(),
                    port: self.port,
                    path: file.unwrap_or_else(|| self.fallback_path.clone()),
                    line,
                })
            }
            HostKeyVerdict::Mismatch { recorded } => {
                return Err(Error::HostKeyMismatch {
                    host: self.host.clone(),
                    port: self.port,
                    path: recorded
                        .first()
                        .and_then(|record| record.file())
                        .map_or_else(|| self.fallback_path.clone(), Path::to_path_buf),
                    key,
                    recorded,
                })
            }
            // A CA the user trusts covers this host, so a plain key is a downgrade.
            // No policy, pin or prompt turns that into trust.
            HostKeyVerdict::Unknown {
                covered_by_certificate_authority: true,
                ..
            } => {
                return Err(Error::HostKeyCertificateExpected {
                    host: self.host.clone(),
                    port: self.port,
                    key,
                    path: self.fallback_path.clone(),
                })
            }
            HostKeyVerdict::Unknown { .. } => self.decide_unknown(key)?,
        }
        self.verified.store(true, Ordering::SeqCst);
        Ok(true)
    }

    fn decide_unknown(&self, key: ServerKey) -> Result<(), Error> {
        match &self.policy {
            // The caller answered a previous prompt with these exact bytes.
            HostKeyPolicy::TrustNew(pinned) if pinned.blob() == key.blob() => Ok(()),
            HostKeyPolicy::TrustNew(pinned) => Err(Error::HostKeyMismatch {
                host: self.host.clone(),
                port: self.port,
                path: self.fallback_path.clone(),
                key,
                recorded: vec![known_hosts::RecordedKey::pinned(pinned)],
            }),
            HostKeyPolicy::Strict => Err(Error::HostKeyUnknown {
                host: self.host.clone(),
                port: self.port,
                path: self.fallback_path.clone(),
                key,
                covered_by_certificate_authority: false,
            }),
            HostKeyPolicy::TrustFingerprint(pin) => {
                let presented = key.fingerprint();
                if *pin != presented {
                    return Err(Error::HostKeyPinMismatch {
                        host: self.host.clone(),
                        port: self.port,
                        key,
                        presented,
                        pinned: pin.clone(),
                    });
                }
                // Record first, accept second: no authentication goes to a key that
                // is not on record.
                let Some(path) = &self.record_to else {
                    return Err(Error::Usage(
                        "a host key cannot be trusted without a file to record it in",
                    ));
                };
                known_hosts::append_if_absent(path, &self.host, self.port, key.blob())
                    .map(|_| ())
                    .map_err(|error| match error {
                        error @ Error::HostKeyStoreUnsafe { .. } => error,
                        other => Error::HostKeyRecordFailed {
                            host: self.host.clone(),
                            port: self.port,
                            key,
                            path: path.clone(),
                            reason: other.to_string(),
                        },
                    })
            }
        }
    }
}

impl client::Handler for HostKeyVerifier {
    type Error = Error;

    async fn check_server_key(
        &mut self,
        presented: &PublicKeyOrCertificate,
    ) -> Result<bool, Error> {
        let key = match presented {
            PublicKeyOrCertificate::PublicKey { key, .. } => ServerKey::from_blob(key.to_bytes()?)?,
            // Refused rather than ignored: a certificate replaces the host key in the
            // exchange, so "check the key inside it instead" would be checking something
            // the user was never asked to trust (the same reasoning `russh` records where
            // it calls this).
            PublicKeyOrCertificate::Certificate(certificate) => {
                return Err(Error::HostCertificateUnsupported {
                    host: self.host.clone(),
                    port: self.port,
                    fingerprint: crate::fingerprint(&certificate.to_bytes()?),
                })
            }
        };
        self.decide(key)
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        // The `none` exchange carries no host key, so `check_server_key` never runs.
        if names.kex == kex::NONE {
            return Err(Error::HostKeyNotVerified);
        }
        Ok(())
    }
}

/// Authenticate over an already host-key-verified session.
async fn authenticate<H>(
    handle: &mut client::Handle<H>,
    config: &BastionConfig,
) -> Result<(), Error>
where
    H: client::Handler,
{
    let result = match &config.auth {
        Auth::Password(password) => {
            handle
                .authenticate_password(config.user.clone(), password.expose_secret().to_owned())
                .await?
        }
        Auth::Key { path, passphrase } => {
            let text = std::fs::read_to_string(path).map_err(|source| Error::KeyUnreadable {
                path: path.clone(),
                source,
            })?;
            let key = decode_secret_key(
                &text,
                passphrase.as_ref().map(|secret| secret.expose_secret()),
            )?;
            let hash_alg = rsa_hash_alg(handle, key.algorithm().is_rsa()).await?;
            handle
                .authenticate_publickey(
                    config.user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await?
        }
        Auth::Agent => authenticate_with_agent(handle, &config.user).await?,
    };

    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => Err(Error::AuthenticationRejected {
            user: config.user.clone(),
            methods: describe_methods(&remaining_methods, partial_success),
        }),
    }
}

/// Which RSA signature algorithm to ask for, if the key is an RSA one.
///
/// `russh` answers this from the server's `server-sig-algs` extension; `None` means the
/// server did not advertise one, and passing `None` then means the legacy `ssh-rsa`
/// (SHA-1), which is what that server is asking for.
async fn rsa_hash_alg<H>(handle: &client::Handle<H>, is_rsa: bool) -> Result<Option<HashAlg>, Error>
where
    H: client::Handler,
{
    if !is_rsa {
        return Ok(None);
    }
    Ok(handle.best_supported_rsa_hash().await?.flatten())
}

/// Try every identity the agent holds, in the order the agent lists them.
///
/// The agent is the signer: the private key never enters this process, which is the
/// point of using it. A certificate identity is offered as a certificate and signed by
/// the agent as well.
async fn authenticate_with_agent<H>(
    handle: &mut client::Handle<H>,
    user: &str,
) -> Result<AuthResult, Error>
where
    H: client::Handler,
{
    let mut agent = AgentClient::connect_env().await?;
    let identities = agent.request_identities().await?;
    let hash_alg = rsa_hash_alg(handle, identities.iter().any(is_rsa_identity)).await?;
    let mut last = AuthResult::Failure {
        remaining_methods: MethodSet::empty(),
        partial_success: false,
    };

    for identity in identities {
        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                let hash_alg = if key.algorithm().is_rsa() {
                    hash_alg
                } else {
                    None
                };
                handle
                    .authenticate_publickey_with(user.to_owned(), key, hash_alg, &mut agent)
                    .await?
            }
            AgentIdentity::Certificate { certificate, .. } => {
                let hash_alg = if certificate.algorithm().is_rsa() {
                    hash_alg
                } else {
                    None
                };
                handle
                    .authenticate_certificate_with(
                        user.to_owned(),
                        certificate,
                        hash_alg,
                        &mut agent,
                    )
                    .await?
            }
        };
        match result {
            AuthResult::Success => return Ok(AuthResult::Success),
            failure => last = failure,
        }
    }
    Ok(last)
}

fn is_rsa_identity(identity: &AgentIdentity) -> bool {
    match identity {
        AgentIdentity::PublicKey { key, .. } => key.algorithm().is_rsa(),
        AgentIdentity::Certificate { certificate, .. } => certificate.algorithm().is_rsa(),
    }
}

fn describe_methods(remaining: &MethodSet, partial_success: bool) -> String {
    let names = remaining
        .iter()
        .map(String::from)
        .collect::<Vec<_>>()
        .join(", ");
    if partial_success {
        format!("{names} (partial success)")
    } else if names.is_empty() {
        "nothing".to_owned()
    } else {
        names
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use base64::Engine as _;

    use super::*;

    const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G2sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";

    fn key(body: &str) -> ServerKey {
        ServerKey::from_blob(
            base64::engine::general_purpose::STANDARD
                .decode(body)
                .expect("base64"),
        )
        .expect("a key")
    }

    struct Rig {
        dir: tempfile::TempDir,
        verifier: HostKeyVerifier,
    }

    impl Rig {
        fn app_file(&self) -> PathBuf {
            self.dir.path().join("app_known_hosts")
        }

        fn user_file(&self) -> PathBuf {
            self.dir.path().join("user_known_hosts")
        }
    }

    /// A verifier for `bastion.corp:22` over a user file (`user_text`) and an app file
    /// that does not exist yet, with `policy`.
    fn rig(user_text: &str, policy: HostKeyPolicy) -> Rig {
        let dir = tempfile::tempdir().expect("temp dir");
        let user = dir.path().join("user_known_hosts");
        let app = dir.path().join("app_known_hosts");
        std::fs::write(&user, user_text).expect("write user file");
        let verifier = HostKeyVerifier {
            host: "bastion.corp".to_owned(),
            port: 22,
            files: vec![
                StoreFile::new(&user, Origin::User),
                StoreFile::new(&app, Origin::App),
            ],
            fallback_path: app.clone(),
            record_to: Some(app),
            policy,
            verified: Arc::new(AtomicBool::new(false)),
            abandoned: Arc::new(AtomicBool::new(false)),
        };
        Rig { dir, verifier }
    }

    fn verified(rig: &Rig) -> bool {
        rig.verifier.verified.load(Ordering::SeqCst)
    }

    fn pin(key: &ServerKey) -> HostKeyPolicy {
        HostKeyPolicy::TrustFingerprint(key.fingerprint())
    }

    #[test]
    fn an_unknown_host_without_a_pin_is_refused_and_nothing_is_written() {
        let rig = rig("", HostKeyPolicy::Strict);
        assert!(matches!(
            rig.verifier.decide(key(KEY_A)),
            Err(Error::HostKeyUnknown { .. })
        ));
        assert!(!verified(&rig));
        assert!(!rig.app_file().exists());
    }

    #[test]
    fn the_right_pin_records_first_then_accepts() {
        let presented = key(KEY_A);
        let rig = rig("", pin(&presented));
        assert!(rig.verifier.decide(presented.clone()).expect("accepted"));
        assert!(verified(&rig));
        let text = std::fs::read_to_string(rig.app_file()).expect("recorded");
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.starts_with(&format!(
            "bastion.corp ssh-ed25519 {KEY_A} # accepted by QueryHive 20"
        )));
        let mode = std::fs::metadata(rig.app_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // And the very next check, with no pin at all, matches without writing.
        let again = HostKeyVerifier {
            policy: HostKeyPolicy::Strict,
            verified: Arc::new(AtomicBool::new(false)),
            abandoned: Arc::new(AtomicBool::new(false)),
            files: rig.verifier.files.clone(),
            record_to: rig.verifier.record_to.clone(),
            fallback_path: rig.verifier.fallback_path.clone(),
            host: rig.verifier.host.clone(),
            port: rig.verifier.port,
        };
        assert!(again.decide(presented).expect("matched"));
        assert!(again.verified.load(Ordering::SeqCst));
        assert_eq!(std::fs::read_to_string(rig.app_file()).unwrap(), text);
    }

    #[test]
    fn a_wrong_pin_is_refused_and_nothing_is_recorded() {
        let rig = rig("", pin(&key(KEY_B)));
        match rig.verifier.decide(key(KEY_A)) {
            Err(Error::HostKeyPinMismatch {
                presented, pinned, ..
            }) => {
                assert_eq!(presented, key(KEY_A).fingerprint());
                assert_eq!(pinned, key(KEY_B).fingerprint());
            }
            other => panic!("expected a pin mismatch, got {other:?}"),
        }
        assert!(!verified(&rig));
        assert!(!rig.app_file().exists());
    }

    #[test]
    fn a_changed_key_is_never_accepted_with_or_without_the_pin() {
        let presented = key(KEY_A);
        let recorded = format!("bastion.corp ssh-ed25519 {KEY_B}\n");
        for policy in [
            HostKeyPolicy::Strict,
            pin(&presented),
            HostKeyPolicy::TrustNew(presented.clone()),
        ] {
            let rig = rig(&recorded, policy);
            match rig.verifier.decide(presented.clone()) {
                Err(Error::HostKeyMismatch { recorded, path, .. }) => {
                    assert_eq!(recorded.len(), 1);
                    assert_eq!(path, rig.user_file(), "the message names the file to fix");
                }
                other => panic!("expected a mismatch, got {other:?}"),
            }
            assert!(!verified(&rig));
            assert!(!rig.app_file().exists());
        }
    }

    #[test]
    fn a_revoked_key_is_never_accepted_even_with_the_pin() {
        let presented = key(KEY_A);
        let rig = rig(
            &format!("@revoked bastion.corp ssh-ed25519 {KEY_A}\n"),
            pin(&presented),
        );
        assert!(matches!(
            rig.verifier.decide(presented),
            Err(Error::HostKeyRevoked { line: 1, .. })
        ));
        assert!(!verified(&rig));
        assert!(!rig.app_file().exists());
    }

    #[test]
    fn a_plain_key_for_a_ca_managed_host_is_refused_whatever_the_policy() {
        let presented = key(KEY_A);
        let ca = format!("@cert-authority *.corp ssh-ed25519 {KEY_B}\n");
        for policy in [
            HostKeyPolicy::Strict,
            pin(&presented),
            HostKeyPolicy::TrustNew(presented.clone()),
        ] {
            let rig = rig(&ca, policy);
            assert!(
                matches!(
                    rig.verifier.decide(presented.clone()),
                    Err(Error::HostKeyCertificateExpected { .. })
                ),
                "a CA-covered host must not reach first-use acceptance"
            );
            assert!(!verified(&rig));
            assert!(!rig.app_file().exists());
        }
    }

    #[test]
    fn a_pin_that_cannot_be_recorded_is_not_an_acceptance() {
        let presented = key(KEY_A);
        let mut rig = rig("", pin(&presented));
        let locked = rig.dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let target = locked.join("known_hosts");
        rig.verifier.record_to = Some(target.clone());
        rig.verifier.files[1] = StoreFile::new(&target, Origin::App);

        let result = rig.verifier.decide(presented);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(result, Err(Error::HostKeyRecordFailed { .. })),
            "{result:?}"
        );
        assert!(!verified(&rig));
    }

    #[test]
    fn an_app_file_others_can_write_is_unsafe_even_for_a_pinned_unknown_host() {
        let presented = key(KEY_A);
        let rig = rig("", pin(&presented));
        std::fs::write(rig.app_file(), "").unwrap();
        std::fs::set_permissions(rig.app_file(), std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(matches!(
            rig.verifier.decide(presented),
            Err(Error::HostKeyStoreUnsafe { .. })
        ));
        assert!(!verified(&rig));
    }

    #[test]
    fn a_pin_must_look_like_a_fingerprint() {
        assert!(is_fingerprint(&key(KEY_A).fingerprint()));
        for bad in [
            "",
            "SHA256:",
            "sha256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog",
            "MD5:aa:bb",
            "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog=",
            "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuo",
            "SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuo!",
        ] {
            assert!(!is_fingerprint(bad), "{bad:?}");
        }
    }

    #[test]
    fn authentication_is_not_started_on_a_connection_nothing_verified() {
        let flag = AtomicBool::new(false);
        assert!(matches!(
            require_verified(&flag),
            Err(Error::HostKeyNotVerified)
        ));
        flag.store(true, Ordering::SeqCst);
        assert!(require_verified(&flag).is_ok());
    }

    #[test]
    fn the_offer_has_no_none_exchange_no_certificates_and_recorded_kinds_first() {
        let plain = client_config(&[]);
        assert!(!plain.preferred.kex.contains(&kex::NONE));
        assert!(plain.preferred.host_key_certificates.is_empty());
        assert_eq!(
            plain.preferred.key,
            Preferred::DEFAULT.key,
            "no records, default order"
        );

        let rsa = client_config(&["ssh-rsa".to_owned()]);
        assert!(
            matches!(rsa.preferred.key[0], Algorithm::Rsa { .. }),
            "{:?}",
            rsa.preferred.key
        );
        assert_eq!(rsa.preferred.key.len(), Preferred::DEFAULT.key.len());
        let rsa_count = rsa
            .preferred
            .key
            .iter()
            .filter(|a| matches!(a, Algorithm::Rsa { .. }))
            .count();
        assert!(rsa.preferred.key[..rsa_count]
            .iter()
            .all(|a| matches!(a, Algorithm::Rsa { .. })));

        // A recorded kind that cannot be negotiated changes nothing.
        let sk = client_config(&[
            "sk-ssh-ed25519@openssh.com".to_owned(),
            "ssh-dss".to_owned(),
        ]);
        assert_eq!(sk.preferred.key, Preferred::DEFAULT.key);
    }

    fn bastion() -> BastionConfig {
        BastionConfig::new(
            "127.0.0.1",
            1,
            "nobody",
            Auth::Agent,
            "/nonexistent/known_hosts",
        )
    }

    #[tokio::test]
    async fn a_bad_pin_or_a_pin_with_nowhere_to_record_is_refused_before_the_network() {
        let target = || Target::new("db", 5432);
        let good = key(KEY_A).fingerprint();
        let no_store = bastion().trust_fingerprint(good.clone());
        assert!(matches!(
            Tunnel::open(&no_store, target()).await,
            Err(Error::Usage(_))
        ));
        let bad_pin = bastion()
            .record_to("/nonexistent/app")
            .trust_fingerprint("SHA256:x");
        assert!(matches!(
            Tunnel::open(&bad_pin, target()).await,
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn a_session_the_caller_gave_up_on_records_nothing() {
        let presented = key(KEY_A);
        let rig = rig("", pin(&presented));
        rig.verifier.abandoned.store(true, Ordering::SeqCst);
        assert!(matches!(
            rig.verifier.decide(presented),
            Err(Error::HostKeyNotVerified)
        ));
        assert!(!verified(&rig));
        assert!(!rig.app_file().exists());
    }

    // The time limit and the guard, against sockets of our own: no container needed.

    /// A listener that accepts, says `greeting` (maybe nothing) and then never speaks
    /// or closes again.
    async fn quiet_listener(greeting: &'static [u8]) -> u16 {
        use tokio::io::AsyncWriteExt as _;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(greeting).await;
                held.push(stream);
            }
        });
        port
    }

    fn quick(port: u16) -> BastionConfig {
        BastionConfig::new(
            "127.0.0.1",
            port,
            "nobody",
            Auth::Password(SecretString::new("not-sent".into())),
            "/nonexistent/known_hosts",
        )
        .connect_timeout(Duration::from_millis(300))
    }

    /// Says yes to every host key, as a verifier with a bug would, and marks the
    /// connection verified only if it was given the flag.
    struct Lax(Option<Arc<AtomicBool>>);

    impl client::Handler for Lax {
        type Error = Error;

        async fn check_server_key(&mut self, _: &PublicKeyOrCertificate) -> Result<bool, Error> {
            if let Some(flag) = &self.0 {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(true)
        }
    }

    /// An SSH server that rejects every login and counts how many it was asked for.
    struct Counting(Arc<std::sync::atomic::AtomicUsize>);

    impl russh::server::Handler for Counting {
        type Error = russh::Error;

        async fn auth_none(&mut self, _: &str) -> Result<russh::server::Auth, russh::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(russh::server::Auth::reject())
        }

        async fn auth_password(
            &mut self,
            _: &str,
            _: &str,
        ) -> Result<russh::server::Auth, russh::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(russh::server::Auth::reject())
        }
    }

    async fn counting_server() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let config = Arc::new(russh::server::Config {
            keys: vec![russh::keys::PrivateKey::from(
                russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
            )],
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..russh::server::Config::default()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let counter = Arc::clone(&attempts);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let counting = Counting(Arc::clone(&counter));
                if let Ok(session) =
                    russh::server::run_stream(Arc::clone(&config), stream, counting).await
                {
                    tokio::spawn(session);
                }
            }
        });
        (port, attempts)
    }

    #[tokio::test]
    async fn a_bastion_that_goes_quiet_ends_in_a_timeout_not_a_hang() {
        // Silent from the first byte (a black hole that accepts), and silent after its
        // banner (stalled in the key exchange): the two places the handshake can wait.
        for greeting in [&b""[..], &b"SSH-2.0-quiet\r\n"[..]] {
            let port = quiet_listener(greeting).await;
            let started = std::time::Instant::now();
            let result = Tunnel::open(&quick(port), Target::new("db", 5432)).await;
            match result {
                Err(Error::ConnectTimeout {
                    port: reported,
                    after,
                    ..
                }) => {
                    assert_eq!(reported, port);
                    assert_eq!(after, Duration::from_millis(300));
                }
                other => panic!("expected a connect timeout, got {other:?}"),
            }
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }

    #[tokio::test]
    async fn a_timeout_tells_the_session_the_caller_left() {
        let port = quiet_listener(b"SSH-2.0-quiet\r\n").await;
        let (verified, abandoned) = (AtomicBool::new(false), AtomicBool::new(false));
        let result = connect_verified(
            &quick(port),
            Arc::new(client_config(&[])),
            Lax(None),
            &verified,
            &abandoned,
        )
        .await;
        assert!(matches!(result, Err(Error::ConnectTimeout { .. })));
        assert!(abandoned.load(Ordering::SeqCst));
        assert!(!verified.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_refused_connection_fails_at_once_and_is_not_a_timeout() {
        let port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            probe.local_addr().expect("addr").port()
        };
        let started = std::time::Instant::now();
        let result = Tunnel::open(&quick(port), Target::new("db", 5432)).await;
        // `russh` reports the refused socket as its own transport error.
        assert!(
            matches!(result, Err(Error::Ssh(_) | Error::Io(_))),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[tokio::test]
    async fn authentication_never_starts_on_a_connection_the_verifier_did_not_accept() {
        let (port, attempts) = counting_server().await;
        let config = quick(port).connect_timeout(Duration::from_secs(10));
        let ssh_config = || Arc::new(client_config(&[]));

        // The handler says yes and never marks the connection verified: the guard, not
        // the handler, is what keeps the password at home.
        let unmarked = AtomicBool::new(false);
        let refused = connect_verified(
            &config,
            ssh_config(),
            Lax(None),
            &unmarked,
            &AtomicBool::new(false),
        )
        .await;
        assert!(
            matches!(refused, Err(Error::HostKeyNotVerified)),
            "{:?}",
            refused.err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 0, "a login was attempted");

        // The control: the same server and config, verified this time. The server does
        // see the login, so the zero above means something.
        let marked = Arc::new(AtomicBool::new(false));
        let reached = connect_verified(
            &config,
            ssh_config(),
            Lax(Some(Arc::clone(&marked))),
            &marked,
            &AtomicBool::new(false),
        )
        .await;
        assert!(
            matches!(reached, Err(Error::AuthenticationRejected { .. })),
            "{:?}",
            reached.err()
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_system_file_is_read_as_the_system_s() {
        let config = bastion().with_system_known_hosts().record_to("/app");
        let origins: Vec<Origin> = config.store_files().iter().map(|f| f.origin).collect();
        assert_eq!(origins, [Origin::User, Origin::System, Origin::App]);
    }
}
