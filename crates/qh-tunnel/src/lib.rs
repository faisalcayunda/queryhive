//! SSH tunnels to a database through a bastion, with `known_hosts` verified here.
//!
//! Why this is its own crate, and why the check is ours
//! ---------------------------------------------------
//! The blueprint (§3.1, line 581) picked `russh` + `russh-keys` over `ssh2` for one
//! reason: `ssh2` wraps libssh2 and gives no control over `known_hosts`, and this crate
//! exists to own that check. Two consequences are load-bearing:
//!
//! - **The host key is checked before authentication.** `russh` calls the handler's
//!   `check_server_key` during the key exchange, so a bastion that cannot be identified
//!   never receives a public key signature or a password. The check is not a post-hoc
//!   audit of a connection we already made.
//! - **Trust on first use is a decision, not a callback.** [`Tunnel::open`] refuses an
//!   unknown host with [`Error::HostKeyUnknown`], which carries the fingerprint to show
//!   and the [`ServerKey`] that would be accepted. The caller puts that in front of a
//!   person, and on "yes" either appends it ([`known_hosts::append`]) and opens again,
//!   or passes it back as [`HostKeyPolicy::TrustNew`]. Nothing in this crate ever asks
//!   for a decision, blocks on one, or accepts a key on its own — the point is that a
//!   prompt is only shown when a person is actually there to answer it.
//!
//! The app reads `~/.ssh/known_hosts` itself, which is why it ships without a sandbox
//! (ADR-0007): the sandbox and this requirement collide directly, not in theory.
//!
//! Host certificates are refused, not ignored
//! ------------------------------------------
//! A `@cert-authority` line holds a CA key, not a host key, and is never matched as
//! one. This build does not verify host certificates: when a server offers one,
//! [`Error::HostCertificateUnsupported`] is returned, and an "unknown host" shown while
//! a CA covers that host says so
//! ([`HostKeyVerdict::Unknown::covered_by_certificate_authority`]). Ignoring the offer
//! and reporting the key inside the certificate as an unknown host would be the one
//! outcome that is genuinely worse: it would look like a normal TOFU prompt. Supporting
//! certificates is a decision for whoever needs them, and it needs CA matching, validity
//! windows and principals — not a line skipped in a parser.
//!
//! What is verified end to end
//! ---------------------------
//! `crates/qh-tunnel/tests/sshd.rs` drives a real `sshd` in a container for the whole
//! flow: unknown host, append, match, mismatch against a second host key, a `@revoked`
//! refusal, and a real forward whose bytes are read back through the tunnel. It skips
//! loudly unless `QH_TEST_SSH=1`, because a green run that tested nothing is worse than
//! a visible skip.

#![forbid(unsafe_code)]

mod error;
mod key;
pub mod known_hosts;
mod tunnel;

pub use error::Error;
pub use key::{fingerprint, ServerKey};
pub use known_hosts::{HostKeyVerdict, RecordedKey};
pub use tunnel::{Auth, BastionConfig, HostKeyPolicy, Target, Tunnel};
// The type of the passwords and passphrases in [`Auth`], so a caller does not have to
// depend on `secrecy` itself to build one.
pub use secrecy::SecretString;
