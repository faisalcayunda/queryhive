//! The `queryhive-mcp` binary: an MCP server over stdio, and the three token subcommands.
//!
//! ```text
//! queryhive-mcp [--token TOKEN]        serve MCP on stdin/stdout (token also from QH_MCP_TOKEN)
//! queryhive-mcp issue --name N [--scope a,b] [--connection id1,id2] [--days D]
//! queryhive-mcp list
//! queryhive-mcp revoke --id ID
//! ```
//!
//! # One rule about stdout, and it is absolute
//!
//! In serve mode **stdout carries protocol messages and nothing else**. Every log, every
//! warning and every startup failure goes to stderr, because a line of prose on stdout is
//! a corrupted JSON-RPC stream to the client. The token subcommands are ordinary CLIs and
//! print their JSON result to stdout; they are not the server.
//!
//! # A helper, not a part of the app
//!
//! This process shares nothing with `QueryHive.app` except the database file and the
//! Keychain: it is spawned by whichever MCP client the user has configured, and the app
//! never spawns it. Killing it therefore cannot affect the app, and the handshake it
//! writes is how a reader finds a live one — a handshake whose pid is gone is ignored,
//! which is what a SIGKILL leaves behind.
//!
//! # The token is read, hashed and forgotten
//!
//! `--token` or `QH_MCP_TOKEN` supplies the token; the store is searched by its SHA-256,
//! and the plaintext is never logged, stored, or written to the handshake. A missing,
//! unknown, expired or revoked token is one line on stderr and a non-zero exit.

use std::io::Write;
use std::process::ExitCode;

use qh_ffi::mcp::{self, Handshake, Server};
use qh_ffi::RealEngine;
use qh_sync::SyncId;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    match argv.get(1).map(String::as_str) {
        Some("issue") => cmd_issue(&argv[2..]),
        Some("list") => cmd_list(),
        Some("revoke") => cmd_revoke(&argv[2..]),
        Some("serve") => cmd_serve(&argv[2..]),
        Some("--help" | "-h") => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        // No subcommand is the server: a client configured with an absolute path and a
        // `--token` argument should not also have to spell out "serve".
        _ => cmd_serve(&argv[1..]),
    }
}

/// Serve MCP on stdin/stdout until the client closes the stream.
fn cmd_serve(args: &[String]) -> ExitCode {
    let Some((token_flag, unknown)) = split_token(args) else {
        eprintln!("queryhive-mcp: unknown argument '{}'", unknown_args(args));
        eprintln!("{}", usage());
        return ExitCode::from(2);
    };
    if unknown {
        eprintln!("queryhive-mcp: unknown argument '{}'", unknown_args(args));
        eprintln!("{}", usage());
        return ExitCode::from(2);
    }

    let token_value = token_flag.or_else(|| std::env::var(mcp::TOKEN_ENV).ok());
    let base_pairs = db_pairs();

    let store = match mcp::open_store(&base_pairs) {
        Ok(store) => store,
        Err(message) => return fail(&message),
    };
    let now = qh_storage::now_millis();
    let (token, record) = match mcp::resolve_token(&store, token_value.as_deref(), now) {
        Ok(resolved) => resolved,
        Err(refusal) => {
            // The refusal message never contains the token, and neither does anything
            // else on this path.
            return fail(refusal.message());
        }
    };

    let server = Server::new(token, base_pairs);
    let engine = RealEngine::new();
    let runtime = match qh_rt::build_main() {
        Ok(runtime) => runtime,
        Err(error) => return fail(&format!("could not start a runtime: {error}")),
    };

    // The handshake is written before the first request is read and removed on a clean
    // exit. A SIGKILL leaves it behind; a reader ignores it because the pid is gone.
    let handshake_path = mcp::handshake_path();
    if let Some(path) = &handshake_path {
        let handshake = Handshake {
            pid: std::process::id(),
            started_at: now,
            token_id: record.id.to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        if let Err(error) = mcp::write_handshake(path, &handshake) {
            // Not fatal: the handshake is a convenience for other processes, and refusing
            // to serve because it could not be written would make a read-only Application
            // Support directory a broken server.
            eprintln!("queryhive-mcp: could not write the handshake: {error}");
        }
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let token_id = record.id.clone();
    let mut touch = || {
        if let Err(error) = store.touch_mcp_token(&token_id, qh_storage::now_millis()) {
            eprintln!("queryhive-mcp: could not record the token use: {error}");
        }
    };

    eprintln!(
        "queryhive-mcp {} serving token '{}'",
        env!("CARGO_PKG_VERSION"),
        record.name
    );

    let outcome = mcp::serve(
        &server,
        &engine,
        &runtime,
        stdin.lock(),
        &mut output,
        &mut touch,
    );
    // Flush before removing the handshake, so the last response is on its way out before
    // anything else this process does.
    let _ = output.flush();
    if let Some(path) = &handshake_path {
        mcp::remove_handshake(path);
    }

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the MCP stream failed: {error}")),
    }
}

/// `issue`: mint a token and print it exactly once.
fn cmd_issue(args: &[String]) -> ExitCode {
    let Some(name) = argument(args, "--name") else {
        return usage_error("issue needs --name");
    };
    if name.trim().is_empty() {
        return usage_error("issue needs a non-empty --name");
    }

    // Omitted means every read-only tool; an explicit list is honoured as written, and a
    // name nobody has is refused here rather than becoming a scope that matches nothing.
    let scopes = match argument(args, "--scope") {
        Some(raw) => {
            let scopes = mcp::comma_list(&raw);
            if let Some(unknown) = scopes.iter().find(|scope| !mcp::is_read_only_tool(scope)) {
                return usage_error(&format!("unknown tool '{unknown}' in --scope"));
            }
            scopes
        }
        None => mcp::read_only_tool_names(),
    };
    let connections = argument(args, "--connection")
        .map(|raw| mcp::comma_list(&raw))
        .unwrap_or_default();

    let now = qh_storage::now_millis();
    let expires_at = match argument(args, "--days") {
        Some(raw) => match raw.parse::<i64>() {
            Ok(days) if days > 0 => Some(mcp::expires_after_days(days, now)),
            _ => return usage_error("--days must be a positive whole number of days"),
        },
        None => None,
    };

    let store = match mcp::open_store(&db_pairs()) {
        Ok(store) => store,
        Err(message) => return fail(&message),
    };
    match store.issue_mcp_token(&name, &scopes, &connections, expires_at, now) {
        Ok((token, record)) => {
            // The only time this string exists outside the client's hands.
            let line = serde_json::json!({"token": token, "id": record.id.as_str()});
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("could not issue the token: {error}")),
    }
}

/// `list`: every token, with no hash and no secret.
fn cmd_list() -> ExitCode {
    let store = match mcp::open_store(&db_pairs()) {
        Ok(store) => store,
        Err(message) => return fail(&message),
    };
    match store.mcp_tokens() {
        Ok(tokens) => {
            println!("{}", mcp::token_listing(&tokens));
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("could not list the tokens: {error}")),
    }
}

/// `revoke`: stop a token, keeping the row for the audit trail.
fn cmd_revoke(args: &[String]) -> ExitCode {
    let Some(raw) = argument(args, "--id") else {
        return usage_error("revoke needs --id");
    };
    let Ok(id) = SyncId::parse(&raw) else {
        return usage_error(&format!("--id '{raw}' is not a token id"));
    };
    let store = match mcp::open_store(&db_pairs()) {
        Ok(store) => store,
        Err(message) => return fail(&message),
    };
    match store.revoke_mcp_token(&id, qh_storage::now_millis()) {
        Ok(revoked) => {
            println!(
                "{}",
                serde_json::json!({"id": id.as_str(), "revoked": revoked})
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("could not revoke the token: {error}")),
    }
}

/// `DB_PATH` as settings pairs, so the subcommands and the server read the same database
/// the app would. Absent means the Application Support database.
fn db_pairs() -> Vec<(String, String)> {
    match std::env::var("DB_PATH") {
        Ok(path) if !path.is_empty() => vec![("DB_PATH".to_owned(), path)],
        _ => Vec::new(),
    }
}

/// The value of `--flag VALUE` or `--flag=VALUE`.
fn argument(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == flag {
            return iter.next().cloned();
        }
        if let Some(value) = argument.strip_prefix(&prefix) {
            return Some(value.to_owned());
        }
    }
    None
}

/// `(the --token value, whether anything unrecognised was seen)`.
///
/// Returns `None` only when `--token` is the last argument with nothing after it, which is
/// a usage error the caller reports.
fn split_token(args: &[String]) -> Option<(Option<String>, bool)> {
    let mut token = None;
    let mut unknown = false;
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        if argument == "--token" {
            let value = iter.next()?;
            token = Some(value.clone());
        } else if let Some(value) = argument.strip_prefix("--token=") {
            token = Some(value.to_owned());
        } else if !argument.is_empty() {
            unknown = true;
        }
    }
    Some((token, unknown))
}

fn unknown_args(args: &[String]) -> String {
    args.iter()
        .find(|argument| !argument.starts_with("--token"))
        .cloned()
        .unwrap_or_default()
}

fn fail(message: &str) -> ExitCode {
    eprintln!("queryhive-mcp: {message}");
    ExitCode::from(1)
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("queryhive-mcp: {message}");
    eprintln!("{}", usage());
    ExitCode::from(2)
}

fn usage() -> String {
    format!(
        "usage: queryhive-mcp [--token TOKEN]\n\
         \x20      queryhive-mcp issue --name N [--scope t1,t2] [--connection id1,id2] [--days D]\n\
         \x20      queryhive-mcp list\n\
         \x20      queryhive-mcp revoke --id ID\n\
         the token may also come from {} and the database from DB_PATH",
        mcp::TOKEN_ENV
    )
}
