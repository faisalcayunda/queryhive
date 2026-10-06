//! `import_data` on a `.sql` file, against a real PostgreSQL server.
//!
//! The recorder in `import_sql.rs` proves the order of the statements the engine
//! *would* send. This file proves the two claims a fake session cannot: that
//! `SET session_replication_role = replica` really lets a child row load before
//! its parent, and that the checks are really back on after both exits — the
//! commit path and the rollback path.
//!
//! ```bash
//! QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test import_live
//! ```
//!
//! A throwaway cluster is enough:
//!
//! ```bash
//! /opt/homebrew/opt/postgresql@17/bin/initdb -D /tmp/qh-pg -U qh --auth-local=trust --auth-host=trust
//! /opt/homebrew/opt/postgresql@17/bin/pg_ctl -D /tmp/qh-pg -o "-p 55432" -l /tmp/qh-pg.log start
//! /opt/homebrew/opt/postgresql@17/bin/createdb -h 127.0.0.1 -p 55432 -U qh qh
//! ```
//!
//! Without `QH_TEST_POSTGRES=1` each test prints why it is skipping and returns,
//! for the reason `real_server.rs` gives: a green run that tested nothing is worse
//! than a visible skip.

use std::path::{Path, PathBuf};

use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, Command, RealEngine, Settings};
use serde_json::Value as Json;

const SKIP_HINT: &str = "skipped: set QH_TEST_POSTGRES=1 with a throwaway PostgreSQL on port 55432";

fn settings() -> Option<Vec<(&'static str, &'static str)>> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "55432"),
        ("DB_USER", "qh"),
        // The podman cluster requires a password from the host (scram-sha-256); a trust cluster ignores it.
        ("DB_PASSWORD", "qh-dev-only"),
        ("DB_DATABASE", "qh"),
        ("DB_SCHEMA", "public"),
        ("DB_SSLMODE", "disable"),
        ("RETRIES", "0"),
    ])
}

fn script(name: &str, text: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("qh-import-live-{}-{name}.sql", std::process::id()));
    std::fs::write(&path, text).expect("write the script");
    path
}

/// Run `import_data` with the live connection plus `extra` settings.
async fn import(path: &Path, extra: &[(&str, &str)]) -> Result<Capture, qh_ffi::CliError> {
    let env = settings().expect("a live target");
    let mut pairs: Vec<(String, String)> = env
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    pairs.push(("IMPORT_PATH".to_owned(), path.display().to_string()));
    for (key, value) in extra {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let settings = Settings::from_pairs(pairs);
    let mut out = Capture::new();
    run(
        Command::ImportData,
        &settings,
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

/// Run a read and return every row each `rows` event carried, in order.
async fn select(sql: &str) -> Vec<Json> {
    let env = settings().expect("a live target");
    let mut pairs: Vec<(String, String)> = env
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    pairs.push(("SQL".to_owned(), sql.to_owned()));
    let mut out = Capture::new();
    run(
        Command::Preview,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .unwrap_or_else(|error| panic!("preview failed: {}", error.message()));
    out.lines
        .iter()
        .filter(|event| event["event"] == "rows")
        .flat_map(|event| match &event["data"] {
            Json::Array(rows) => rows.clone(),
            other => panic!("`data` is an array of rows, got {other}"),
        })
        .collect()
}

/// The setup a test runs against its own pair of tables, so two tests in this
/// binary do not race each other's DDL when cargo runs them in parallel.
fn setup_sql(parent: &str, child: &str) -> String {
    format!(
        "DROP TABLE IF EXISTS {child};\n\
         DROP TABLE IF EXISTS {parent};\n\
         CREATE TABLE {parent} (id integer PRIMARY KEY);\n\
         CREATE TABLE {child} (id integer PRIMARY KEY, \
            parent_id integer REFERENCES {parent}(id));\n"
    )
}

async fn reset(parent: &str, child: &str) {
    import(
        &script(&format!("setup-{child}"), &setup_sql(parent, child)),
        &[],
    )
    .await
    .expect("the tables are created");
}

fn teardown_sql(parent: &str, child: &str) -> String {
    format!("DROP TABLE IF EXISTS {child};\nDROP TABLE IF EXISTS {parent};\n")
}

/// Drops the test's tables when it ends, whether it passed, failed an assertion or
/// panicked. The drop runs on its own thread and runtime because `Drop` cannot await,
/// and the test's runtime may be unwinding.
struct Tables {
    parent: &'static str,
    child: &'static str,
}

impl Tables {
    /// Drops any leftover pair from an earlier crashed run, creates the tables and
    /// returns the guard that drops them again.
    async fn create(parent: &'static str, child: &'static str) -> Tables {
        reset(parent, child).await;
        Tables { parent, child }
    }
}

impl Drop for Tables {
    fn drop(&mut self) {
        let (parent, child) = (self.parent, self.child);
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async {
                let path = script(&format!("teardown-{child}"), &teardown_sql(parent, child));
                let _ = import(&path, &[]).await;
            });
        })
        .join();
    }
}

/// A bogus child insert that must fail while the foreign key is enforced.
async fn fk_is_enforced(child: &str, name: &str) -> bool {
    let path = script(
        name,
        &format!("INSERT INTO {child} (id, parent_id) VALUES (99, 424242);\n"),
    );
    import(&path, &[]).await.is_err()
}

#[tokio::test]
async fn a_child_loads_before_its_parent_and_the_checks_come_back_on_at_commit() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let (parent, child) = ("qh_parent_a", "qh_child_a");
    let _tables = Tables::create(parent, child).await;

    // The child first, referencing a parent that does not exist yet. With the
    // checks on this is the statement a `SET session_replication_role = replica`
    // exists to allow.
    let load = script(
        "commit-load",
        &format!(
            "INSERT INTO {child} (id, parent_id) VALUES (1, 1);\n\
             INSERT INTO {parent} (id) VALUES (1);\n"
        ),
    );
    let events = import(&load, &[("FOREIGN_KEYS", "off")])
        .await
        .expect("the load succeeds with the checks off");
    let done = events.lines.last().expect("a done");
    assert_eq!(done["statements"], 2, "{done}");
    assert_eq!(done["foreign_keys"], "off", "{done}");

    let rows = select(&format!("SELECT id, parent_id FROM {child} ORDER BY id")).await;
    assert_eq!(rows.len(), 1, "the child row landed: {rows:?}");
    assert_eq!(rows[0][1], Json::from("1"));

    // Back on at commit: the same bogus insert the switch allowed now fails.
    assert!(
        fk_is_enforced(child, "commit-bogus").await,
        "the foreign key is enforced again after a commit"
    );
}

#[tokio::test]
async fn the_checks_come_back_on_after_a_rollback_too() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let (parent, child) = ("qh_parent_b", "qh_child_b");
    let _tables = Tables::create(parent, child).await;

    // The third statement is invalid SQL, so `stop` (the default) rolls the whole
    // load back — and the epilogue must still run.
    let load = script(
        "rollback-load",
        &format!(
            "INSERT INTO {child} (id, parent_id) VALUES (1, 1);\n\
             INSERT INTO {parent} (id) VALUES (1);\n\
             SELECT * FROM qh_no_such_table;\n"
        ),
    );
    let error = import(&load, &[("FOREIGN_KEYS", "off")])
        .await
        .expect_err("the bad statement stops the import");
    assert!(
        error.message().contains("rolled back"),
        "{}",
        error.message()
    );

    // Nothing landed: the rollback really rolled back.
    let rows = select(&format!("SELECT id FROM {child} ORDER BY id")).await;
    assert!(rows.is_empty(), "the child row was rolled back: {rows:?}");

    // And the checks are on again on this exit as well.
    assert!(
        fk_is_enforced(child, "rollback-bogus").await,
        "the foreign key is enforced again after a rollback"
    );
}
