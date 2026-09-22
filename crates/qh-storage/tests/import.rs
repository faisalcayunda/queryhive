//! Importing a `connections.json` from the previous store.
//!
//! The fixtures are files rather than in-memory strings for the tests that write, because
//! the backup is part of the contract: the import must not start without one, and "it made
//! a copy" is only checkable against something that exists.
//!
//! What these prove that a unit test of the mapping cannot: that the order the file lists
//! connections in survives as `sort_order`, that the second run of an import writes
//! nothing, and that a row the importer could not read costs only itself.

use std::path::{Path, PathBuf};

use qh_storage::import::{
    already_imported, import_connections, import_history, plan_json, ImportError,
};
use qh_storage::{ConnectionKind, Storage};

/// A file that looks like the one the app writes, with the fields each kind uses.
const REALISTIC: &str = r#"[
  {
    "id": "9DB3C0CC-7062-49BB-9D47-62F765275A9B",
    "name": "Warehouse",
    "color": "violet",
    "kind": "trino",
    "host": "trino.internal",
    "port": 8443,
    "scheme": "https",
    "user": "analyst",
    "database": "hive",
    "schema": "analytics",
    "verify": false
  },
  {
    "id": "01924f1e-2f4a-7c3b-9c1e-7b0f3a5d6e80",
    "name": "Replica",
    "kind": "postgres",
    "host": "127.0.0.1",
    "port": 5432,
    "sslmode": "require",
    "user": "qh",
    "database": "qh",
    "showAllSchemas": true
  }
]"#;

struct Fixture {
    _directory: tempfile::TempDir,
    storage: Storage,
    source: PathBuf,
}

fn fixture(json: &str) -> Fixture {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let source = directory.path().join("connections.json");
    std::fs::write(&source, json).expect("write the fixture");
    let mut storage = Storage::open(directory.path().join("queryhive.sqlite3")).expect("open");
    storage.migrate_at(1_700_000_000_000).expect("migrate");
    Fixture {
        _directory: directory,
        storage,
        source,
    }
}

fn plan(fixture: &Fixture) -> qh_storage::ImportPlan {
    plan_json(&fixture.source, REALISTIC, 1_700_000_000_000).expect("a plan")
}

#[test]
fn the_fields_the_file_holds_become_the_columns_they_belong_in() {
    let fixture = fixture(REALISTIC);
    let plan = plan(&fixture);
    assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    assert_eq!(plan.connections.len(), 2);

    let trino = &plan.connections[0];
    // The id is stored in the lower-case spelling `SyncId` gives every identity, and
    // `qh-credentials` folds it back to upper case to find the Keychain item.
    assert_eq!(
        trino.meta.id.as_str(),
        "9db3c0cc-7062-49bb-9d47-62f765275a9b"
    );
    assert_eq!(trino.name, "Warehouse");
    assert_eq!(trino.kind, ConnectionKind::Trino);
    assert_eq!(trino.host.as_deref(), Some("trino.internal"));
    assert_eq!(trino.port, Some(8443));
    assert_eq!(trino.user_name.as_deref(), Some("analyst"));
    assert_eq!(trino.database_name.as_deref(), Some("hive"));
    // The account a password for this connection would be filed under, derived from the id
    // rather than looked up.
    assert_eq!(
        trino.secret_ref.as_deref(),
        Some("9db3c0cc-7062-49bb-9d47-62f765275a9b")
    );
    assert!(
        !trino.is_production,
        "a new field, and the file has no opinion"
    );
    assert!(!trino.is_read_only);

    // The fields with no column of their own, at the values the app's model would hold.
    let options: serde_json::Value = serde_json::from_str(&trino.options_json).unwrap();
    assert_eq!(options["color"], "violet");
    assert_eq!(options["scheme"], "https");
    assert_eq!(options["schema"], "analytics");
    assert_eq!(options["verify"], false);
    assert_eq!(options["showAllSchemas"], false, "the decoder's default");
    assert_eq!(
        options["sslmode"], "",
        "empty for Trino, as the app stores it"
    );

    let postgres = &plan.connections[1];
    assert_eq!(postgres.kind, ConnectionKind::Postgres);
    let options: serde_json::Value = serde_json::from_str(&postgres.options_json).unwrap();
    assert_eq!(options["color"], "blue", "the decoder's default colour");
    assert_eq!(options["scheme"], "https", "the decoder's default scheme");
    assert_eq!(options["sslmode"], "require");
    assert_eq!(options["verify"], true, "the decoder's default");
    assert_eq!(options["showAllSchemas"], true);

    // The order the file lists them in is the order the sidebar shows, and the file has
    // nowhere else to say so.
    assert_eq!(trino.sort_order, 0);
    assert_eq!(postgres.sort_order, 1);
}

#[test]
fn an_absent_field_takes_the_default_the_app_itself_would_use() {
    // Every default here was read out of the app's decoder: a port by kind, `trino` for a
    // missing kind, `blue`, `https`, `verify: true`.
    let json = r#"[{"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b", "name": "Bare"}]"#;
    let fixture = fixture(json);
    let plan = plan_json(&fixture.source, json, 1_700_000_000_000).expect("a plan");
    let record = &plan.connections[0];
    assert_eq!(record.kind, ConnectionKind::Trino);
    assert_eq!(record.port, Some(8080), "Trino's default port");
    assert_eq!(record.host, None, "an empty string is not a value");
    assert_eq!(record.user_name, None);
    assert_eq!(record.database_name, None);
    let options: serde_json::Value = serde_json::from_str(&record.options_json).unwrap();
    assert_eq!(options["color"], "blue");
    assert_eq!(options["verify"], true);

    // The port default depends on the kind, so the kind has to be read first.
    let mysql = r#"[{"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b", "name": "M", "kind": "mysql"}]"#;
    let plan = plan_json(&fixture.source, mysql, 1_700_000_000_000).expect("a plan");
    assert_eq!(plan.connections[0].port, Some(3306));

    let postgres =
        r#"[{"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b", "name": "P", "kind": "postgres"}]"#;
    let plan = plan_json(&fixture.source, postgres, 1_700_000_000_000).expect("a plan");
    assert_eq!(plan.connections[0].port, Some(5432));
}

#[test]
fn the_names_this_file_used_before_are_read_and_not_written() {
    // A file from before the app spoke to more than Trino spells the catalog `catalog` and
    // the scheme `httpScheme`. The decoder reads the new name first and the old one after,
    // so a file carrying both is read rather than rejected — and this is the only place
    // those spellings are understood.
    let legacy = r#"[{
      "id": "9db3c0cc-7062-49bb-9d47-62f765275a9b",
      "name": "Old file",
      "httpScheme": "http",
      "catalog": "hive"
    }]"#;
    let fixture = fixture(legacy);
    let plan = plan_json(&fixture.source, legacy, 1_700_000_000_000).expect("a plan");
    let record = &plan.connections[0];
    assert_eq!(record.database_name.as_deref(), Some("hive"));
    let options: serde_json::Value = serde_json::from_str(&record.options_json).unwrap();
    assert_eq!(options["scheme"], "http");

    // Both names present: the current one wins, as it does in the app.
    let both = r#"[{
      "id": "9db3c0cc-7062-49bb-9d47-62f765275a9b",
      "name": "Both",
      "database": "current",
      "catalog": "legacy",
      "scheme": "https",
      "httpScheme": "http"
    }]"#;
    let plan = plan_json(&fixture.source, both, 1_700_000_000_000).expect("a plan");
    assert_eq!(
        plan.connections[0].database_name.as_deref(),
        Some("current")
    );
    let options: serde_json::Value =
        serde_json::from_str(&plan.connections[0].options_json).unwrap();
    assert_eq!(options["scheme"], "https");
}

#[test]
fn an_unreadable_row_costs_only_itself() {
    let json = r#"[
      {"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b", "name": "Good"},
      {"name": "No id"},
      {"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b"},
      {"id": "not-a-uuid", "name": "Bad id"},
      {"id": "9db3c0cc-7062-49bb-9d47-62f765275a9b", "name": "Unknown kind", "kind": "oracle"},
      "not an object"
    ]"#;
    let fixture = fixture(json);
    let plan = plan_json(&fixture.source, json, 1_700_000_000_000).expect("a plan");

    // This is the one place the importer deliberately differs from the app, whose decoder
    // throws on the whole array: nineteen connections and one typo keep the nineteen.
    assert_eq!(plan.connections.len(), 1);
    assert_eq!(plan.connections[0].name, "Good");
    assert_eq!(plan.skipped.len(), 5);
    let reasons: Vec<&str> = plan
        .skipped
        .iter()
        .map(|skipped| skipped.reason.as_str())
        .collect();
    assert_eq!(
        plan.skipped.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5],
        "the file's own positions, so it can be repaired"
    );
    assert!(reasons[0].contains("no id"), "{reasons:?}");
    assert!(reasons[1].contains("no name"), "{reasons:?}");
    assert!(reasons[2].contains("not a UUID"), "{reasons:?}");
    assert!(reasons[3].contains("oracle"), "{reasons:?}");
    assert!(reasons[4].contains("not a JSON object"), "{reasons:?}");
}

#[test]
fn a_file_that_is_not_an_array_of_connections_is_refused_before_anything_is_written() {
    let fixture = fixture("{}");
    match plan_json(&fixture.source, "{}", 1_700_000_000_000) {
        Err(ImportError::Malformed { reason, .. }) => {
            assert!(reason.contains("not an array"), "{reason}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(plan_json(&fixture.source, "{not json", 1_700_000_000_000).is_err());
    assert!(already_imported(&fixture.storage, &fixture.source)
        .unwrap()
        .is_none());
    assert!(fixture.storage.connections().unwrap().is_empty());
}

#[test]
fn an_import_copies_the_file_aside_writes_the_rows_and_marks_itself_done() {
    let fixture = fixture(REALISTIC);
    let plan = plan(&fixture);

    let report = import_connections(&fixture.storage, &plan, 1_700_000_100_000).unwrap();
    assert!(!report.already_imported);
    assert!(report.verified);
    assert_eq!(report.written, 2);
    assert_eq!(report.kept, 0);
    assert_eq!(report.summary(), "imported 2 connection(s)");

    // The rows are there, and the sidebar order is the file's order.
    let stored = fixture.storage.connections().unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].name, "Warehouse");
    assert_eq!(stored[1].name, "Replica");
    assert_eq!(stored[0], plan.connections[0]);

    // The copy is beside the original, with the original's contents.
    let backup = report.backup.expect("a backup path");
    assert!(backup.exists(), "{backup:?}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), REALISTIC);
    assert_eq!(
        backup.file_name().unwrap().to_string_lossy(),
        "connections.json.before-import-1700000100000"
    );

    // And the marker says when.
    assert_eq!(
        already_imported(&fixture.storage, &fixture.source).unwrap(),
        Some(1_700_000_100_000)
    );
    let history = import_history(&fixture.storage).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].connections, 2);
    assert_eq!(history[0].source, fixture.source.to_string_lossy());
    fixture.storage.check_integrity().unwrap();
}

#[test]
fn importing_twice_writes_nothing_and_makes_no_second_copy() {
    // The launch path: this runs on every start, so the second run has to be the dearer
    // case — no writes, no second backup, and no second marker.
    let fixture = fixture(REALISTIC);
    let plan = plan(&fixture);
    let first = import_connections(&fixture.storage, &plan, 1_700_000_100_000).unwrap();
    let before = fixture.storage.connections_including_deleted().unwrap();

    let second = import_connections(&fixture.storage, &plan, 1_700_000_200_000).unwrap();
    assert!(second.already_imported);
    assert_eq!(second.written, 0);
    assert_eq!(second.backup, None, "no second copy of the same file");
    assert_eq!(
        fixture.storage.connections_including_deleted().unwrap(),
        before
    );
    assert_eq!(import_history(&fixture.storage).unwrap().len(), 1);
    assert_eq!(
        already_imported(&fixture.storage, &fixture.source).unwrap(),
        Some(1_700_000_100_000),
        "the date of the first import, not the second"
    );
    assert!(
        !Path::new(&format!(
            "{}.before-import-1700000200000",
            fixture.source.to_string_lossy()
        ))
        .exists(),
        "the second run made no copy"
    );
    let _ = first;
}

#[test]
fn an_import_that_finds_a_newer_row_keeps_it() {
    // The merge rule is qh-sync's, and this is the import going through it rather than
    // around it: a row edited after the file was written is not overwritten by the file.
    let fixture = fixture(REALISTIC);
    let plan = plan(&fixture);
    let mut edited = plan.connections[0].clone();
    edited.name = "Edited in the app".to_owned();
    edited.meta.touch(1_700_000_050_000);
    fixture.storage.save_connection(&edited).unwrap();

    let report = import_connections(&fixture.storage, &plan, 1_700_000_100_000).unwrap();
    assert_eq!(report.written, 1, "the other row is new");
    assert_eq!(report.kept, 1);
    assert_eq!(
        fixture
            .storage
            .connection(&edited.meta.id)
            .unwrap()
            .unwrap()
            .name,
        "Edited in the app"
    );
    assert!(
        report.summary().contains("kept 1 newer row"),
        "{}",
        report.summary()
    );
}

#[test]
fn an_empty_file_is_still_an_import_that_is_done() {
    // A user whose connections.json holds `[]` has been through this. Marking it done is
    // what stops the import from re-reading the file on every launch forever.
    let fixture = fixture("[]");
    let plan = plan_json(&fixture.source, "[]", 1_700_000_000_000).unwrap();
    let report = import_connections(&fixture.storage, &plan, 1_700_000_100_000).unwrap();
    assert_eq!(report.written, 0);
    assert!(report.verified);
    assert!(already_imported(&fixture.storage, &fixture.source)
        .unwrap()
        .is_some());
    assert_eq!(report.summary(), "imported 0 connection(s)");
}
