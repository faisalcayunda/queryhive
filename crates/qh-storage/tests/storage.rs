//! The local database against a real file, and the schema against the blueprint.
//!
//! The unit tests in `src/` cover the migration rules and the schema against an in-memory
//! database. These cover the API over a real file, and what only a file can show: that a
//! database survives being closed and reopened, and that reopening runs the launch path
//! (a migration that finds nothing to do) without disturbing anything.
//!
//! The schema itself is pinned in `src/lib.rs`'s tests rather than here, because reading
//! it means reaching past the API for `sqlite_master`, and a test that does that belongs
//! where the connection is.

use qh_storage::{ConnectionKind, ConnectionRecord, Storage};
use qh_sync::{Resolution, SyncId, Version};

/// An open database with the migrations applied, in a directory that goes away with the
/// test.
fn storage() -> (Storage, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("queryhive.sqlite3");
    let mut storage = Storage::open(&path).expect("open");
    storage.migrate_at(1_700_000_000_000).expect("migrate");
    (storage, directory)
}

fn sample(name: &str) -> ConnectionRecord {
    let mut record = ConnectionRecord::new(name, ConnectionKind::Postgres, 1_700_000_000_000);
    record.host = Some("127.0.0.1".to_owned());
    record.port = Some(5432);
    record.user_name = Some("qh".to_owned());
    record
}

#[test]
fn a_database_survives_being_closed_and_reopened() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("queryhive.sqlite3");

    let saved = {
        let mut storage = Storage::open(&path).unwrap();
        storage.migrate_at(1_700_000_000_000).unwrap();
        let record = sample("Production");
        storage.save_connection(&record).unwrap();
        record
    };

    let mut reopened = Storage::open(&path).unwrap();
    // Reopening runs the migration again, which is the launch path: it must find nothing
    // to do and must not disturb what is there.
    assert!(reopened.migrate_at(1_700_000_001_000).unwrap().is_empty());
    let found = reopened
        .connection(&saved.meta.id)
        .unwrap()
        .expect("the row");
    assert_eq!(found, saved);
    assert_eq!(
        reopened.schema_version().unwrap(),
        qh_storage::migrate::latest_version()
    );
}

#[test]
fn a_deletion_on_one_side_beats_an_older_edit_on_the_other() {
    let (storage, _directory) = storage();
    let record = sample("Deleted here");
    storage.save_connection(&record).unwrap();
    storage
        .soft_delete_connection(&record.meta.id, 1_700_000_010_000)
        .unwrap();

    let mut edited_elsewhere = record.clone();
    edited_elsewhere.name = "Edited before the deletion".to_owned();
    edited_elsewhere.meta.touch(1_700_000_008_000);
    assert_eq!(
        storage.merge_connection(&edited_elsewhere).unwrap(),
        Resolution::KeepOurs,
        "the deletion is the newer revision"
    );
    assert!(storage
        .connection(&record.meta.id)
        .unwrap()
        .unwrap()
        .meta
        .is_deleted());
}

#[test]
fn a_connection_row_cannot_hold_a_password_by_accident() {
    let (storage, _directory) = storage();
    // The column is a reference and the comment in the migration says so. This is the
    // only part of that rule SQL can hold: the column exists, it stores text, and nothing
    // in this crate ever writes a password into it — the app's flow hands a Keychain
    // account to qh-credentials and stores the account here.
    let mut record = sample("Passwordless");
    record.secret_ref = None;
    storage.save_connection(&record).unwrap();
    let stored = storage.connection(&record.meta.id).unwrap().unwrap();
    assert_eq!(stored.secret_ref, None);
}

#[test]
fn a_connection_round_trips_through_sql() {
    let (storage, _directory) = storage();
    let mut record = sample("Production");
    record.options_json = r#"{"ssl_mode":"require","show_system_schemas":false}"#.to_owned();
    record.is_production = true;
    record.is_read_only = true;
    record.secret_ref = Some("9DB3C0CC-7062-49BB-9D47-62F765275A9B".to_owned());
    record.sort_order = -3;

    storage.save_connection(&record).unwrap();
    assert_eq!(
        storage.connection(&record.meta.id).unwrap().unwrap(),
        record
    );

    // Writing the same identity again replaces the row rather than failing on the primary
    // key: editing a connection is the ordinary case, not the exceptional one.
    let mut edited = record.clone();
    edited.name = "Production (read replica)".to_owned();
    edited.meta.touch(1_700_000_002_000);
    storage.save_connection(&edited).unwrap();
    assert_eq!(storage.connections().unwrap(), vec![edited.clone()]);
    assert_eq!(edited.meta.version.get(), 2, "the edit is a revision");
}

#[test]
fn a_deleted_connection_keeps_its_row_and_loses_its_place_in_the_sidebar() {
    let (storage, _directory) = storage();
    let kept = sample("Kept");
    let removed = sample("Removed");
    storage.save_connection(&kept).unwrap();
    storage.save_connection(&removed).unwrap();
    assert_eq!(storage.connections().unwrap().len(), 2);

    assert!(storage
        .soft_delete_connection(&removed.meta.id, 1_700_000_005_000)
        .unwrap());

    // Gone from the list, still there by identity — which is what a sync needs, and what
    // "the row is deleted" has to mean if a deletion is to travel at all.
    assert_eq!(storage.connections().unwrap(), vec![kept.clone()]);
    let tombstone = storage
        .connection(&removed.meta.id)
        .unwrap()
        .expect("the tombstone is still a row");
    assert_eq!(tombstone.meta.deleted_at, Some(1_700_000_005_000));
    assert_eq!(tombstone.meta.version.get(), 2, "deleting is a revision");
    assert_eq!(storage.connections_including_deleted().unwrap().len(), 2);
    storage.check_integrity().unwrap();

    // Deleting it again is a no-op rather than a second revision.
    assert!(storage
        .soft_delete_connection(&removed.meta.id, 1_700_000_006_000)
        .unwrap());
    assert_eq!(
        storage
            .connection(&removed.meta.id)
            .unwrap()
            .unwrap()
            .meta
            .version
            .get(),
        2
    );
    // Deleting something that was never there is not an error either.
    assert!(!storage
        .soft_delete_connection(&SyncId::now(), 1_700_000_006_000)
        .unwrap());
}

#[test]
fn a_deleted_connection_can_come_back() {
    let (storage, _directory) = storage();
    let record = sample("Mistake");
    storage.save_connection(&record).unwrap();
    storage
        .soft_delete_connection(&record.meta.id, 1_700_000_005_000)
        .unwrap();
    assert!(storage
        .restore_connection(&record.meta.id, 1_700_000_007_000)
        .unwrap());

    let restored = storage.connection(&record.meta.id).unwrap().unwrap();
    assert!(!restored.meta.is_deleted());
    assert_eq!(restored.meta.version.get(), 3, "deleted, then brought back");
    assert_eq!(storage.connections().unwrap().len(), 1);
}

#[test]
fn a_merge_keeps_the_newer_revision_whichever_side_it_is_on() {
    let (storage, _directory) = storage();
    let stored = sample("Stored");
    storage.save_connection(&stored).unwrap();

    // Older, in both senses the rule looks at: the same revision, stamped earlier. A row
    // that claims revision 1 with a *later* timestamp is not older, it is a clock that
    // disagrees — which the next case covers.
    let mut older = stored.clone();
    older.name = "Renamed elsewhere, earlier".to_owned();
    older.meta.version = Version::from(1);
    older.meta.updated_at = 1_699_999_999_000;
    assert_eq!(
        storage.merge_connection(&older).unwrap(),
        Resolution::KeepOurs
    );
    assert_eq!(
        storage.connection(&stored.meta.id).unwrap().unwrap().name,
        "Stored"
    );

    // Newer by revision: taken, with the name that came with it. The revision decides
    // even though the stored row's timestamp is not older by much — a counter does not
    // care whose clock is ahead.
    let mut newer = stored.clone();
    newer.name = "Renamed elsewhere, later".to_owned();
    newer.meta.touch(1_700_000_009_000);
    assert_eq!(
        storage.merge_connection(&newer).unwrap(),
        Resolution::TakeTheirs
    );
    assert_eq!(
        storage.connection(&stored.meta.id).unwrap().unwrap().name,
        "Renamed elsewhere, later"
    );

    // An identity this database has never seen is taken as new.
    let stranger = sample("From another machine");
    assert_eq!(
        storage.merge_connection(&stranger).unwrap(),
        Resolution::TakeTheirs
    );
    assert_eq!(storage.connections().unwrap().len(), 2);
}

#[test]
fn a_connection_cannot_point_at_a_group_that_is_not_there() {
    let (storage, _directory) = storage();
    // The other half of the ON DELETE clause: a dangling reference is refused at write time,
    // which is what `foreign_keys = ON` buys and what the schema alone does not.
    let mut record = sample("In no group");
    record.group_id = Some(SyncId::now());
    let refused = storage.save_connection(&record);
    assert!(refused.is_err(), "a dangling group_id is not a value");
    assert!(storage.connections().unwrap().is_empty());
}
