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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::client::AuthResult;
use russh::client::{self, Config};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{decode_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::MethodSet;
use secrecy::{ExposeSecret, SecretString};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::known_hosts::{self, HostKeyVerdict};
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

/// What to do when the bastion's key is not in `known_hosts`.
#[derive(Debug, Clone)]
pub enum HostKeyPolicy {
    /// Refuse it and report the fingerprint. The caller decides what happens next.
    Strict,
    /// Accept this one key and no other: the answer to a previous refusal, replayed.
    TrustNew(ServerKey),
}

/// Where the bastion is, how to authenticate to it, and what to check it against.
#[derive(Debug)]
pub struct BastionConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    /// The `known_hosts` file to check against. Read and written by this crate rather
    /// than by a transport library, which is why the app ships without a sandbox
    /// (ADR-0007).
    pub known_hosts: PathBuf,
    pub host_key_policy: HostKeyPolicy,
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
            known_hosts: known_hosts.into(),
            host_key_policy: HostKeyPolicy::Strict,
        }
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
#[derive(Debug)]
pub struct Tunnel {
    local_port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
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
    /// [`Error::AuthenticationRejected`] if no authentication method worked.
    pub async fn open(config: &BastionConfig, target: Target) -> Result<Self, Error> {
        let ssh_config = Arc::new(Config {
            // A database session can idle for minutes between a reader's batches, so the
            // SSH session must not be collected for being quiet. `inactivity_timeout`
            // stays `None` for that reason; keepalives are what notice a bastion that
            // went away, and three unanswered ones close the connection.
            keepalive_interval: Some(Duration::from_secs(30)),
            // Small interactive traffic over a tunnel pays Nagle's delay on every round
            // trip, which is the same reason the drivers set it on their own sockets.
            nodelay: true,
            ..Config::default()
        });

        let verifier = HostKeyVerifier {
            host: config.host.clone(),
            port: config.port,
            known_hosts: config.known_hosts.clone(),
            policy: config.host_key_policy.clone(),
        };
        let mut handle =
            client::connect(ssh_config, (config.host.as_str(), config.port), verifier).await?;

        authenticate(&mut handle, config).await?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let handle = Arc::new(Mutex::new(handle));

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
        })
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

/// The host-key check, run by `russh` during the key exchange.
struct HostKeyVerifier {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    policy: HostKeyPolicy,
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

        // `check` reads the file and walks it. That is blocking work inside the session
        // loop, and it is deliberate: the file is small, and the alternative — checking
        // after the exchange — would be checking after authenticating.
        match known_hosts::check(&self.known_hosts, &self.host, self.port, key.blob())? {
            HostKeyVerdict::Matched => Ok(true),
            HostKeyVerdict::Revoked { line } => Err(Error::HostKeyRevoked {
                host: self.host.clone(),
                port: self.port,
                path: self.known_hosts.clone(),
                line,
            }),
            HostKeyVerdict::Mismatch { recorded } => Err(Error::HostKeyMismatch {
                host: self.host.clone(),
                port: self.port,
                path: self.known_hosts.clone(),
                key,
                recorded,
            }),
            HostKeyVerdict::Unknown {
                covered_by_certificate_authority,
                ..
            } => match &self.policy {
                // The caller answered a previous prompt with these exact bytes.
                HostKeyPolicy::TrustNew(pinned) if pinned.blob() == key.blob() => Ok(true),
                HostKeyPolicy::TrustNew(pinned) => Err(Error::HostKeyMismatch {
                    host: self.host.clone(),
                    port: self.port,
                    path: self.known_hosts.clone(),
                    key,
                    recorded: vec![known_hosts::RecordedKey::pinned(pinned)],
                }),
                HostKeyPolicy::Strict => Err(Error::HostKeyUnknown {
                    host: self.host.clone(),
                    port: self.port,
                    path: self.known_hosts.clone(),
                    key,
                    covered_by_certificate_authority,
                }),
            },
        }
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
