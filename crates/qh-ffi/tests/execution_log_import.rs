//! `import_data`'s early refusal reaches the execution log.
//!
//! Its own test binary, because the sink is process-scoped and `execution_log.rs` already
//! installs one: a second installer in the same process would make the two tests race.

use qh_ffi::events::Capture;
use qh_ffi::{execution_log, run, CancelFlag, Command, RealEngine, Settings};
use qh_storage::Storage;

fn log_database() -> Storage {
    let mut storage = Storage::in_memory().expect("in-memory database");
    storage.migrate_at(1_000).expect("migrations");
    storage
}

#[tokio::test]
async fn an_import_refused_before_it_connects_is_still_logged() {
    execution_log::install(log_database());

    // A CSV that exists, so the refusal is Safe Mode's and not a missing file. It is never
    // read: the DML check refuses before the reader opens and before the connector.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("people.csv");
    std::fs::write(&path, "id,name\n1,Ada\n").expect("write the CSV");
    let path_text = path.to_string_lossy().into_owned();

    let settings = Settings::from_pairs([
        ("DB_KIND", "trino"),
        ("DB_HOST", "coordinator.invalid"),
        ("DB_PORT", "8080"),
        ("DB_USER", "queryhive"),
        ("RETRIES", "0"),
        ("SAFE_MODE", "read_only"),
        ("IMPORT_PATH", path_text.as_str()),
        ("TARGET_CATALOG", "hive"),
        ("TARGET_SCHEMA", "default"),
        ("TARGET_TABLE", "people"),
    ]);

    let mut out = Capture::new();
    let error = run(
        Command::ImportData,
        &settings,
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .expect_err("read_only refuses an import");
    assert!(error.to_string().contains("read-only"), "{error}");

    execution_log::with_storage(|storage| {
        let rows = storage.execution_log(10).expect("read the log");
        assert_eq!(rows.len(), 1, "the early refusal is one decision: {rows:?}");
        assert_eq!(rows[0].decision, "refused");
        assert_eq!(rows[0].statement_kind, "dml");
        assert_eq!(rows[0].safe_mode, "read_only");
        assert!(
            rows[0]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("read-only"),
            "{:?}",
            rows[0].reason
        );
        // The subject is hashed like any statement, never stored as the file path.
        assert_eq!(rows[0].statement_hash.len(), 64, "a hex SHA-256");
        assert!(!rows[0].statement_hash.contains("people.csv"));
        assert_eq!(storage.verify_execution_log().unwrap(), 1);
    })
    .expect("a sink is installed");

    execution_log::uninstall();
}
