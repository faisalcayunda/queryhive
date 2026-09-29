//! The three local commands, against a throwaway database and a throwaway file.
//!
//! Every test here points `DB_PATH` at a `tempfile` database and `LEGACY_PATH` at a
//! `tempfile` directory, so nothing the user owns is read or written. The one exception
//! is the Keychain round trip, which is gated on `QH_TEST_KEYCHAIN=1` for the reason
//! `qh-credentials`' own keychain test is: an unattended run that writes to a login
//! keychain raises a permission dialog, and a test that waits for a human to answer it
//! hangs.
//!
//! ```bash
//! QH_TEST_KEYCHAIN=1 cargo test -p qh-ffi --test local
//! ```
//!
//! # Why the events are asserted key by key rather than as one literal
//!
//! An event is a protocol: a decoder in the app reads named keys, and a snapshot of the
//! whole line would fail on a change that no decoder would notice while passing on a
//! change that broke every caller — a key renamed, or `options` arriving as text again.
//! The tests below assert the shape that matters, including the key set itself.

use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, RealEngine, Settings};
use qh_storage::{ConnectionKind, ConnectionRecord, Storage};
use qh_sync::SyncId;
use serde_json::{json, Value as Json};

const SKIP_HINT: &str = "skipped: set QH_TEST_KEYCHAIN=1 to exercise the real login keychain";

/// One command, driven the way the binary drives it, with its events in memory.
async fn events(command: Command, settings: &[(&str, &str)]) -> Result<Vec<Json>, CliError> {
    let settings = Settings::from_pairs(settings.iter().map(|(key, value)| (*key, *value)));
    let mut out = Capture::new();
    run(
        command,
        &settings,
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await?;
    Ok(out.lines)
}

/// One command's single event, which is all three local commands ever emit.
async fn one(command: Command, settings: &[(&str, &str)]) -> Json {
    let mut events = events(command, settings)
        .await
        .unwrap_or_else(|error| panic!("{command:?} failed: {error}"));
    assert_eq!(events.len(), 1, "{command:?} emits exactly one event");
    events.remove(0)
}

/// A migrated database at `path`, which is what `DB_PATH` will be pointed at.
fn database_at(path: &std::path::Path) -> Storage {
    let mut storage = Storage::open(path).expect("open the temp database");
    storage.migrate().expect("migrate the temp database");
    storage
}

// --------------------------------------------------------------------------- //
// connections
// --------------------------------------------------------------------------- //

/// The row written below, and the two that must not reach the event.
fn seed(storage: &Storage) {
    let mut alive = ConnectionRecord::new("Analytics", ConnectionKind::Trino, 1_700_000_000_000);
    alive.host = Some("trino.internal".to_owned());
    alive.port = Some(8443);
    alive.user_name = Some("faisal".to_owned());
    alive.database_name = Some("hive".to_owned());
    alive.options_json = r#"{"color":"green","verify":true}"#.to_owned();
    alive.is_production = true;
    alive.secret_ref = Some(alive.meta.id.to_string());
    alive.sort_order = 2;
    storage.save_connection(&alive).expect("save the live row");

    // Every nullable column left unset, so the event has to carry nulls rather than
    // empty strings. Its `sort_order` puts it first, so the order of the event is a
    // consequence of the store's order and not of the order these were written in.
    let mut bare = ConnectionRecord::new("Bare", ConnectionKind::Postgres, 1_700_000_000_001);
    bare.sort_order = 1;
    storage.save_connection(&bare).expect("save the bare row");

    let gone = ConnectionRecord::new("Gone", ConnectionKind::Mysql, 1_700_000_000_002);
    storage
        .save_connection(&gone)
        .expect("save the row to delete");
    storage
        .soft_delete_connection(&gone.meta.id, 1_700_000_000_003)
        .expect("delete it");
}

#[tokio::test]
async fn connections_lists_living_rows_with_parsed_options() {
    let dir = tempfile::tempdir().expect("a temp directory");
    let db = dir.path().join("queryhive.sqlite3");
    seed(&database_at(&db));
    let db_path = db.to_string_lossy().into_owned();

    let event = one(Command::Connections, &[("DB_PATH", db_path.as_str())]).await;

    assert_eq!(event["event"], json!("connections"));
    let listed = event["connections"].as_array().expect("an array of rows");
    assert_eq!(listed.len(), 2, "the deleted row is not listed");

    // `sort_order` decides the order, not the name and not the order of the writes.
    assert_eq!(listed[0]["name"], json!("Bare"));
    assert_eq!(listed[1]["name"], json!("Analytics"));

    // A connection that uses none of these carries nulls, not empty strings.
    for missing in ["host", "port", "user", "database", "secret_ref"] {
        assert_eq!(listed[0][missing], Json::Null, "{missing} is null");
    }
    assert_eq!(listed[0]["options"], json!({}));
    assert_eq!(listed[0]["kind"], json!("postgres"));
    assert_eq!(listed[0]["is_production"], json!(false));
    assert_eq!(listed[0]["is_read_only"], json!(false));
    assert_eq!(listed[0]["deleted"], json!(false));
    assert_eq!(listed[0]["version"], json!(1));
    assert_eq!(listed[0]["sort_order"], json!(1));

    assert_eq!(listed[1]["kind"], json!("trino"));
    assert_eq!(listed[1]["host"], json!("trino.internal"));
    assert_eq!(listed[1]["port"], json!(8443));
    assert_eq!(listed[1]["user"], json!("faisal"));
    assert_eq!(listed[1]["database"], json!("hive"));
    assert_eq!(listed[1]["is_production"], json!(true));
    assert_eq!(listed[1]["secret_ref"], json!(listed[1]["id"]));
    assert_eq!(listed[1]["sort_order"], json!(2));
    // The single most important line in this test: `options` is the parsed object, not
    // the JSON text it is stored as, because a caller that receives a string here has to
    // parse it back and one that receives `null` shows the user nothing.
    let options = &listed[1]["options"];
    assert!(
        options.is_object(),
        "options is an object, not the stored text: {options}"
    );
    assert_eq!(options["color"], json!("green"));
    assert_eq!(options["verify"], json!(true));

    // The key set is the shape a decoder is written against, so it is pinned exactly.
    let mut keys: Vec<&str> = listed[0]
        .as_object()
        .expect("a row is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "database",
            "deleted",
            "host",
            "id",
            "is_production",
            "is_read_only",
            "kind",
            "name",
            "options",
            "port",
            "secret_ref",
            "sort_order",
            "user",
            "version",
        ]
    );

    // The id is the row's own identity, which is what a caller feeds back into
    // `credential` as the Keychain account.
    let id = listed[0]["id"].as_str().expect("an id");
    assert!(SyncId::parse(id).is_ok(), "{id} is a UUID");
}

#[tokio::test]
async fn connections_says_so_when_the_database_is_empty() {
    let dir = tempfile::tempdir().expect("a temp directory");
    let db = dir.path().join("queryhive.sqlite3");
    database_at(&db);
    let db_path = db.to_string_lossy().into_owned();

    // A database with no rows yet is not a failure: it is what a fresh install has.
    let event = one(Command::Connections, &[("DB_PATH", db_path.as_str())]).await;
    assert_eq!(event["connections"], json!([]));
}

// --------------------------------------------------------------------------- //
// import_connections
// --------------------------------------------------------------------------- //

/// One readable row, and one that is not — a row with no name, which the store skips
/// and reports rather than letting it take the other connection down with it.
const LEGACY_JSON: &str = r#"[
  {
    "id": "b7c8d9e0-1111-4222-8333-444455556666",
    "name": "Analytics",
    "kind": "trino",
    "host": "trino.internal",
    "port": 8443,
    "user": "faisal",
    "database": "hive"
  },
  {
    "id": "c8d9e0f1-2222-4333-8444-555566667777",
    "kind": "trino"
  }
]"#;

/// The two paths a test needs: the database the import writes and the file it reads.
///
/// Owned strings rather than paths borrowed from a temporary, so `settings` can hand out
/// `&str` pairs that live as long as the fixture does.
struct Paths {
    _dir: tempfile::TempDir,
    db: String,
    legacy: String,
}

fn legacy_fixture(contents: Option<&str>) -> Paths {
    let dir = tempfile::tempdir().expect("a temp directory");
    let db = dir.path().join("queryhive.sqlite3");
    let legacy = dir.path().join("connections.json");
    if let Some(contents) = contents {
        std::fs::write(&legacy, contents).expect("write the legacy file");
    }
    Paths {
        _dir: dir,
        db: db.to_string_lossy().into_owned(),
        legacy: legacy.to_string_lossy().into_owned(),
    }
}

impl Paths {
    fn settings(&self) -> Vec<(&str, &str)> {
        vec![
            ("DB_PATH", self.db.as_str()),
            ("LEGACY_PATH", self.legacy.as_str()),
        ]
    }

    /// The directory the legacy file lives in, which is also where its backup goes.
    fn directory(&self) -> &std::path::Path {
        std::path::Path::new(&self.legacy)
            .parent()
            .expect("the legacy file has a parent")
    }

    /// How many copies of the file an import has left beside it.
    fn backups(&self) -> usize {
        std::fs::read_dir(self.directory())
            .expect("the temp directory")
            .filter(|entry| {
                entry
                    .as_ref()
                    .map(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .contains("before-import")
                    })
                    .unwrap_or(false)
            })
            .count()
    }

    /// The imported row, read back through the store the command wrote it with.
    fn stored_connection(&self) -> ConnectionRecord {
        let storage = database_at(std::path::Path::new(&self.db));
        let id = SyncId::parse("b7c8d9e0-1111-4222-8333-444455556666").expect("a UUID");
        storage
            .connection(&id)
            .expect("read the row back")
            .expect("the imported row is there")
    }
}

#[tokio::test]
async fn import_connections_writes_once_and_says_so_the_second_time() {
    let paths = legacy_fixture(Some(LEGACY_JSON));
    let settings = paths.settings();

    let first = one(Command::ImportConnections, &settings).await;
    assert_eq!(first["event"], json!("import"));
    assert_eq!(first["source"], json!(paths.legacy));
    assert_eq!(first["source_found"], json!(true));
    assert_eq!(first["already_imported"], json!(false));
    assert_eq!(first["written"], json!(1));
    assert_eq!(first["kept"], json!(0));
    assert_eq!(first["verified"], json!(true));
    // The unreadable row is reported with its position in the file, so the file can be
    // repaired — and the readable row is imported anyway.
    assert_eq!(first["skipped"], json!([{"index": 1, "reason": "no name"}]));
    let backup = first["backup"]
        .as_str()
        .expect("the source was copied aside before it was read");
    assert!(
        std::path::Path::new(backup).is_file(),
        "the backup is on disk at {backup}"
    );

    let imported = paths.stored_connection();
    assert_eq!(paths.backups(), 1);

    let second = one(Command::ImportConnections, &settings).await;
    assert_eq!(second["source_found"], json!(true));
    assert_eq!(second["already_imported"], json!(true));
    assert_eq!(second["written"], json!(0));
    assert_eq!(second["kept"], json!(0));
    assert_eq!(second["verified"], json!(true));
    assert_eq!(second["backup"], Json::Null, "no second copy is made");
    assert_eq!(second["skipped"], first["skipped"]);

    // Nothing was written a second time: the row is the same revision it was, and no new
    // backup appeared beside the source.
    assert_eq!(paths.stored_connection(), imported, "the row is untouched");
    assert_eq!(paths.stored_connection().meta.version.get(), 1);
    assert_eq!(paths.backups(), 1);
}

#[tokio::test]
async fn a_missing_legacy_file_is_reported_and_not_a_failure() {
    // The fixture is not written: this is a machine where the app never saved a
    // connection, which is a normal launch and has to say so rather than fail.
    let paths = legacy_fixture(None);
    let settings = paths.settings();

    let event = one(Command::ImportConnections, &settings).await;
    assert_eq!(event["event"], json!("import"));
    // The path is still reported — it is where a file would have been read from.
    assert_eq!(event["source"], json!(paths.legacy));
    assert_eq!(event["source_found"], json!(false));
    assert_eq!(event["already_imported"], json!(false));
    assert_eq!(event["written"], json!(0));
    assert_eq!(event["kept"], json!(0));
    assert_eq!(event["verified"], json!(false), "nothing was verified");
    assert_eq!(event["backup"], Json::Null);
    assert_eq!(event["skipped"], json!([]));

    // And nothing was written to the database: it is still an empty schema.
    let storage = database_at(std::path::Path::new(&paths.db));
    assert!(storage.connections().expect("connections").is_empty());
}

// --------------------------------------------------------------------------- //
// credential
// --------------------------------------------------------------------------- //

/// One `credential` action. `store` is the value for `CREDENTIAL_STORE`, which is how the
/// memory seam is selected; `None` leaves the real Keychain in place.
async fn credential_action(
    store: Option<&str>,
    account: &str,
    action: &str,
    password: Option<&str>,
) -> Json {
    let mut pairs = vec![("CONNECTION_ID", account), ("CREDENTIAL_ACTION", action)];
    if let Some(store) = store {
        pairs.push(("CREDENTIAL_STORE", store));
    }
    if let Some(password) = password {
        pairs.push(("DB_PASSWORD", password));
    }
    one(Command::Credential, &pairs).await
}

#[tokio::test]
async fn credential_round_trips_through_the_memory_store() {
    // A UUID, so the test also covers the account spelling the app uses, and fixed
    // rather than generated so a failure leaves one item to look at rather than a new one
    // per run. It is not any connection's id.
    let account = "5a1c9d2e-0000-4000-8000-00000000feed";

    let before = credential_action(Some("memory"), account, "has", None).await;
    assert_eq!(before["event"], json!("credential"));
    assert_eq!(before["action"], json!("has"));
    assert_eq!(before["account"], json!(account));
    assert_eq!(before["exists"], json!(false), "nothing is stored yet");
    assert!(
        before.get("secret").is_none(),
        "`has` never carries a secret"
    );

    let set = credential_action(Some("memory"), account, "set", Some("hunter2")).await;
    assert_eq!(set["exists"], json!(true), "`set` leaves one stored");
    assert!(set.get("secret").is_none(), "`set` never carries a secret");

    let after = credential_action(Some("memory"), account, "has", None).await;
    assert_eq!(after["exists"], json!(true));
    // Which is the difference between the two: `has` answers the question and stops.
    assert!(
        after.get("secret").is_none(),
        "`has` never carries a secret"
    );

    let got = credential_action(Some("memory"), account, "get", None).await;
    assert_eq!(got["action"], json!("get"));
    assert_eq!(got["exists"], json!(true));
    assert_eq!(got["secret"], json!("hunter2"));

    // The password is only set by `set`: a `get` with no `DB_PASSWORD` must not write
    // one back, and a `delete` must not need one.
    let deleted = credential_action(Some("memory"), account, "delete", None).await;
    assert_eq!(deleted["exists"], json!(false));
    assert!(deleted.get("secret").is_none());

    let absent = credential_action(Some("memory"), account, "get", None).await;
    assert_eq!(absent["exists"], json!(false));
    // The key is there and null: a decoder reading `get` should not have to treat a
    // missing key differently from an empty one.
    assert_eq!(absent["secret"], Json::Null);

    let gone = credential_action(Some("memory"), account, "has", None).await;
    assert_eq!(gone["exists"], json!(false));
}

#[tokio::test]
async fn a_set_without_a_password_is_refused() {
    let account = "5a1c9d2e-0000-4000-8000-00000000cafe";
    let error = events(
        Command::Credential,
        &[
            ("CREDENTIAL_STORE", "memory"),
            ("CONNECTION_ID", account),
            ("CREDENTIAL_ACTION", "set"),
        ],
    )
    .await
    .expect_err("no DB_PASSWORD is a usage error");
    assert_eq!(
        error.message(),
        "DB_PASSWORD is required to set a credential"
    );

    // An action nobody offers is refused by name, with the four that exist.
    let error = events(
        Command::Credential,
        &[
            ("CREDENTIAL_STORE", "memory"),
            ("CONNECTION_ID", account),
            ("CREDENTIAL_ACTION", "peek"),
        ],
    )
    .await
    .expect_err("an unknown action is a usage error");
    assert_eq!(
        error.message(),
        "unknown CREDENTIAL_ACTION 'peek'; expected has, get, set or delete"
    );
}

/// The same round trip against the real login Keychain.
///
/// Gated, because it is the only test in this crate that leaves the process: the item it
/// writes is named after the process id and is deleted at the end of the round trip, but
/// the Keychain's own permission dialog is still a thing a machine running CI cannot
/// answer.
#[tokio::test]
async fn credential_round_trips_through_the_real_keychain() {
    if std::env::var("QH_TEST_KEYCHAIN").as_deref() != Ok("1") {
        eprintln!("{SKIP_HINT}");
        return;
    }

    // Shaped like a UUID and derived from the process id, so a crashed run leaves one
    // item to remove and never collides with the app's own.
    let account = format!(
        "7f3d2a10-{:04x}-4c8e-9f01-2b7c5d6e8a91",
        std::process::id() & 0xffff
    );

    // The Keychain is the default store, so this is what a caller with no
    // `CREDENTIAL_STORE` gets.
    let _ = credential_action(None, &account, "delete", None).await;

    let before = credential_action(None, &account, "has", None).await;
    assert_eq!(before["exists"], json!(false));

    let set = credential_action(None, &account, "set", Some("pässword with space")).await;
    assert_eq!(set["exists"], json!(true));

    let got = credential_action(None, &account, "get", None).await;
    assert_eq!(got["secret"], json!("pässword with space"));

    let deleted = credential_action(None, &account, "delete", None).await;
    assert_eq!(deleted["exists"], json!(false));
    let after = credential_action(None, &account, "has", None).await;
    assert_eq!(after["exists"], json!(false));
}

/// A refusal names what to use instead, not only what does not exist.
///
/// The previous engine's refusal carried the hint and this one did not, which left the
/// user to guess that `catalogs` is where MySQL keeps its databases. The sentence is built
/// from the levels the driver declares rather than written per engine, so it stays right
/// for a driver that gains or loses a level without anyone editing a string.
#[tokio::test]
async fn a_refused_level_names_the_commands_that_work_instead() {
    // MySQL declares a database level and a table level, so `schemas` is the command with
    // nowhere to go -- and the two that work are the two the sentence names.
    let error = events(
        Command::Schemas,
        &[("DB_KIND", "mysql"), ("DB_HOST", "127.0.0.1")],
    )
    .await
    .expect_err("mysql has no schema level");
    assert_eq!(
        error.message(),
        "mysql has no schema level; use catalogs or tables instead"
    );

    // And the gate does not speak for the driver on a listing level. PostgreSQL's tree
    // starts at schemas and `catalogs` is a command it answers -- that is the pair the
    // previous engine's own test asserts -- so the refusal must not happen here, before the
    // driver is asked. Nothing listens on port 1, so what comes back is the connection
    // failing: the proof that the command was handed over rather than pre-empted.
    let error = events(
        Command::Catalogs,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "127.0.0.1"),
            ("DB_PORT", "1"),
            // `RETRIES=0` because the default would retry this connect five times and
            // spend the full 62 s of backoff to prove the same sentence — that the
            // command reached the driver. See `crates/qh-ffi/src/retry.rs`.
            ("RETRIES", "0"),
        ],
    )
    .await
    .expect_err("nothing listens on port 1");
    assert!(
        !matches!(error, CliError::Usage(_)),
        "a listing level a driver answers has to reach the driver: {error:?}"
    );
}

// --------------------------------------------------------------------------- //
// history and saved queries
// --------------------------------------------------------------------------- //

/// A migrated database under a temporary directory, and the `DB_PATH` that points at it.
///
/// The directory is handed back because dropping it deletes the database, and a test that
/// dropped it early would be reading a file that is not there.
fn scratch() -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("queryhive.sqlite3");
    (directory, path.to_string_lossy().into_owned())
}

#[tokio::test]
async fn history_add_records_and_history_lists_it() {
    let (_directory, database) = scratch();
    let connection = SyncId::now();

    let written = one(
        Command::HistoryAdd,
        &[
            ("DB_PATH", database.as_str()),
            ("CONNECTION_ID", connection.as_str()),
            ("SQL", "SELECT 1"),
            ("STARTED_AT", "1700000000000"),
            ("ELAPSED_MS", "12"),
            ("ROW_COUNT", "1"),
            ("OUTCOME", "ok"),
        ],
    )
    .await;
    assert_eq!(written["event"], "history_entry");
    assert_eq!(
        written["merged"], false,
        "nothing was stored to merge into, so this is the record's own id"
    );

    let listed = one(Command::History, &[("DB_PATH", database.as_str())]).await;
    assert_eq!(listed["event"], "history");
    let entries = listed["entries"].as_array().expect("an array of entries");
    assert_eq!(entries.len(), 1, "one execution, one entry: {listed}");
    assert_eq!(entries[0]["id"], written["id"]);
    assert_eq!(entries[0]["sql"], "SELECT 1");
    assert_eq!(entries[0]["outcome"], "ok");
    assert_eq!(entries[0]["row_count"], 1);
    assert_eq!(entries[0]["elapsed_ms"], 12);
    assert_eq!(entries[0]["connection_id"], connection.as_str());
    assert_eq!(entries[0]["deleted"], false);
}

#[tokio::test]
async fn a_blank_elapsed_ms_is_a_null_and_not_a_zero() {
    // The app sets the settings it knows about, so a key that is present and empty means
    // "nothing to say" -- the same convention `CONNECTION_ID` follows. Reading it as zero
    // would report that a query took no time, which is a fact about a different query.
    let (_directory, database) = scratch();
    one(
        Command::HistoryAdd,
        &[
            ("DB_PATH", database.as_str()),
            ("SQL", "SELECT 1"),
            ("STARTED_AT", "1700000000000"),
            ("ELAPSED_MS", ""),
            ("ROW_COUNT", ""),
        ],
    )
    .await;

    let listed = one(Command::History, &[("DB_PATH", database.as_str())]).await;
    let entry = &listed["entries"][0];
    assert_eq!(entry["elapsed_ms"], Json::Null);
    assert_eq!(entry["row_count"], Json::Null);
    assert_eq!(entry["outcome"], Json::Null);
}

#[tokio::test]
async fn history_add_merges_two_saves_of_one_event() {
    // `idx_history_dedupe` makes two saves of one event one event, and the reply is the only
    // place a caller learns that its write landed on a row that was already there.
    let (_directory, database) = scratch();
    let settings = [
        ("DB_PATH", database.as_str()),
        ("SQL", "SELECT 1"),
        ("STARTED_AT", "1700000000000"),
    ];

    let first = one(Command::HistoryAdd, &settings).await;
    let second = one(Command::HistoryAdd, &settings).await;
    assert_eq!(first["merged"], false);
    assert_eq!(second["merged"], true, "the second save is the same event");
    assert_eq!(first["id"], second["id"]);

    let listed = one(Command::History, &[("DB_PATH", database.as_str())]).await;
    assert_eq!(listed["entries"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn history_clear_counts_only_what_was_living() {
    let (_directory, database) = scratch();
    for offset in 0..2i64 {
        let started = (1_700_000_000_000i64 + offset).to_string();
        one(
            Command::HistoryAdd,
            &[
                ("DB_PATH", database.as_str()),
                ("SQL", "SELECT 1"),
                ("STARTED_AT", started.as_str()),
            ],
        )
        .await;
    }

    let cleared = one(Command::HistoryClear, &[("DB_PATH", database.as_str())]).await;
    assert_eq!(cleared["cleared"], 2);
    let again = one(Command::HistoryClear, &[("DB_PATH", database.as_str())]).await;
    assert_eq!(
        again["cleared"], 0,
        "a tombstone is not cleared a second time"
    );

    let listed = one(Command::History, &[("DB_PATH", database.as_str())]).await;
    assert_eq!(listed["entries"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn saved_queries_saves_lists_renames_and_deletes() {
    let (_directory, database) = scratch();
    let saved = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "save"),
            ("NAME", "Penerima 2026"),
            ("SQL", "SELECT * FROM penerima_manfaat"),
        ],
    )
    .await;
    assert_eq!(saved["action"], "save");
    let id = saved["query"]["id"]
        .as_str()
        .expect("an identity")
        .to_owned();

    let listed = one(
        Command::SavedQueries,
        &[("DB_PATH", database.as_str()), ("SAVED_ACTION", "list")],
    )
    .await;
    let queries = listed["queries"].as_array().expect("an array of queries");
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0]["name"], "Penerima 2026");
    assert_eq!(queries[0]["deleted"], false);

    let renamed = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "rename"),
            ("SAVED_ID", id.as_str()),
            ("NAME", "Penerima 2026 final"),
        ],
    )
    .await;
    assert_eq!(renamed["renamed"], true);

    let read = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "get"),
            ("SAVED_ID", id.as_str()),
        ],
    )
    .await;
    assert_eq!(read["query"]["name"], "Penerima 2026 final");
    assert_eq!(
        read["query"]["version"], 2,
        "the name moved with the revision, which is what a sync reads"
    );

    let deleted = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "delete"),
            ("SAVED_ID", id.as_str()),
        ],
    )
    .await;
    assert_eq!(deleted["deleted"], true);

    let empty = one(
        Command::SavedQueries,
        &[("DB_PATH", database.as_str()), ("SAVED_ACTION", "list")],
    )
    .await;
    assert_eq!(empty["queries"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn history_searches_the_statements_it_holds() {
    let (_directory, database) = scratch();
    for (sql, started) in [
        ("SELECT * FROM penerima_manfaat", 1_700_000_000_000i64),
        ("SELECT * FROM pengguna", 1_700_000_000_001),
    ] {
        let started = started.to_string();
        one(
            Command::HistoryAdd,
            &[
                ("DB_PATH", database.as_str()),
                ("SQL", sql),
                ("STARTED_AT", started.as_str()),
            ],
        )
        .await;
    }

    // A search is the third question the listing can answer, beside "the newest" and "one
    // connection's": the app's own search field sets this key.
    let found = one(
        Command::History,
        &[
            ("DB_PATH", database.as_str()),
            ("HISTORY_SEARCH", "penerima"),
        ],
    )
    .await;
    let entries = found["entries"].as_array().expect("an array of entries");
    assert_eq!(
        entries.len(),
        1,
        "one of the two statements matches: {found}"
    );
    assert_eq!(entries[0]["sql"], "SELECT * FROM penerima_manfaat");

    // Punctuation that would be FTS5 syntax if it were passed through: it has to come back as an
    // empty result, not as a failed command.
    let punctuation = one(
        Command::History,
        &[("DB_PATH", database.as_str()), ("HISTORY_SEARCH", "(*")],
    )
    .await;
    assert_eq!(punctuation["entries"].as_array().map(Vec::len), Some(0));

    // And a blank field is "no search", which is the whole list.
    let blank = one(
        Command::History,
        &[("DB_PATH", database.as_str()), ("HISTORY_SEARCH", "")],
    )
    .await;
    assert_eq!(blank["entries"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn a_saved_query_can_be_favourited_and_the_listing_says_so() {
    let (_directory, database) = scratch();
    let saved = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "save"),
            ("NAME", "Penerima 2026"),
            ("SQL", "SELECT * FROM penerima_manfaat"),
        ],
    )
    .await;
    let id = saved["query"]["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        saved["query"]["favourite"], false,
        "a new query is not a favourite"
    );

    // No `FAVOURITE` at all means "keep it". A caller that only knows how to add a star should not
    // have to spell the value as well.
    let marked = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "favourite"),
            ("SAVED_ID", id.as_str()),
        ],
    )
    .await;
    assert_eq!(
        marked["favourite"], true,
        "a bare favourite means keep it: {marked}"
    );
    assert_eq!(marked["found"], true);

    let listed = one(
        Command::SavedQueries,
        &[("DB_PATH", database.as_str()), ("SAVED_ACTION", "list")],
    )
    .await;
    assert_eq!(
        listed["queries"][0]["favourite"], true,
        "the listing carries the flag"
    );

    let cleared = one(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "favourite"),
            ("SAVED_ID", id.as_str()),
            ("FAVOURITE", "0"),
        ],
    )
    .await;
    assert_eq!(cleared["favourite"], false, "and it can be taken back off");

    // A third value is refused by name rather than read as false: "maybe" is a caller's bug, and
    // silently un-favouriting a query is the worst possible way to report it.
    let error = events(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "favourite"),
            ("SAVED_ID", id.as_str()),
            ("FAVOURITE", "maybe"),
        ],
    )
    .await
    .expect_err("a favourite is one of two answers");
    assert!(error.message().contains("unknown FAVOURITE"), "{error:?}");
}

#[tokio::test]
async fn the_new_local_commands_refuse_what_they_cannot_do() {
    let (_directory, database) = scratch();

    let error = events(
        Command::HistoryAdd,
        &[("DB_PATH", database.as_str()), ("SQL", "   ")],
    )
    .await
    .expect_err("a blank statement is not an execution to record");
    assert!(matches!(error, CliError::Usage(_)), "{error:?}");

    let error = events(
        Command::HistoryAdd,
        &[
            ("DB_PATH", database.as_str()),
            ("SQL", "SELECT 1"),
            ("OUTCOME", "maybe"),
        ],
    )
    .await
    .expect_err("an outcome is one of three words");
    assert!(error.message().contains("unknown OUTCOME"), "{error:?}");

    let error = events(Command::SavedQueries, &[("DB_PATH", database.as_str())])
        .await
        .expect_err("an action is required");
    assert!(
        error.message().contains("SAVED_ACTION is required"),
        "{error:?}"
    );

    let error = events(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "save"),
            ("SQL", "SELECT 1"),
        ],
    )
    .await
    .expect_err("a saved query needs a name");
    assert!(error.message().contains("NAME is required"), "{error:?}");

    let error = events(
        Command::SavedQueries,
        &[
            ("DB_PATH", database.as_str()),
            ("SAVED_ACTION", "rename"),
            ("NAME", "whatever"),
        ],
    )
    .await
    .expect_err("renaming needs the row's own identity");
    assert!(
        error.message().contains("SAVED_ID is required"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_session_round_trips_the_tabs_it_was_handed() {
    let (_directory, database) = scratch();
    let tabs = r#"[{"id":"a","sql":"SELECT 1"},{"id":"b","sql":"SELECT 2"}]"#;

    let saved = one(
        Command::Session,
        &[
            ("DB_PATH", database.as_str()),
            ("SESSION_ACTION", "save"),
            ("TABS_JSON", tabs),
            ("ACTIVE_TAB_ID", "b"),
        ],
    )
    .await;
    assert_eq!(saved["saved"], true);

    let loaded = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "load")],
    )
    .await;
    assert_eq!(loaded["saved"], true, "there is a session: {loaded}");
    // The blob comes back parsed, as an array, rather than as a string the caller has to parse
    // again. The command never looks inside it, so this is the app's own JSON unchanged.
    assert_eq!(loaded["tabs"], serde_json::from_str::<Json>(tabs).unwrap());
    assert_eq!(loaded["active_tab_id"], "b");

    // A second save replaces the first rather than adding a second session.
    one(
        Command::Session,
        &[
            ("DB_PATH", database.as_str()),
            ("SESSION_ACTION", "save"),
            ("TABS_JSON", r#"[{"id":"c","sql":"SELECT 3"}]"#),
        ],
    )
    .await;
    let replaced = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "load")],
    )
    .await;
    assert_eq!(replaced["tabs"].as_array().map(Vec::len), Some(1));
    assert_eq!(replaced["tabs"][0]["id"], "c");
    assert!(
        replaced.get("active_tab_id").is_none(),
        "a save with no front tab leaves the key out rather than sending an empty string: {replaced}"
    );
}

#[tokio::test]
async fn a_database_with_no_session_says_so_rather_than_failing() {
    // A fresh install. The app turns this into one blank tab; an error here would make every
    // first launch look like a failure.
    let (_directory, database) = scratch();
    let loaded = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "load")],
    )
    .await;
    assert_eq!(loaded["saved"], false);
    assert!(loaded.get("tabs").is_none(), "{loaded}");
}

#[tokio::test]
async fn clearing_a_session_reports_it_and_a_load_then_finds_nothing() {
    let (_directory, database) = scratch();
    one(
        Command::Session,
        &[
            ("DB_PATH", database.as_str()),
            ("SESSION_ACTION", "save"),
            ("TABS_JSON", r#"[{"id":"a"}]"#),
        ],
    )
    .await;

    let cleared = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "clear")],
    )
    .await;
    assert_eq!(cleared["cleared"], true);
    let again = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "clear")],
    )
    .await;
    assert_eq!(again["cleared"], false, "there is nothing left to forget");

    let loaded = one(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "load")],
    )
    .await;
    assert_eq!(loaded["saved"], false);
}

#[tokio::test]
async fn a_session_save_refuses_what_it_cannot_store() {
    let (_directory, database) = scratch();

    let error = events(
        Command::Session,
        &[
            ("DB_PATH", database.as_str()),
            ("SESSION_ACTION", "save"),
            ("TABS_JSON", "   "),
        ],
    )
    .await
    .expect_err("a blank session is not a session");
    assert!(
        error.message().contains("TABS_JSON is required"),
        "{error:?}"
    );

    // Not JSON at all. Refused here so a session that cannot be parsed is reported at quit
    // rather than discovered at the next launch.
    let error = events(
        Command::Session,
        &[
            ("DB_PATH", database.as_str()),
            ("SESSION_ACTION", "save"),
            ("TABS_JSON", "{not json"),
        ],
    )
    .await
    .expect_err("a blob that is not JSON cannot be stored");
    assert!(error.message().contains("must be JSON"), "{error:?}");

    let error = events(
        Command::Session,
        &[("DB_PATH", database.as_str()), ("SESSION_ACTION", "maybe")],
    )
    .await
    .expect_err("an action is one of three words");
    assert!(
        error.message().contains("unknown SESSION_ACTION"),
        "{error:?}"
    );
}
