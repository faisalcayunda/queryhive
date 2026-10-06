//! The registry's budget and lock handling, and the width stats a store keeps for its head.

use std::sync::Arc;

use qh_columnar::ChunkBuilder;
use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{Origin, Outcome, StoreConfig, StoreError, StoreHandle, StoreRegistry};

const MIB: usize = 1024 * 1024;

fn registry(config: StoreConfig) -> Arc<StoreRegistry> {
    StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..config
    })
}

#[test]
fn a_helper_lease_is_charged_and_given_back() {
    let registry = registry(StoreConfig {
        budget_bytes: 100 * MIB,
        low_water_bytes: 90 * MIB,
        ..StoreConfig::default()
    });
    let lease = registry.reserve_query(80 * MIB).unwrap();
    assert_eq!(lease.bytes(), 80 * MIB);
    assert_eq!(registry.query_reserved_bytes(), 80 * MIB);
    assert_eq!(registry.stats().resident_bytes, 80 * MIB);

    // 20 MiB are left and a helper gets nothing useful under 32.
    match registry.reserve_query(80 * MIB) {
        Err(StoreError::BudgetExceeded { needed, budget }) => {
            assert_eq!((needed, budget), (32 * MIB, 20 * MIB));
        }
        other => panic!(
            "expected BudgetExceeded, got {:?}",
            other.map(|l| l.bytes())
        ),
    }

    drop(lease);
    assert_eq!(registry.query_reserved_bytes(), 0);
    assert_eq!(registry.stats().resident_bytes, 0);
}

#[test]
fn a_poisoned_live_table_does_not_stop_create_stats_or_a_full_budget() {
    let registry = registry(StoreConfig {
        budget_bytes: MIB,
        low_water_bytes: MIB / 2,
        ..StoreConfig::default()
    });
    let before = registry.create();
    let poisoner = Arc::clone(&registry);
    let _ = std::thread::spawn(move || poisoner.poison_live_for_test()).join();

    // `create` inserts into the table, `stats` reads it, and a charge past the budget evicts
    // through it: each used to `expect` and so to panic.
    let after = registry.create();
    assert_eq!(registry.stats().stores, 2);
    assert!(matches!(
        registry.charge(2 * MIB, Origin::Writer),
        Err(StoreError::BudgetExceeded { .. })
    ));
    assert_eq!(
        registry.stats().resident_bytes,
        0,
        "a refused charge is given back"
    );
    drop((before, after));
    assert_eq!(registry.stats().stores, 0);
}

fn texts(first: usize, rows: usize, long_at: usize) -> Vec<Vec<Value>> {
    let word = |row: usize| match row {
        1 => Value::Null,
        row if row == long_at => Value::Text("x".repeat(50).into()),
        row if row >= 250 => Value::Text("x".repeat(90).into()),
        row => Value::Text("x".repeat(row % 7 + 1).into()),
    };
    vec![
        (first..first + rows).map(word).collect(),
        (first..first + rows)
            .map(|row| Value::Int(row as i64))
            .collect(),
    ]
}

fn store(registry: &Arc<StoreRegistry>) -> StoreHandle {
    let handle = registry.create();
    handle
        .writer()
        .begin(vec![
            ColumnMeta::new("t", "text"),
            ColumnMeta::new("n", "bigint"),
        ])
        .unwrap();
    handle
}

/// A chunk the way a driver that builds its own would hand it over.
fn sealed(columns: Vec<Vec<Value>>) -> qh_columnar::SealedChunk {
    let mut builder = ChunkBuilder::new(columns.len());
    for (index, column) in columns.into_iter().enumerate() {
        for value in column {
            builder.push_value(index, value);
        }
    }
    builder.seal().unwrap()
}

#[test]
fn a_store_fed_sealed_chunks_keeps_the_same_head_widths_as_one_fed_batches() {
    let registry = registry(StoreConfig::default());

    // 300 rows in two pieces; the 50-character cell at row 160 is among the first 200 rows and the
    // 90-character ones from row 250 on are not.
    let by_batch = store(&registry);
    let by_chunk = store(&registry);
    for (first, long_at) in [(0, usize::MAX), (150, 160)] {
        let columns = texts(first, 150, long_at);
        by_batch
            .writer()
            .push(&ColumnBatch::new(columns.clone()).unwrap())
            .unwrap();
        by_chunk.writer().push_chunk(sealed(columns)).unwrap();
    }
    let complete = || Outcome::Complete { truncated: false };
    by_batch.writer().finish(complete()).unwrap();
    by_chunk.writer().finish(complete()).unwrap();

    // The longest text is 50, the widest integer among rows 0 to 199 has three digits.
    assert_eq!(by_batch.shared().head_widths(), vec![50, 3]);
    assert_eq!(
        by_chunk.shared().head_widths(),
        by_batch.shared().head_widths()
    );
}

#[test]
fn a_result_remembers_that_a_row_limit_cut_it_short() {
    let registry = registry(StoreConfig::default());
    for truncated in [false, true] {
        let handle = store(&registry);
        handle
            .writer()
            .finish(Outcome::Complete { truncated })
            .unwrap();
        assert_eq!(handle.shared().is_truncated(), truncated);
    }
}
