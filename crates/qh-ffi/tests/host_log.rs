//! The engine host writes its Safe Mode decisions down (W11-T1, owner decision O-27a).
//!
//! Until now only the CLI and the MCP server installed the execution log, so a decision made
//! through the app, which runs the engine in its own process, was written nowhere and the reader
//! of the log (`execution_log`) would have shown an empty page. These tests drive the host the app
//! keeps (`EngineHost::new`) with a statement the guard refuses before it connects, so no server
//! is needed.
//!
//! One file, and its tests take a lock: the sink is global to the process, which is why the host
//! keys it by the `DB_PATH` a run names, and two of these tests running at the same moment would
//! take it from each other. A test elsewhere that runs several databases at once must not depend
//! on the log.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use qh_ffi::execution_log;
use qh_ffi::host::{EngineHost, RealConnector};
use qh_ffi::uniffi_api::{EngineCommand, EventSink, RunCancel, Setting};
use serde_json::Value as Json;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl EventSink for Recorder {
    fn on_event(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }
}

/// Run one command through `host` and return its events.
fn run(host: &EngineHost, command: EngineCommand, pairs: &[(&str, &str)]) -> Vec<Json> {
    let sink = Recorder::default();
    host.run(
        command,
        pairs
            .iter()
            .map(|(key, value)| Setting {
                key: (*key).to_owned(),
                value: (*value).to_owned(),
            })
            .collect(),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    let lines = sink.0.lock().unwrap().clone();
    lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("an event is JSON"))
        .collect()
}

/// A statement `read_only` refuses before the engine connects: one `refused` decision, no server.
fn refused_delete(host: &EngineHost, database: &Path) -> Vec<Json> {
    let database = database.to_string_lossy().into_owned();
    run(
        host,
        EngineCommand::Preview,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "db.invalid"),
            ("DB_USER", "queryhive"),
            ("RETRIES", "0"),
            ("SAFE_MODE", "read_only"),
            ("SQL", "DELETE FROM t"),
            ("DB_PATH", &database),
        ],
    )
}

/// The `execution_log` event of the database `database` names.
fn read_log(host: &EngineHost, database: &Path) -> Json {
    let database = database.to_string_lossy().into_owned();
    let events = run(host, EngineCommand::ExecutionLog, &[("DB_PATH", &database)]);
    events
        .into_iter()
        .find(|event| event["event"] == "execution_log")
        .unwrap_or_else(|| panic!("no execution_log event for {database}"))
}

#[test]
fn a_refusal_through_the_host_lands_in_the_database_the_run_names() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("a.sqlite3");
    let host = EngineHost::new();

    let events = refused_delete(&host, &database);
    assert_eq!(events.last().unwrap()["event"], "error", "{events:?}");

    let log = read_log(&host, &database);
    assert_eq!(log["writer"], true, "{log}");
    assert_eq!(log["chain"]["verified"], true, "{log}");
    assert_eq!(log["chain"]["rows"], 1, "{log}");
    let decisions = log["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1, "{log}");
    let decision = &decisions[0];
    assert_eq!(decision["decision"], "refused");
    assert_eq!(decision["safe_mode"], "read_only");
    assert_eq!(decision["statement_kind"], "dml");
    assert_eq!(
        decision["statement_hash"],
        qh_storage::hash_statement("DELETE FROM t")
    );
    // The statement itself is never written, and neither is anything about the connection.
    let text = decision.to_string();
    for leaked in ["DELETE", "db.invalid", "queryhive"] {
        assert!(!text.contains(leaked), "{leaked} in {text}");
    }
    execution_log::uninstall();
}

#[test]
fn two_hosts_with_two_databases_each_read_their_own_rows() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let (a, b) = (
        directory.path().join("a.sqlite3"),
        directory.path().join("b.sqlite3"),
    );
    let (first, second) = (EngineHost::new(), EngineHost::new());

    refused_delete(&first, &a);
    refused_delete(&second, &b);
    refused_delete(&second, &b);
    // The reader follows the database it is asked about, not the one written last.
    assert_eq!(read_log(&first, &a)["chain"]["rows"], 1);
    assert_eq!(read_log(&second, &b)["chain"]["rows"], 2);
    refused_delete(&first, &a);
    assert_eq!(read_log(&first, &a)["chain"]["rows"], 2);
    assert_eq!(read_log(&second, &b)["chain"]["rows"], 2);
    execution_log::uninstall();
}

#[test]
fn the_same_path_is_not_opened_again() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("a.sqlite3");
    let host = EngineHost::new();
    let before = execution_log::sink_opens();

    for _ in 0..3 {
        refused_delete(&host, &database);
    }
    read_log(&host, &database);
    assert_eq!(
        execution_log::sink_opens() - before,
        1,
        "one open for four runs on one path"
    );

    // Another path is another open, and coming back is one more: the key follows the run.
    let other = directory.path().join("b.sqlite3");
    refused_delete(&host, &other);
    refused_delete(&host, &database);
    assert_eq!(execution_log::sink_opens() - before, 3);
    execution_log::uninstall();
}

#[test]
fn a_local_command_opens_the_log_so_the_first_run_does_not() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("a.sqlite3");
    let path = database.to_string_lossy().into_owned();
    let host = EngineHost::new();
    let before = execution_log::sink_opens();

    // The app's first local command (the session restore at launch) is where the open happens.
    let events = run(&host, EngineCommand::History, &[("DB_PATH", &path)]);
    assert_eq!(events.last().unwrap()["event"], "history", "{events:?}");
    assert_eq!(execution_log::sink_opens() - before, 1);
    assert!(
        database.exists(),
        "the log was opened for the database the command named"
    );

    // The Run that follows names the same database and finds the log already open.
    let events = refused_delete(&host, &database);
    assert_eq!(events.last().unwrap()["event"], "error", "{events:?}");
    assert_eq!(
        execution_log::sink_opens() - before,
        1,
        "the first Run opened the log again"
    );
    assert_eq!(read_log(&host, &database)["chain"]["rows"], 1);
    execution_log::uninstall();
}

#[test]
fn credential_opens_no_database() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("a.sqlite3");
    let host = EngineHost::new();
    let before = execution_log::sink_opens();

    // No action: a usage failure the engine decides before it touches the Keychain.
    let path = database.to_string_lossy().into_owned();
    let events = run(&host, EngineCommand::Credential, &[("DB_PATH", &path)]);
    assert_eq!(events.last().unwrap()["event"], "error", "{events:?}");
    assert_eq!(execution_log::sink_opens(), before);
    assert!(!database.exists(), "credential opened a database");
    execution_log::uninstall();
}

#[test]
fn a_host_over_a_fake_connector_leaves_the_database_alone() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("a.sqlite3");
    let host = EngineHost::with_connector(Arc::new(RealConnector::new()));

    let events = refused_delete(&host, &database);
    assert_eq!(events.last().unwrap()["event"], "error", "{events:?}");
    assert!(
        !database.exists(),
        "a test host opened a database it was not asked to"
    );
}

#[test]
fn a_log_that_cannot_be_opened_does_not_stop_the_guard() {
    let _serial = serial();
    execution_log::uninstall();
    let directory = tempfile::tempdir().unwrap();
    // A directory that is not there: SQLite cannot create the file.
    let database = directory.path().join("missing").join("a.sqlite3");
    let host = EngineHost::new();

    let events = refused_delete(&host, &database);
    let error = events.last().unwrap();
    assert_eq!(error["event"], "error");
    assert!(
        error["message"].as_str().unwrap().contains("read-only"),
        "the refusal is still the guard's, not the log's: {error}"
    );
    execution_log::uninstall();
}
