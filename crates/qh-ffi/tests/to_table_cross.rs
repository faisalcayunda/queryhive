//! `to_table` across two connections (W13-T20): the settings of the second one.
//!
//! A `DEST_` key of any kind, blank or not, says "write to another connection". If a
//! partial or empty set were read as "no destination" the command would run its
//! `CREATE TABLE AS` on the **source** connection, which may be production, under the
//! `TARGET_*` name. So the rule fails closed, and these tests hold it from the outside:
//! each one drives `qh_ffi::run`, the entry point the CLI and the host share, with an
//! engine that counts every connect it is asked for and fails it. A refusal that leaves
//! the count at zero and the event stream empty is a refusal that no statement, and no
//! connection, could have gone out ahead of.
//!
//! The transfer itself is not here yet. Until it lands, a complete and valid set is
//! refused as unavailable, which is the same guarantee: nothing reaches the source.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, RealEngine, Settings};

/// An engine that counts connects and always fails them. The real drivers answer
/// `driver()`, so the slot list and the default port are the ones production uses.
struct CountingEngine {
    inner: RealEngine,
    connects: AtomicUsize,
}

impl CountingEngine {
    fn new() -> Self {
        Self {
            inner: RealEngine::new(),
            connects: AtomicUsize::new(0),
        }
    }

    fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Engine for CountingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Err(EngineError::Connect {
            message: "the test engine never connects".to_owned(),
            kind: FailureKind::Permanent,
        })
    }
}

/// A `to_table` whose source is a PostgreSQL connection that says `SAFE_MODE=full`, with
/// `extra` added after it. The top-level `SAFE_MODE=full` is there on purpose: it must
/// never stand in for a missing `DEST_SAFE_MODE`.
fn settings(extra: &[(&str, &str)]) -> Settings {
    let mut pairs = vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "source.invalid"),
        ("DB_USER", "queryhive"),
        ("RETRIES", "0"),
        ("SAFE_MODE", "full"),
        ("SQL", "SELECT 1 AS id"),
        ("TARGET_SCHEMA", "public"),
        ("TARGET_TABLE", "people"),
    ];
    pairs.extend_from_slice(extra);
    Settings::from_pairs(pairs)
}

/// The kind and the Safe Mode a destination cannot do without.
const COMPLETE: [(&str, &str); 3] = [
    ("DEST_DB_KIND", "mysql"),
    ("DEST_DB_HOST", "target.invalid"),
    ("DEST_SAFE_MODE", "full"),
];

async fn to_table(settings: &Settings) -> (Result<(), CliError>, usize, usize) {
    let engine = CountingEngine::new();
    let mut out = Capture::new();
    let result = run(
        Command::ToTable,
        settings,
        &mut out,
        &engine,
        &CancelFlag::new(),
    )
    .await;
    (result, engine.connects(), out.lines.len())
}

/// Every refusal is a `Usage` error that names what was wrong, raised before a `step`
/// event and before any connect.
async fn refused(extra: &[(&str, &str)], mentions: &str) {
    let (result, connects, events) = to_table(&settings(extra)).await;
    match result {
        Err(CliError::Usage(message)) => assert!(
            message.contains(mentions),
            "`{message}` should name {mentions} ({extra:?})"
        ),
        other => panic!("expected a usage error for {extra:?}, got {other:?}"),
    }
    assert_eq!(connects, 0, "the source was reached for {extra:?}");
    assert_eq!(events, 0, "an event went out for {extra:?}");
}

#[tokio::test]
async fn without_any_dest_key_the_command_is_the_ctas_it_always_was() {
    // The control: the counting engine does see the connect of an ordinary `to_table`, so
    // the zeros below mean something.
    let (result, connects, _) = to_table(&settings(&[])).await;
    assert!(matches!(result, Err(CliError::Connect(_))), "{result:?}");
    assert_eq!(connects, 1);
}

#[tokio::test]
async fn a_bare_or_blank_dest_key_selects_the_transfer_and_is_refused_for_what_it_lacks() {
    // `DEST_` with nothing after it, blank values, and a lone secret: each one must be
    // read as "a destination was asked for", never as "none was".
    for extra in [
        &[("DEST_", "")][..],
        &[("DEST_DB_KIND", "")],
        &[("DEST_DB_URL", ""), ("DEST_SAFE_MODE", "full")],
        &[("DEST_DB_PASSWORD", "x")],
        &[("DEST_DB_HOST", "target.invalid")],
        &[("DEST_TRINO_URL", "trino://target.invalid")],
        &[("DEST_SAFE_MODE", "full")],
    ] {
        refused(extra, "DEST_DB_KIND").await;
    }
}

#[tokio::test]
async fn the_destination_safe_mode_is_mandatory_and_never_inherited() {
    for blank in ["", "   "] {
        refused(
            &[
                ("DEST_DB_KIND", "mysql"),
                ("DEST_DB_HOST", "target.invalid"),
                ("DEST_SAFE_MODE", blank),
            ],
            "DEST_SAFE_MODE",
        )
        .await;
    }
    // Absent altogether: the source's own `SAFE_MODE=full` (above) must not fill in.
    refused(
        &[
            ("DEST_DB_KIND", "mysql"),
            ("DEST_DB_HOST", "target.invalid"),
        ],
        "DEST_SAFE_MODE",
    )
    .await;
    // A URL alone satisfies the connection part, and not this one.
    refused(
        &[("DEST_DB_URL", "mysql://target.invalid/db")],
        "DEST_SAFE_MODE",
    )
    .await;
    // A spelling nobody offers is refused by name, not read as `full`.
    for misspelt in ["ful", "readonly", "none", "false"] {
        let mut extra = COMPLETE.to_vec();
        extra.retain(|(key, _)| *key != "DEST_SAFE_MODE");
        extra.push(("DEST_SAFE_MODE", misspelt));
        refused(&extra, "DEST_SAFE_MODE").await;
    }
}

#[tokio::test]
async fn append_is_not_offered_across_connections() {
    for spelling in ["append", " Append ", "APPEND"] {
        let mut extra = COMPLETE.to_vec();
        extra.push(("WRITE_MODE", spelling));
        refused(&extra, "append").await;
    }
    // A mode this engine cannot name is not the mode that was asked for, here as in the
    // single-connection command.
    for unknown in ["", "merge"] {
        let mut extra = COMPLETE.to_vec();
        extra.push(("WRITE_MODE", unknown));
        refused(&extra, "WRITE_MODE").await;
    }
}

#[tokio::test]
async fn a_complete_set_is_not_a_ctas_on_the_source_either() {
    // Until the transfer exists the answer is "not available", and the source is still
    // untouched. A `DEST_` set must never degrade into the one-connection command.
    for mode in [None, Some("create"), Some("replace")] {
        let mut extra = COMPLETE.to_vec();
        if let Some(mode) = mode {
            extra.push(("WRITE_MODE", mode));
        }
        refused(&extra, "not available").await;
    }
    // A URL in place of the kind is the other accepted shape.
    refused(
        &[
            ("DEST_DB_URL", "mysql://target.invalid/db"),
            ("DEST_SAFE_MODE", "confirm"),
        ],
        "not available",
    )
    .await;
}
