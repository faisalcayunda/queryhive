//! `--self-test-confinement`: what the kernel sandbox actually forbids, asked from inside
//! it (blueprint section 14.9).
//!
//! The app runs the helper under `sandbox-exec` with a fixed profile. This command is how a
//! test proves the profile works instead of arguing that it does: it tries each thing the
//! helper must not be able to do, and the one thing it must, and prints one line for each:
//!
//! ```text
//! <probe> allowed
//! <probe> denied <reason>
//! ```
//!
//! The probes that take a path or an address get it as `key=value` arguments, because the
//! test knows where its fixtures are and the helper does not.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn report(out: &mut impl Write, probe: &str, outcome: std::io::Result<()>) {
    let _ = match outcome {
        Ok(()) => writeln!(out, "{probe} allowed"),
        Err(error) => writeln!(out, "{probe} denied {error}"),
    };
}

fn try_create(dir: &std::path::Path, name: &str) -> std::io::Result<()> {
    let path = dir.join(name);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    // It was created, so it is not denied: do not leave it behind.
    std::fs::remove_file(&path)
}

/// Run every probe, writing the report to `out`. The arguments are `key=value`:
/// `read_ok`, `write_denied`, `connect`, and `read_denied` (any number of times; reported as
/// `read_denied_0`, `read_denied_1`, ...).
pub fn run(args: &[String], out: &mut impl Write) {
    let get = |key: &str| {
        args.iter().find_map(|arg| {
            arg.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
        })
    };

    // The one write that must work: the spill directory, which is the working directory.
    let spill = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    report(out, "write_spill", try_create(&spill, ".confinement-probe"));
    report(
        out,
        "write_tmp",
        try_create(&std::env::temp_dir(), ".confinement-probe"),
    );
    if let Some(path) = get("write_denied") {
        report(
            out,
            "write_elsewhere",
            try_create(&PathBuf::from(path), ".confinement-probe"),
        );
    }
    if let Some(path) = get("read_ok") {
        report(out, "read_ok", std::fs::read(path).map(drop));
    }
    for (index, path) in args
        .iter()
        .filter_map(|arg| arg.strip_prefix("read_denied="))
        .enumerate()
    {
        report(
            out,
            &format!("read_denied_{index}"),
            std::fs::read(path).map(drop),
        );
    }
    if let Some(address) = get("connect").and_then(|text| text.parse::<SocketAddr>().ok()) {
        report(
            out,
            "connect",
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).map(drop),
        );
    }
    report(
        out,
        "exec",
        Command::new("/bin/sh")
            .args(["-c", "true"])
            .status()
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(std::io::Error::other("it ran and failed"))
                }
            }),
    );
    let _ = out.flush();
}
