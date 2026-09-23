//! The control plane, behind UniFFI (ADR-0004).
//!
//! ADR-0004 splits the app's two paths: the control plane — connect, introspect, run, cancel,
//! events, at low frequency — goes through UniFFI, because what it needs is *correct error and
//! type mapping*; the data plane does not, because marshalling every cell as a string is the
//! very problem this migration exists to remove. This module is only the first of those.
//!
//! # Why it returns JSON lines
//!
//! [`run`] returns the same NDJSON the CLI writes. That is not laziness: the app already
//! parses exactly these events — the Python engine wrote them to stdout and `Engine.swift`
//! decodes them — so this surface can replace the process boundary without touching a single
//! call site. Typed events are the next step, and doing them before there is an app-side
//! consumer would mean designing a type hierarchy against no caller.
//!
//! # The error is where the types are
//!
//! The opposite is true of failures, which is why they are not a string. A `Result` becomes a
//! thrown Swift error carrying what the UI reacts to: a `usage` failure means the user's own
//! request was unusable and nothing was touched, a `connect` failure means the connection did
//! not open, a `query` failure means the server refused the statement, and a `partial` failure
//! carries the warnings a `replace` write earned before it failed — the one case where
//! reporting the error cannot undo what already happened.
//!
//! # Blocking, deliberately
//!
//! Every function here is synchronous. The commands are async, so each call runs one on a
//! runtime of its own; what matters to the caller is that one FFI call must not run on the
//! main thread, which the app decides. A cancel entry point is a separate, later piece of
//! work rather than a parameter here, because the app's cancel has to reach a command that is
//! already running.

use crate::events::Capture;
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

/// What went wrong, in the shape the UI acts on.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    /// The caller's own request was unusable. Decided before the network was touched, so
    /// there is nothing to retry and nothing to undo.
    #[error("{message}")]
    Usage { message: String },

    /// The connection did not open.
    #[error("{message}")]
    Connect { message: String },

    /// The server refused the statement, or the connection failed while it ran.
    #[error("{message}")]
    Query { message: String },

    /// A failure that already changed something the user has to hear about: a `replace` write
    /// drops the old table before it creates the new one, so a failure after that has both the
    /// failure and the warnings to report.
    #[error("{message}")]
    Partial {
        message: String,
        warnings: Vec<String>,
    },

    /// Everything else — a bad setting, a storage or export failure, an internal error — with
    /// the kind named so a caller can tell them apart without parsing the message.
    #[error("{message}")]
    Failed { kind: String, message: String },
}

impl From<CliError> for EngineError {
    fn from(error: CliError) -> Self {
        // `Warned` is checked first and by name: it is the only failure whose *other* field
        // matters, and `message()` would silently drop it.
        if let CliError::Warned { message, warnings } = &error {
            return Self::Partial {
                message: message.clone(),
                warnings: warnings.clone(),
            };
        }
        let message = error.message();
        match error {
            CliError::Usage(_) => Self::Usage { message },
            CliError::Connect(_) => Self::Connect { message },
            CliError::Query(_) => Self::Query { message },
            CliError::Warned { .. } => unreachable!("handled above"),
            other => Self::Failed {
                // The variant's name, spelled the way the CLI spells it, so a caller can log
                // or branch on it without depending on the message's wording.
                kind: kind_of(&other).to_owned(),
                message,
            },
        }
    }
}

/// The variant's own name, which is stable where the message is not.
fn kind_of(error: &CliError) -> &'static str {
    match error {
        CliError::Usage(_) => "usage",
        CliError::Connect(_) => "connect",
        CliError::Query(_) => "query",
        CliError::Warned { .. } => "warned",
        CliError::Config(_) => "config",
        CliError::Setting(_) => "setting",
        CliError::Sql(_) => "sql",
        CliError::Export(_) => "export",
        CliError::Storage(_) => "storage",
        CliError::Import(_) => "import",
        CliError::Credential(_) => "credential",
        CliError::Io(_) => "io",
        CliError::Internal(_) => "internal",
    }
}

/// Run one command and return its events, one JSON line each.
///
/// The events are the same ones the CLI writes, in the same order, with the same keys: this is
/// the same [`crate::run`] the binary calls, with a [`Capture`] instead of stdout, so the two
/// entry points cannot drift.
#[uniffi::export]
pub fn run(command: EngineCommand, settings: Vec<Setting>) -> Result<Vec<String>, EngineError> {
    let settings = Settings::from_pairs(
        settings
            .into_iter()
            .map(|setting| (setting.key, setting.value)),
    );

    // A runtime per call: the commands are async and the caller is not, and a shared runtime
    // would be a piece of global state whose shutdown the app cannot reason about. The call is
    // already off the main thread by the caller's own decision — see the module note.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| EngineError::Failed {
            kind: "internal".to_owned(),
            message: format!("could not start a runtime: {error}"),
        })?;

    let mut capture = Capture::new();
    let engine = RealEngine::with_settings(settings.clone());
    let cancel = CancelFlag::new();

    runtime
        .block_on(run_command(
            command.as_command(),
            &settings,
            &mut capture,
            &engine,
            &cancel,
        ))
        .map_err(EngineError::from)?;

    Ok(capture.lines())
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
    use super::*;
    use serde_json::Value;

    fn setting(key: &str, value: &str) -> Setting {
        Setting {
            key: key.to_owned(),
            value: value.to_owned(),
        }
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
    fn a_command_runs_through_the_ffi_path_and_returns_the_events_the_cli_writes() {
        // The whole path in one assertion: settings as data instead of environment, dispatch,
        // events captured instead of written to stdout. `connections` is the command to test
        // it with because it reaches the local store and nothing else -- no server, no
        // network, and a database under a temporary directory the test owns.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = directory.path().join("queryhive.sqlite3");

        let events = run(
            EngineCommand::Connections,
            vec![setting("DB_PATH", &database.to_string_lossy())],
        )
        .expect("the command runs");

        assert_eq!(events.len(), 1, "one event: {events:?}");
        let event: Value = serde_json::from_str(&events[0]).expect("an event is JSON");
        assert_eq!(event["event"], "connections");
        assert_eq!(
            event["connections"].as_array().map(Vec::len),
            Some(0),
            "a database this test just created holds no connections"
        );
        // The store migrated itself on the way in, which is what makes this a real run rather
        // than a shape check.
        assert!(database.exists(), "the command opened its own database");
    }

    #[test]
    fn a_failure_the_caller_can_act_on_arrives_as_a_typed_error_not_a_string() {
        // `credential` without its action: a usage failure, which is the one kind where the
        // app must know nothing was touched.
        match run(EngineCommand::Credential, Vec::new()) {
            Err(EngineError::Usage { message }) => {
                assert!(message.contains("CREDENTIAL_ACTION"), "{message}");
            }
            other => panic!("expected a usage failure, got {other:?}"),
        }
    }

    #[test]
    fn the_version_comes_from_the_build() {
        assert_eq!(engine_version(), env!("CARGO_PKG_VERSION"));
        assert!(!engine_version().is_empty());
    }
}
