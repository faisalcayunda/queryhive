//! The `queryhive-engine` CLI must not write the owner's execution log from a harness.
//!
//! The CLI installs the decision log in `main` (ADR-0026 §9), and with no `DB_PATH` that log is
//! `~/Library/Application Support/QueryHive/queryhive.sqlite3`, the file the app owns. The golden
//! harness (`tools/golden/live_cases.py`) once ran the CLI that way and appended rows to the
//! owner's real audit log. The CLI keeps its default, because a hand-run engine is a real audit
//! path; the callers that are not one name a throwaway `DB_PATH`, and these tests hold both ends.
//!
//! The refusal used here (`DELETE` under `SAFE_MODE=read_only`) is decided before any network
//! use, so no server is needed. `HOME` is a temp directory, so a stray write cannot reach the
//! real database even if this test is the thing that fails.

use std::path::{Path, PathBuf};
use std::process::Command;

use qh_storage::Storage;

const ENGINE: &str = env!("CARGO_BIN_EXE_queryhive-engine");

/// A `HOME` that holds nothing.
fn empty_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temp HOME")
}

/// Nothing under `home` was created: not the default database, not even its directory.
fn assert_untouched(home: &Path) {
    let library = home.join("Library");
    assert!(
        !library.exists(),
        "something created {}, the default database's directory",
        library.display()
    );
}

#[test]
fn the_cli_logs_to_db_path_when_named_and_leaves_the_default_database_alone() {
    let home = empty_home();
    let db: PathBuf = home.path().join("elsewhere.sqlite3");

    let output = Command::new(ENGINE)
        .arg("preview")
        .env_clear()
        .env("HOME", home.path())
        .env("DB_PATH", &db)
        .env("DB_KIND", "postgres")
        .env("SAFE_MODE", "read_only")
        .env("SQL", "DELETE FROM t")
        .env("RETRIES", "0")
        .output()
        .expect("run the engine");
    assert_eq!(output.status.code(), Some(1), "a refused write exits 1");

    assert_untouched(home.path());
    let storage = Storage::open(&db).expect("open the named log");
    let rows = storage.execution_log(10).expect("read the log");
    assert_eq!(
        rows.len(),
        1,
        "one decision, in the named database: {rows:?}"
    );
    assert_eq!(rows[0].decision, "refused");
}

/// The harness as it really runs: its own `run_case`, its own environment, a `HOME` that is
/// empty. If it ever stops naming a throwaway `DB_PATH`, the CLI creates the default database
/// under that `HOME` and this fails.
#[test]
fn the_golden_harness_never_writes_the_default_database() {
    let home = empty_home();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/golden");
    let script = r#"
import os, sys
sys.path.insert(0, sys.argv[1])
import live_cases
# After start-up, not in the spawn: a version-manager shim for python3 needs the real HOME.
os.environ["HOME"] = sys.argv[2]
case = live_cases.LiveCase(
    "isolation", "preview", "postgres",
    {"DB_KIND": "postgres", "SAFE_MODE": "read_only"}, "",
    sql="DELETE FROM t",
)
code, stdout, _ = live_cases.run_case(case)
assert code == 1 and "SAFE_MODE=read_only refuses" in stdout, (code, stdout)
"#;

    let output = Command::new("python3")
        .args(["-I", "-c", script])
        .arg(&golden)
        .arg(home.path())
        .env("QH_ENGINE", ENGINE)
        .output()
        .expect("run python3");
    assert!(
        output.status.success(),
        "the harness run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_untouched(home.path());
}
