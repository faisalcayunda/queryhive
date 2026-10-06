//! `queryhive-analytics`: see the library docs. Started by the app as
//! `sandbox-exec -p <profile> ... queryhive-analytics --protocol 1`, with its working
//! directory set to its spill directory, an empty environment, and a pipe on each of
//! stdin, stdout and stderr.

use std::os::fd::AsFd;
use std::process::ExitCode;

use qh_analytics::server::{serve, ServeError, ServerConfig};
use qh_analytics::session::EngineConfig;
use qh_analytics_proto::{MAX_FRAME_LEN, PROTOCOL_VERSION};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some("--self-test-confinement") {
        qh_analytics::confinement::run(&args[1..], &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }

    let protocol = args
        .windows(2)
        .find(|pair| pair[0] == "--protocol")
        .and_then(|pair| pair[1].parse::<u32>().ok());
    if protocol != Some(PROTOCOL_VERSION) {
        eprintln!("queryhive-analytics: speaks protocol {PROTOCOL_VERSION}; start it with `--protocol {PROTOCOL_VERSION}`");
        return ExitCode::from(2);
    }
    // The working directory is the spill directory; an explicit one is for tests.
    let spill_dir = args
        .windows(2)
        .find(|pair| pair[0] == "--spill-dir")
        .map(|pair| std::path::PathBuf::from(&pair[1]))
        .or_else(|| std::env::current_dir().ok());

    // The raw descriptors, not `Stdin` and `Stdout`: those buffer, and the protocol is
    // exact reads and whole frames. A duplicate of the descriptor is a safe `File`.
    let input = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(std::fs::File::from);
    let output = std::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .map(std::fs::File::from);
    let (Ok(input), Ok(output)) = (input, output) else {
        eprintln!("queryhive-analytics: cannot open stdin and stdout");
        return ExitCode::from(2);
    };

    let runtime = match qh_rt::build_user_initiated("qh-sql") {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("queryhive-analytics: cannot start the runtime: {error}");
            return ExitCode::from(2);
        }
    };
    let config = ServerConfig {
        // The app refuses a helper whose build is not its own (section 14.3). The app's
        // build script sets `QH_ANALYTICS_BUILD` to its `CFBundleVersion` when it compiles
        // this binary; without it the crate version stands in.
        build: option_env!("QH_ANALYTICS_BUILD")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_owned(),
        engine: EngineConfig::new(spill_dir),
        max_chunk_bytes: MAX_FRAME_LEN as usize,
        // The app sees EOF and runs its crash path instead of waiting on a helper that
        // can no longer speak.
        on_write_failure: || std::process::exit(3),
    };
    let result = serve(input, output, runtime.handle().clone(), config);
    // Queries are abandoned, not waited for: the app is gone or has said goodbye.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(ServeError::Protocol(error)) => {
            eprintln!("queryhive-analytics: {error}");
            ExitCode::from(3)
        }
        Err(error) => {
            eprintln!("queryhive-analytics: {error}");
            ExitCode::from(2)
        }
    }
}
