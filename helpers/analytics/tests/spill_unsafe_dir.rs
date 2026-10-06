//! A spill directory that is not safe means no spill, and never the OS temp directory.
//!
//! Its own test binary because it points `TMPDIR` at an empty directory for the whole
//! process, which would race any other test that makes a temp directory.

mod common;

use common::{big_table, collect};
use qh_analytics::session::{Engine, EngineConfig};
use qh_analytics_proto::ErrorKind;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsafe_directory_disables_spill_and_nothing_lands_in_tmpdir() {
    let base = tempfile::tempdir().unwrap();
    let elsewhere = base.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let fake_tmp = base.path().join("tmp");
    std::fs::create_dir(&fake_tmp).unwrap();
    // The spill "directory" is a symlink: the sweep refuses it (section 9.7).
    let link = base.path().join("spill");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    std::env::set_var("TMPDIR", &fake_tmp);
    assert_eq!(
        std::env::temp_dir(),
        fake_tmp,
        "the isolation must hold for this test to mean anything"
    );

    let mut config = EngineConfig::new(Some(link));
    config.target_partitions = 2;
    let engine = Engine::new(config).unwrap();
    assert!(
        engine
            .spill_disabled_reason()
            .is_some_and(|reason| reason.contains("symlink")),
        "{:?}",
        engine.spill_disabled_reason()
    );
    assert!(engine.spill().is_none());

    let session = engine.session();
    session
        .register("t", big_table(500_000, "unsafe"), Some(500_000))
        .unwrap();
    // A lease the sort reservations fit in (2 partitions x 2 MiB) but the data does not:
    // the sort must spill, it cannot, and it must say so in the user's words.
    let _lease = engine.grant(12 << 20);
    let error = collect(&session, "SELECT k, s FROM t ORDER BY s")
        .await
        .unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::TooLarge, "{error}");

    assert_eq!(
        std::fs::read_dir(&fake_tmp).unwrap().count(),
        0,
        "TMPDIR was written to"
    );
    assert_eq!(
        std::fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "the symlink target was written to"
    );
}

#[test]
fn no_directory_at_all_is_the_same_as_an_unsafe_one() {
    let engine = Engine::new(EngineConfig::new(None)).unwrap();
    assert!(engine.spill().is_none());
    assert!(engine.spill_disabled_reason().is_some());
}
