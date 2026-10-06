//! A released store that spilled must give its spill fd back. The spill file
//! is unlinked before the first byte, so the fd is the only handle on it.
//!
//! This is the only test in its binary: the fd count is process-wide, and a
//! sibling test opening files in parallel would move it.

use std::sync::Arc;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{Outcome, StoreConfig, StoreHandle, StoreRegistry};

fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

fn spilled_store(registry: &Arc<StoreRegistry>) -> StoreHandle {
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(vec![ColumnMeta::new("text", "text")]).unwrap();
    for batch in 0..8 {
        let column: Vec<Value> = (0..500)
            .map(|row| Value::Text(format!("{batch}-{row}-{}", "x".repeat(80)).into()))
            .collect();
        writer
            .push(&ColumnBatch::new(vec![column]).unwrap())
            .unwrap();
    }
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    handle
}

#[test]
fn releasing_spilled_stores_returns_the_fd_count_to_baseline() {
    let dir = std::env::temp_dir().join(format!("qh-fdleak-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let budget = 100 * 1024;
    let registry = StoreRegistry::for_test(StoreConfig {
        budget_bytes: budget,
        low_water_bytes: budget / 3,
        spill_dir: Some(dir.clone()),
        ..StoreConfig::default()
    });
    let baseline = open_fds();
    let handles: Vec<StoreHandle> = (0..30).map(|_| spilled_store(&registry)).collect();
    assert!(registry.stats().spilled_bytes > 0, "the stores must spill");
    let peak = open_fds();
    assert!(peak > baseline, "spilled stores should hold fds ({peak})");
    drop(handles);
    assert_eq!(registry.stats().stores, 0);
    assert_eq!(registry.stats().spilled_bytes, 0);
    assert_eq!(open_fds(), baseline, "spill fds leaked after release");
    let _ = std::fs::remove_dir_all(&dir);
}
