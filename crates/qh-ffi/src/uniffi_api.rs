//! The control plane, behind UniFFI (ADR-0004).
//!
//! ADR-0004 splits the app's two paths: the control plane — connect, introspect, run, cancel,
//! events, at low frequency — goes through UniFFI, because what it needs is *correct error and
//! type mapping*; the data plane does not, because marshalling every cell as a string is the
//! very problem this migration exists to remove. This module is only the first of those.
//!
//! # Why the events are JSON lines
//!
//! A `run` call hands back the same NDJSON the CLI writes. That is not laziness: the app already
//! parses exactly these events — the previous engine wrote them to stdout and `Engine.swift`
//! decodes them — so this surface replaces the process boundary without touching a single call
//! site. Typed events are still the next step, and doing them before there is an app-side
//! consumer would mean designing a type hierarchy against no caller.
//!
//! # Failure is one more line, not a thrown error
//!
//! A command that fails emits one `error` event and ends, exactly as the CLI does, and the sink
//! receives it like any other event. This module used to declare a typed `EngineError` instead,
//! and it went away with the change that made events stream: a `Result` can only be returned once,
//! at the end, so an error channel that is not an event cannot describe a failure that happens
//! while the caller is already painting. The app never used the type — its seven call sites read
//! the `error` event's message or the last line of the stderr argument, and `RustEngine` mapped
//! the typed error into exactly those two places — so one channel costs the app nothing and makes
//! the FFI path and the CLI path observably identical, which is what the golden corpus tests.
//!
//! What is given up is the `kind` on the error: `usage` versus `connect` versus `query`, which a
//! caller could branch on without reading the message. It has nowhere to go today — using it means
//! changing every failure path in `AppModel` — and the place it would land is a `kind` field on
//! this same `error` event, beside the message, once something reads it. The table that named the
//! variants went with the type rather than staying behind unused: which `CliError` is which kind is
//! a property of those variants, not of a mapping here.
//!
//! # Every call takes a sink and a cancel handle
//!
//! [`run`] is the shape an FFI caller can write: hand over a sink, hand over the handle that
//! stops the run, and the events arrive as the engine produces them rather than in one lump at
//! the end. Two reasons that pairing is the whole surface rather than a convenience:
//!
//! - **A sink, not a return value, because time matters.** The previous engine wrote one line per
//!   event and flushed it, so an export's progress moved while the export ran. Returning a
//!   `Vec<String>` kept every event and lost only *when* it was delivered, which is invisible in
//!   a test and obvious to someone watching a long export.
//! - **Cancel is the caller's own handle, made before the call**, because it has to reach a run
//!   that is already in flight. [`run`] blocks until the command has ended, so a handle it
//!   returned could only ever be used once there was nothing left to stop; the caller builds the
//!   [`RunCancel`], hands it in, and calls `request_cancel()` from wherever its Stop button lives.
//!   `request()` sets a flag the engine reads between rows and between statements, so a stopped
//!   export finishes the statement it is on, keeps the bytes it already wrote, and reports `done`
//!   with `cancelled: true` — the same outcome SIGTERM produces in the CLI, which is the other
//!   thing the flag exists for.
//!
//! Both are synchronous, and that is deliberate: the commands are async, so each call runs one on
//! a runtime of its own, and what matters to the caller is that one FFI call must not run on the
//! main thread. The app decides that, and it is also why the handle is a separate object: the
//! thread that is blocked in [`run`] cannot be the thread that presses Stop.
//!
//! # The events are the same events, and the order is the same order
//!
//! Nothing above changes the protocol. A `run` call emits exactly the lines the CLI writes, from
//! the same [`crate::run`] the binary calls, with a sink in place of stdout, so the two entry
//! points cannot drift — and the golden corpus, which is recorded against the CLI, still tests
//! this path ([`SinkEmitter`] hands on the same [`serde_json::Value`] both of them serialise).

use std::io;
use std::sync::Arc;

use serde_json::Value as Json;

use crate::events::{event, Emitter};
use crate::{run as run_command, CancelFlag, CliError, Command, RealEngine, Settings};

/// One setting, as the environment would have carried it.
///
/// The same keys the CLI reads (`DB_URL`, `DB_HOST`, `SQL_TEXT`, `TABLE_NAME`, …). They are
/// passed as data rather than read from the environment because an FFI caller has no
/// environment to set, and because a password in an environment variable is a password in
/// `ps` — which is exactly the leak the CLI's own settings design exists to avoid
/// (blueprint §1.2).
#[derive(Debug, Clone, uniffi::Record)]
pub struct Setting {
    pub key: String,
    pub value: String,
}

/// The commands the engine offers, as a type rather than a string.
///
/// The names match [`Command::parse`]'s, so an FFI caller and a CLI caller ask for the same
/// thing in the same word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EngineCommand {
    DbDrivers,
    Connections,
    ImportConnections,
    Credential,
    Objects,
    Test,
    Catalogs,
    Schemas,
    Tables,
    Export,
    ToTable,
    Preview,
    Count,
    Explain,
}

impl EngineCommand {
    /// The word the CLI uses, which is also the word the golden snapshots use.
    pub fn name(self) -> &'static str {
        match self {
            Self::DbDrivers => "db_drivers",
            Self::Connections => "connections",
            Self::ImportConnections => "import_connections",
            Self::Credential => "credential",
            Self::Objects => "objects",
            Self::Test => "test",
            Self::Catalogs => "catalogs",
            Self::Schemas => "schemas",
            Self::Tables => "tables",
            Self::Export => "export",
            Self::ToTable => "to_table",
            Self::Preview => "preview",
            Self::Count => "count",
            Self::Explain => "explain",
        }
    }

    /// The command, as the engine's own dispatch table knows it.
    fn as_command(self) -> Command {
        Command::parse(self.name()).expect("every variant names a command the engine knows")
    }
}

/// Where an engine's events go, implemented on the far side of the FFI.
///
/// One method, called once per event, in the order the engine produced them, with the event as
/// the JSON line the CLI writes. Called from inside the run, so the run cannot return before the
/// last event has been handed over.
///
/// A line this side could hold in a `Vec` and return instead; what the callback buys is *when*
/// it arrives. `on_event` is the Rust engine's own spelling of it — `DatabaseEngine.run`'s
/// argument is `onEvent`, and the app passes the same closure here.
#[uniffi::export(foreign)]
pub trait EventSink: Send + Sync {
    fn on_event(&self, line: String);
}

/// Every event, to the caller's sink.
struct SinkEmitter {
    sink: Arc<dyn EventSink>,
}

impl Emitter for SinkEmitter {
    fn emit(&mut self, event: Json) -> io::Result<()> {
        // Serialised here rather than in the sink, because this is the layer that writes the
        // protocol: the same compact form `JsonLines` puts on stdout, so the app decodes one
        // shape whichever entry point produced it.
        //
        // A sink that refuses the event — a UI that has moved on — still leaves the run to
        // finish. The line is a delivered message, not a result the run depends on, and
        // aborting here would turn a lost event into a failed export.
        self.sink
            .on_event(serde_json::to_string(&event).expect("an event is serialisable"));
        Ok(())
    }
}

/// The handle that stops a run, built by the caller and handed to [`run`].
///
/// A handle rather than a function that cancels "whatever is running", because this crate can
/// have more than one command in flight in one process (the app runs each tab's command on its
/// own queue), and a global cancel would stop the wrong one.
///
/// Built by the caller rather than returned, because [`run`] does not return until the command is
/// over: the caller has to be holding the handle while the run is still going. The app's own
/// `EngineRun` already has that shape — it makes the handle, keeps it, and stops it from the main
/// queue while the FFI call blocks on another.
///
/// Setting the flag is all it does. The engine reads it between rows and between statements, so
/// a stopped `export` finishes the statement it is on, keeps the bytes already written and
/// reports `done` with `cancelled: true` — a stop that loses what was written would be worse
/// than no stop button.
#[derive(Debug, Clone, Default, uniffi::Object)]
pub struct RunCancel {
    flag: CancelFlag,
}

#[uniffi::export]
impl RunCancel {
    /// A fresh handle, for one run.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Ask the run to stop. Safe to call before the engine has opened anything, after it has
    /// finished, and more than once.
    pub fn request_cancel(&self) {
        self.flag.request();
    }

    /// Whether the stop has been asked for. Readable from the far side of the FFI so a caller can
    /// show its own "stopping" state without waiting for the engine's `done`.
    pub fn is_cancelled(&self) -> bool {
        self.flag.is_cancelled()
    }
}

/// Run one command, sending each event to `sink` as it is produced.
///
/// `cancel` is the caller's own handle, the one it made before this call and keeps calling
/// `request_cancel()` on while this one is blocked: nothing here can hand a handle back in time to
/// stop the run it names (see the module note and [`RunCancel`]).
///
/// Returns nothing, which is not an omission: a run that could not be *started* is reported
/// through the sink, exactly as the CLI reports it with an `error` line, so the app keeps one
/// failure path. A run that starts and then fails does the same.
///
/// The call blocks until the command has ended, so it belongs off the main thread. The sink's
/// callbacks run on the calling thread, inside the run, and the app hops to the main queue
/// itself — the same split `DatabaseEngine`'s implementation already makes.
#[uniffi::export]
pub fn run(
    command: EngineCommand,
    settings: Vec<Setting>,
    sink: Arc<dyn EventSink>,
    cancel: Arc<RunCancel>,
) {
    let settings = Settings::from_pairs(
        settings
            .into_iter()
            .map(|setting| (setting.key, setting.value)),
    );

    let mut out = SinkEmitter { sink };

    // A runtime per call: the commands are async and the caller is not, and a shared runtime
    // would be a piece of global state whose shutdown the app cannot reason about. The call is
    // already off the main thread by the caller's own decision — see the module note.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            // Nothing was started, so there is no `error` line to write: the sink is the only
            // place this failure can be reported, and it is reported the way a command's failure
            // is, so the app has one shape to decode.
            fail(
                &mut out,
                &CliError::Internal(format!("could not start a runtime: {error}")),
            );
            return;
        }
    };

    let engine = RealEngine::with_settings(settings.clone());
    match runtime.block_on(run_command(
        command.as_command(),
        &settings,
        &mut out,
        &engine,
        &cancel.flag,
    )) {
        Ok(()) => {}
        Err(error) => fail(&mut out, &error),
    }
}

/// The one `error` event, carrying the warnings a failure already earned.
///
/// The same pairing `main.rs` writes for a failure, for the same reason: a `replace` write that
/// dropped the old table has more to report than a message, and reporting the failure cannot undo
/// the drop.
fn fail(out: &mut dyn Emitter, error: &CliError) {
    let warnings = error.warnings();
    let _ = out.emit(
        event("error")
            .field("message", error.message())
            .maybe(
                "warnings",
                (!warnings.is_empty())
                    .then(|| Json::Array(warnings.iter().map(|w| Json::from(w.clone())).collect())),
            )
            .build(),
    );
}

/// The version of this engine, for a caller that has to say what it is talking to.
///
/// The crate's own version rather than a constant that has to be remembered: the app shows it
/// in its about box, and a number that can drift from the build is worse than no number.
#[uniffi::export]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Every command this build offers, spelled as the CLI spells them.
#[uniffi::export]
pub fn command_names() -> Vec<String> {
    crate::COMMANDS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::events::JsonLines;

    fn setting(key: &str, value: &str) -> Setting {
        Setting {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    /// A sink that keeps every line it is handed, so a test can read the protocol.
    ///
    /// Cloning hands out a second handle to the *same* lines, which is what lets `run` take an
    /// owned sink while the test still reads what arrived. A `Mutex` rather than the `Cell` a
    /// single-threaded test would use: this crate forbids unsafe code and the trait is `Sync`, so
    /// the interior mutability has to be the kind that is.
    #[derive(Clone, Default)]
    struct Recorder {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl Recorder {
        fn lines(&self) -> Vec<String> {
            self.lines.lock().expect("recorded lines").clone()
        }

        /// The lines as JSON, for an assertion about a field rather than about a whole line.
        fn events(&self) -> Vec<Json> {
            self.lines()
                .iter()
                .map(|line| serde_json::from_str(line).expect("an event is JSON"))
                .collect()
        }
    }

    impl EventSink for Recorder {
        fn on_event(&self, line: String) {
            self.lines.lock().expect("recorded lines").push(line);
        }
    }

    #[test]
    fn the_sink_is_handed_each_event_in_the_shape_the_cli_writes() {
        // The one thing this layer must not get wrong: an event handed to a sink has to be the
        // same line `JsonLines` puts on stdout, because the app decodes one shape whichever entry
        // point produced it -- compact, one line, no pretty-printing.
        //
        // Asserted by running both emitters over the same events rather than against a
        // hand-written literal, because the shape *is* whatever `JsonLines` writes: a literal
        // would be a second copy of that decision, and it goes stale silently the day the writer
        // changes. Key order is the case in point -- `preserve_order` is on in this tree, so the
        // keys come out in the order the event builder inserted them (`event` first, then the
        // payload) and a sorted-literal assertion here was simply wrong about the protocol.
        let sink = Recorder::default();
        let mut to_sink = SinkEmitter {
            sink: Arc::new(sink.clone()),
        };
        let mut to_stdout = JsonLines::new(Vec::new());

        for event in [
            event("rows")
                .field("data", Json::Array(vec![Json::from(1), Json::from(2)]))
                .build(),
            event("done").field("rows", 2).build(),
        ] {
            to_stdout
                .emit(event.clone())
                .expect("a Vec cannot fail to be written to");
            to_sink
                .emit(event)
                .expect("a sink that records cannot fail");
        }

        let stdout = String::from_utf8(to_stdout.into_inner()).expect("JSON is UTF-8");
        assert_eq!(
            sink.lines(),
            stdout.lines().map(str::to_owned).collect::<Vec<_>>(),
            "the sink is handed the very lines the CLI would have written"
        );
    }

    #[test]
    fn the_cancel_handle_belongs_to_the_caller() {
        // The property the app depends on: the handle is built *before* the call, so the thread
        // that is blocked in `run` is not the thread that presses Stop. Made here, read here, and
        // handed to `run` by the same expression -- which is the only thing the compiler can check
        // (the parameter type) and this can check by value.
        let cancel = RunCancel::new();
        assert!(!cancel.is_cancelled(), "a fresh handle has stopped nothing");

        cancel.request_cancel();
        assert!(cancel.is_cancelled(), "and it stays stopped once asked");

        // Asking twice is not an error: a user presses Stop, then presses it again.
        cancel.request_cancel();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn the_commands_the_surface_offers_are_the_ones_the_cli_offers() {
        // Two lists that must not drift: the enum the app matches on, and the table the
        // usage message and `Command::parse` come from. A variant added to one and not the
        // other would be a command the app can ask for by name and not by type, or the
        // reverse.
        assert_eq!(command_names(), crate::COMMANDS.to_vec());
        for name in command_names() {
            let command = Command::parse(&name).expect("the CLI knows every name it lists");
            assert_eq!(command.name(), name);
        }
    }

    #[test]
    fn a_command_runs_through_the_ffi_path_and_hands_the_sink_the_events_the_cli_writes() {
        // The whole path in one assertion: settings as data instead of environment, dispatch,
        // events handed to the caller's own sink instead of written to stdout. `connections` is
        // the command to test it with because it reaches the local store and nothing else -- no
        // server, no network, and a database under a temporary directory the test owns.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = directory.path().join("queryhive.sqlite3");
        let sink = Recorder::default();

        run(
            EngineCommand::Connections,
            vec![setting("DB_PATH", &database.to_string_lossy())],
            Arc::new(sink.clone()),
            RunCancel::new(),
        );

        let events = sink.events();
        assert_eq!(events.len(), 1, "one event: {events:?}");
        assert_eq!(events[0]["event"], "connections");
        assert_eq!(
            events[0]["connections"].as_array().map(Vec::len),
            Some(0),
            "a database this test just created holds no connections"
        );
        // The store migrated itself on the way in, which is what makes this a real run rather
        // than a shape check.
        assert!(database.exists(), "the command opened its own database");
    }

    #[test]
    fn a_failure_arrives_as_an_error_event_rather_than_a_thrown_error() {
        // `credential` without its action is a usage failure, decided before anything is touched.
        // It arrives on the same channel as every other event: the app reads the `error` event's
        // message and the last line of the stderr argument, and never a typed error.
        let sink = Recorder::default();
        run(
            EngineCommand::Credential,
            Vec::new(),
            Arc::new(sink.clone()),
            RunCancel::new(),
        );

        let events = sink.events();
        assert_eq!(events.len(), 1, "one failure, one event: {events:?}");
        assert_eq!(events[0]["event"], "error");
        let message = events[0]["message"].as_str().unwrap_or_default();
        assert!(message.contains("CREDENTIAL_ACTION"), "{message}");
    }

    #[test]
    fn a_failure_without_warnings_omits_the_field_rather_than_sending_an_empty_array() {
        // The `warnings` pairing `main.rs` writes: present and non-empty after a `replace` that
        // dropped the old table, absent otherwise. `to_table` with no target is a usage error
        // reachable without a server, so it is the case this test can produce -- and what it
        // asserts is the *shape*, because an empty array and an absent key are two different
        // events to a decoder that has learned to read the field.
        let sink = Recorder::default();
        run(
            EngineCommand::ToTable,
            Vec::new(),
            Arc::new(sink.clone()),
            RunCancel::new(),
        );

        let events = sink.events();
        assert_eq!(events[0]["event"], "error");
        assert!(
            events[0].get("warnings").is_none(),
            "a failure with no warnings omits the field: {}",
            events[0]
        );
    }
}
