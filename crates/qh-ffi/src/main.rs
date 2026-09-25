//! The `queryhive-engine` binary: the debug entry point the golden harness runs.
//!
//! Everything here is process-level and nothing here is a command: the argv check,
//! the tokio runtime, the signal handlers, and the one place an error becomes the
//! app's `error` event. The commands themselves live in [`qh_ffi`] and are shared
//! with the FFI surface, so the app and this binary cannot drift.
//!
//! # The last line of defence
//!
//! The removed `queryhive_engine.py`'s `main` ends with a comment worth keeping: the `error`
//! event is emitted from the one path that may not throw, because a failure while
//! reporting a failure leaves the app with no JSON to parse at all. The Rust
//! equivalent is here — the command runs inside `catch_unwind`, so a panic in a
//! command becomes an `error` event and exit 1 rather than a bare backtrace and
//! nothing on stdout.

use std::panic::AssertUnwindSafe;
use std::process::ExitCode;

use qh_ffi::events::{event, Emitter, JsonLines};
use qh_ffi::{run, usage, CancelFlag, CliError, Command, RealEngine, Settings};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let program = argv
        .first()
        .cloned()
        .unwrap_or_else(|| "queryhive-engine".to_owned());
    let mut out = JsonLines::new(std::io::stdout());

    // Exactly one argument, naming a command this build has. Anything else is the
    // usage message — including no argument at all, which is what a user typing the
    // program's name gets.
    let selected = match argv.get(1) {
        Some(name) if argv.len() == 2 => Command::parse(name),
        _ => None,
    };
    let Some(command) = selected else {
        let _ = out.emit(event("error").field("message", usage(&program)).build());
        return ExitCode::from(1);
    };

    let runtime = match qh_rt::build_main() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = out.emit(
                event("error")
                    .field("message", format!("internal error: {error}"))
                    .build(),
            );
            return ExitCode::from(1);
        }
    };

    let cancel = CancelFlag::new();
    install_cancel_handlers(runtime.handle(), cancel.clone());

    let settings = Settings::from_env();
    let engine = RealEngine::new();

    // Unwinding across this boundary is caught rather than allowed through: a panic
    // would otherwise leave the app with a truncated protocol and no `error` event.
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(run(command, &settings, &mut out, &engine, &cancel))
    }));

    match outcome {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(error)) => {
            report(&mut out, &error);
            ExitCode::from(1)
        }
        Err(panic) => {
            let message = panic_message(&panic);
            let _ = out.emit(
                event("error")
                    .field("message", format!("internal error: {message}"))
                    .build(),
            );
            ExitCode::from(1)
        }
    }
}

/// One `error` event, carrying the warnings a failure already earned.
fn report(out: &mut dyn Emitter, error: &CliError) {
    let warnings = error.warnings();
    let _ = out.emit(
        event("error")
            .field("message", error.message())
            // Only a failure that changed something out of sight has anything to
            // add: the DROP of a `replace`, which reporting the error cannot undo.
            .maybe(
                "warnings",
                (!warnings.is_empty()).then(|| serde_json::json!(warnings)),
            )
            .build(),
    );
}

/// What a panic said, for the `error` event. A panic payload is almost always a
/// string; anything else is reported by kind rather than dropped.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    if let Some(text) = panic.downcast_ref::<String>() {
        return text.clone();
    }
    "panicked".to_owned()
}

/// Route SIGTERM and SIGINT to the cancel flag instead of killing the process.
///
/// A cancelled export still gets to emit its `done` and close the files it already
/// wrote, which is the whole reason the flag exists. Installing a handler can fail —
/// on a platform without them — and that is not fatal: the run simply cannot be
/// cancelled from the terminal.
fn install_cancel_handlers(handle: &tokio::runtime::Handle, cancel: CancelFlag) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        for kind in [SignalKind::terminate(), SignalKind::interrupt()] {
            let cancel = cancel.clone();
            handle.spawn(async move {
                if let Ok(mut stream) = signal(kind) {
                    while stream.recv().await.is_some() {
                        cancel.request();
                    }
                }
            });
        }
    }

    #[cfg(not(unix))]
    {
        let cancel = cancel.clone();
        handle.spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel.request();
            }
        });
    }
}
