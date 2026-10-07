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
//! The last block is the row family's own claims (W12-T7a, IMPORT-2 part 1): a day-first date,
//! `NaN`, a stray comma and a lost connection against a real PostgreSQL, and MySQL's `Z`
//! against a real MySQL (`QH_TEST_MYSQL=1`, `deploy/dev/up.sh mysql`).
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
    import_on(settings().expect("a live target"), path, extra).await
}

/// [`import`] against any live target.
async fn import_on(
    env: Vec<(&'static str, &'static str)>,
    path: &Path,
    extra: &[(&str, &str)],
) -> Result<Capture, qh_ffi::CliError> {
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
    select_on(settings().expect("a live target"), sql).await
}

async fn select_on(env: Vec<(&'static str, &'static str)>, sql: &str) -> Vec<Json> {
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

// --------------------------------------------------------------------------- //
// W12-T7a: values a file spells in its own way, against a real server
// --------------------------------------------------------------------------- //

fn csv(name: &str, text: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("qh-import-live-{}-{name}.csv", std::process::id()));
    std::fs::write(&path, text).expect("write the csv");
    path
}

/// Drops its table when the test ends, however it ends (see [`Tables`] for why on a thread).
struct Scratch(&'static str);

impl Scratch {
    async fn create(table: &'static str, columns: &str) -> Scratch {
        let ddl = script(
            &format!("ddl-{table}"),
            &format!("DROP TABLE IF EXISTS {table};\nCREATE TABLE {table} ({columns});\n"),
        );
        import(&ddl, &[])
            .await
            .expect("the scratch table is created");
        Scratch(table)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let table = self.0;
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async {
                let path = script(
                    &format!("drop-{table}"),
                    &format!("DROP TABLE IF EXISTS {table};\n"),
                );
                let _ = import(&path, &[]).await;
            });
        })
        .join();
    }
}

fn into(table: &'static str) -> [(&'static str, &'static str); 2] {
    [("TARGET_TABLE", table), ("TARGET_SCHEMA", "public")]
}

#[tokio::test]
async fn a_day_first_date_lands_as_the_day_it_was_written_and_nan_lands_as_nan() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let table = "qh_vals_a";
    let _scratch = Scratch::create(
        table,
        "id integer, born date, at timestamp, price numeric(12,2), x double precision, qty integer",
    )
    .await;
    // 03/04/2024 is 3 April here; PostgreSQL's own `ISO, MDY` would have read it as 4 March.
    // 1.500,25 is one thousand five hundred and a quarter.
    let path = csv(
        "vals",
        "id,born,at,price,x,qty\n\
         1,03/04/2024,03/04/2024 17:30,\"1.500,25\",NaN,7\n\
         2,29/02/2024,31/12/2025 23:59,\"12\",-Infinity,\"8,0\"\n",
    );
    let mut with = vec![
        ("DATE_FORMAT", "dd/MM/yyyy HH:mm"),
        ("DECIMAL_SEPARATOR", ","),
        ("GROUPING_SEPARATOR", "."),
    ];
    with.extend(into(table));
    // One pattern for the file, so the date-only column needs its own pass.
    let events = import(&path, &with).await;
    let error = events.expect_err("`born` has no time, so the one pattern refuses it");
    let qh_ffi::CliError::Warned { warnings, .. } = error else {
        panic!("the import ran and failed: {}", error.message());
    };
    assert!(warnings[0].contains("column born"), "{warnings:?}");
    assert!(
        select(&format!("SELECT * FROM {table}")).await.is_empty(),
        "nothing landed"
    );

    let path = csv(
        "vals2",
        "id,at,price,x,qty\n\
         1,03/04/2024 17:30,\"1.500,25\",NaN,7\n\
         2,31/12/2025 23:59,\"12\",-Infinity,\"8,0\"\n",
    );
    let events = import(&path, &with).await.expect("imports");
    assert_eq!(events.lines.last().expect("done")["rows"], 2);
    let rows = select(&format!(
        "SELECT id, at::text, price::text, x::text, qty FROM {table} ORDER BY id"
    ))
    .await;
    assert_eq!(rows[0][1], Json::from("2024-04-03 17:30:00"), "{rows:?}");
    assert_eq!(rows[0][2], Json::from("1500.25"), "{rows:?}");
    assert_eq!(rows[0][3], Json::from("NaN"), "{rows:?}");
    assert_eq!(rows[1][1], Json::from("2025-12-31 23:59:00"), "{rows:?}");
    assert_eq!(rows[1][3], Json::from("-Infinity"), "{rows:?}");
}

#[tokio::test]
async fn a_whole_number_column_refuses_a_fraction_instead_of_rounding_it() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let table = "qh_vals_b";
    let _scratch = Scratch::create(table, "id integer, qty integer").await;
    let path = csv("qty", "id,qty\n1,5.5\n2,6\n");
    let mut skip = vec![("ON_ERROR", "skip")];
    skip.extend(into(table));
    let events = import(&path, &skip)
        .await
        .expect("skip imports the valid row");
    let done = events.lines.last().expect("done");
    assert_eq!(done["rejected"], 1, "{done}");
    let rows = select(&format!("SELECT id, qty FROM {table} ORDER BY id")).await;
    assert_eq!(rows.len(), 1, "5.5 did not become 6: {rows:?}");
    assert_eq!(rows[0][0], Json::from("2"));
}

#[tokio::test]
async fn a_stray_comma_in_row_three_never_shifts_a_value_into_the_wrong_column() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let table = "qh_vals_c";
    let _scratch = Scratch::create(table, "id integer, name text, note text").await;
    let path = csv("ragged", "id,name,note\n1,Ayu,a\n2,Budi,b, Jr\n3,Citra,c\n");

    // `stop`: nothing lands, and the message names the line and the widths.
    let mut stop = Vec::new();
    stop.extend(into(table));
    let error = import(&path, &stop)
        .await
        .expect_err("row 3 stops the import");
    assert!(
        error.message().contains("import stopped at line 3"),
        "{}",
        error.message()
    );
    assert!(
        select(&format!("SELECT id FROM {table}")).await.is_empty(),
        "rolled back"
    );

    // `skip`: the other two land, the shifted row does not.
    let mut skip = vec![("ON_ERROR", "skip")];
    skip.extend(into(table));
    let events = import(&path, &skip).await.expect("skip imports the rest");
    let done = events.lines.last().expect("done");
    assert_eq!(
        (done["rows"].as_u64(), done["rejected"].as_u64()),
        (Some(2), Some(1)),
        "{done}"
    );
    let rows = select(&format!("SELECT id, name, note FROM {table} ORDER BY id")).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1][2], Json::from("c"));
}

/// A PostgreSQL proxy that forwards the first `allowed` INSERTs and then closes both
/// sockets without forwarding the next one: the peer is gone mid-import. In-process, because
/// the shared toxiproxy publishes only its API port and the benchmark proxy (see
/// `fault_injection.rs`). Plain text only: the test connects with `DB_SSLMODE=disable`.
async fn insert_cutting_proxy(allowed: usize) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(("127.0.0.1", 55432)).await else {
                    return;
                };
                let (mut from_client, mut to_client) = client.into_split();
                let (mut from_server, mut to_server) = server.into_split();
                let upstream = async move {
                    let (mut chunk, mut inserts) = ([0u8; 16384], 0usize);
                    loop {
                        let Ok(read) = from_client.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        if chunk[..read].windows(11).any(|w| w == b"INSERT INTO") {
                            inserts += 1;
                            if inserts > allowed {
                                return;
                            }
                        }
                        if to_server.write_all(&chunk[..read]).await.is_err() {
                            return;
                        }
                    }
                };
                let downstream = async move {
                    let mut chunk = [0u8; 16384];
                    loop {
                        let Ok(read) = from_server.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 || to_client.write_all(&chunk[..read]).await.is_err() {
                            return;
                        }
                    }
                };
                tokio::select! { () = upstream => {}, () = downstream => {} }
            });
        }
    });
    port
}

#[tokio::test]
async fn skip_mode_stops_when_the_connection_is_cut_and_reports_where() {
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let table = "qh_vals_d";
    let _scratch = Scratch::create(table, "id integer, name text").await;
    let path = csv("cut", "id,name\n1,a\n2,b\n3,c\n4,d\n5,e\n");
    let proxy = insert_cutting_proxy(2).await.to_string();
    let mut extra = vec![("ON_ERROR", "skip"), ("DB_PORT", proxy.as_str())];
    extra.extend(into(table));
    let error = import(&path, &extra)
        .await
        .expect_err("a cut connection is an error, not a done");
    let message = error.message();
    // What landed is whatever was sent before the cut. The message must name the line after
    // it and say that nothing past it was attempted, and no later row may have been written.
    let rows = select(&format!("SELECT id FROM {table} ORDER BY id")).await;
    assert!(
        (1..5).contains(&rows.len()),
        "at least one row went out before the cut, and not all of them: {rows:?}"
    );
    let stopped = rows.len() + 2;
    assert!(
        message.contains(&format!("import stopped at line {stopped}")),
        "{message}"
    );
    assert!(
        message.contains(&format!("rows after line {stopped} were not attempted")),
        "{message}"
    );
}

const MYSQL: &[(&str, &str)] = &[
    ("DB_KIND", "mysql"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "53306"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("DB_SSLMODE", "disable"),
    ("RETRIES", "0"),
];

#[tokio::test]
async fn a_z_suffix_lands_in_mysql_as_utc() {
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
        return;
    }
    let table = "qh_vals_my";
    let sql = |name: &str, text: &str| script(name, text);
    let ddl = sql(
        "my-ddl",
        &format!("DROP TABLE IF EXISTS {table};\nCREATE TABLE {table} (id int, ts datetime);\n"),
    );
    import_on(MYSQL.to_vec(), &ddl, &[])
        .await
        .expect("the table is created");

    let path = csv(
        "my",
        "id,ts\n1,2024-03-04T10:00:00Z\n2,2024-03-04T10:00:00.250z\n",
    );
    let outcome = import_on(
        MYSQL.to_vec(),
        &path,
        &[("TARGET_TABLE", table), ("TARGET_SCHEMA", "qh")],
    )
    .await;
    // MySQL reads a numeric offset as a zone and converts it to the session's, so the stored
    // wall clock is the 10:00 UTC moment as the session sees it, whichever zone that is.
    let same_moment = select_on(
        MYSQL.to_vec(),
        &format!(
            "SELECT id, ts = CONVERT_TZ('2024-03-04 10:00:00', '+00:00', @@session.time_zone) \
             FROM {table} WHERE id = 1"
        ),
    )
    .await;
    let drop = sql("my-drop", &format!("DROP TABLE IF EXISTS {table};\n"));
    let _ = import_on(MYSQL.to_vec(), &drop, &[]).await;

    let events =
        outcome.unwrap_or_else(|error| panic!("MySQL refused the import: {}", error.message()));
    assert_eq!(events.lines.last().expect("done")["rows"], 2);
    assert_eq!(
        same_moment[0][1].to_string().trim_matches('"'),
        "1",
        "{same_moment:?}"
    );
}
