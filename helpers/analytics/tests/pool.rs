//! `LeasePool`: the lease is a cap, and everything comes back (blueprint section 14.15).

use std::sync::Arc;
use std::time::Duration;

use crate::common::{big_table, collect, engine, eventually};
use datafusion::execution::memory_pool::{MemoryConsumer, MemoryPool};
use futures::StreamExt;
use qh_analytics::pool::{LeasePool, OUT_OF_BUDGET};
use qh_analytics_proto::ErrorKind;

fn pool_of(bytes: usize) -> (Arc<LeasePool>, qh_analytics::pool::Lease) {
    let pool = LeasePool::new();
    let lease = pool.grant(bytes);
    (pool, lease)
}

#[test]
fn leases_raise_and_lower_the_limit() {
    let pool = LeasePool::new();
    assert_eq!(pool.limit(), 0);
    let first = pool.grant(100);
    let second = pool.grant(50);
    assert_eq!(pool.limit(), 150);
    drop(first);
    assert_eq!(pool.limit(), 50);
    drop(second);
    assert_eq!(pool.limit(), 0);
}

#[test]
fn nothing_can_be_reserved_without_a_lease() {
    let pool = LeasePool::new();
    let dyn_pool: Arc<dyn MemoryPool> = pool.clone();
    let reservation = MemoryConsumer::new("nobody").register(&dyn_pool);
    assert!(reservation.try_grow(1).is_err());
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn reservations_never_pass_the_lease() {
    let (pool, _lease) = pool_of(1_000);
    let dyn_pool: Arc<dyn MemoryPool> = pool.clone();
    let fixed = MemoryConsumer::new("hash table").register(&dyn_pool);
    let first = MemoryConsumer::new("sort a")
        .with_can_spill(true)
        .register(&dyn_pool);
    let second = MemoryConsumer::new("sort b")
        .with_can_spill(true)
        .register(&dyn_pool);

    // Hammer all three with sizes that do not divide the limit. After every attempt the
    // total is within the lease, and a refused attempt changed nothing.
    let sizes = [37usize, 101, 250, 3, 400, 64, 999, 17];
    for round in 0..200 {
        let before = pool.reserved();
        let size = sizes[round % sizes.len()];
        let result = match round % 3 {
            0 => fixed.try_grow(size),
            1 => first.try_grow(size),
            _ => second.try_grow(size),
        };
        assert!(pool.reserved() <= pool.limit(), "round {round}: {pool}");
        if result.is_err() {
            assert_eq!(pool.reserved(), before, "a refused grow must not reserve");
        }
        if round % 7 == 6 {
            let release = fixed.size() / 2;
            fixed.shrink(release);
        }
        if round % 11 == 10 {
            first.free();
        }
    }
    drop((fixed, first, second));
    assert_eq!(
        pool.reserved(),
        0,
        "everything is given back when the consumers drop"
    );
}

#[test]
fn spillers_share_what_is_left_in_equal_parts_and_leave_the_last_quarter() {
    let (pool, _lease) = pool_of(1_600);
    let dyn_pool: Arc<dyn MemoryPool> = pool.clone();
    let fixed = MemoryConsumer::new("hash table").register(&dyn_pool);
    fixed.try_grow(200).unwrap();
    let first = MemoryConsumer::new("sort a")
        .with_can_spill(true)
        .register(&dyn_pool);
    let second = MemoryConsumer::new("sort b")
        .with_can_spill(true)
        .register(&dyn_pool);

    // Spillers may use 1,200 (three quarters), less the 200 held by the unspillable one:
    // 1,000 for two, 500 each, and not a byte more for either.
    first.try_grow(500).unwrap();
    assert!(first.try_grow(1).is_err(), "a spiller stops at its share");
    second.try_grow(500).unwrap();
    assert!(second.try_grow(1).is_err());
    assert_eq!(pool.reserved(), 1_200);
    // The unspillable consumer still finds the last 400 free: that is what the reserve is
    // for (a merge buffer that is requested after the sorts have taken their shares).
    fixed.try_grow(400).unwrap();
    assert_eq!(pool.reserved(), 1_600);
    assert!(fixed.try_grow(1).is_err(), "and then the lease is full");
}

#[test]
fn a_late_spiller_cannot_push_the_total_past_the_lease() {
    // `FairSpillPool` would let `second` take its fair half although `first` already holds
    // 600, for a total of 1,100. The lease is a cap, so `second` gets what is left, and no
    // more than its share of the three quarters spillers may use.
    let (pool, _lease) = pool_of(1_000);
    let dyn_pool: Arc<dyn MemoryPool> = pool.clone();
    let first = MemoryConsumer::new("sort a")
        .with_can_spill(true)
        .register(&dyn_pool);
    first.try_grow(600).unwrap();
    let second = MemoryConsumer::new("sort b")
        .with_can_spill(true)
        .register(&dyn_pool);
    assert!(second.try_grow(500).is_err());
    assert!(
        second.try_grow(376).is_err(),
        "375 is half of the 750 spillers may share"
    );
    second.try_grow(375).unwrap();
    assert_eq!(pool.reserved(), 975);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_query_gives_everything_back() {
    let (engine, _dir) = engine(4);
    let session = engine.session();
    session
        .register("t", big_table(500_000, "pool"), Some(500_000))
        .unwrap();
    let lease = engine.grant(24 << 20);
    let rows = collect(&session, "SELECT k, s FROM t ORDER BY s DESC")
        .await
        .unwrap();
    assert_eq!(rows.iter().map(|b| b.num_rows()).sum::<usize>(), 500_000);
    assert!(
        engine.spill_stats().bytes > 0,
        "the sort was meant to spill"
    );
    assert!(
        eventually(5, || engine.reserved() == 0).await,
        "reserved is {} after the query ended",
        engine.reserved()
    );
    drop(lease);
    assert_eq!(engine.pool().limit(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_query_gives_everything_back() {
    let (engine, _dir) = engine(4);
    let session = engine.session();
    session
        .register("t", big_table(1_000_000, "cancel"), Some(1_000_000))
        .unwrap();
    let _lease = engine.grant(24 << 20);

    // Take the first batch of a sort that has a million rows to go, and drop the stream
    // there: the query is in the middle of its output, holding what a merge holds.
    let mut stream = session
        .query("SELECT k, s FROM t ORDER BY k")
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(60), stream.next())
        .await
        .expect("the first batch arrives")
        .expect("the sort has output")
        .expect("and it is not an error");
    assert!(first.num_rows() > 0);
    assert!(
        engine.reserved() > 0,
        "a query in the middle of its output holds memory"
    );
    drop(stream);
    assert!(
        eventually(5, || engine.reserved() == 0).await,
        "reserved is {} after the stream was dropped",
        engine.reserved()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_that_cannot_fit_is_too_large_not_a_crash() {
    let (engine, _dir) = engine(2);
    let session = engine.session();
    session
        .register("t", big_table(50_000, "tight"), Some(50_000))
        .unwrap();
    // 4 KiB cannot hold one batch of the scan itself.
    let _lease = engine.grant(4 << 10);
    let error = collect(&session, "SELECT k, s FROM t ORDER BY s")
        .await
        .unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::TooLarge, "{error}");
    assert_eq!(error.0.message, OUT_OF_BUDGET);
    assert!(eventually(5, || engine.reserved() == 0).await);
}

/// Prints which queries fit which leases (`cargo test ... lease_floor -- --ignored --nocapture`).
/// Slow, so it only runs on request; the table is what the sizes in `reserve_query` rest on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "an exploration: prints a table, asserts nothing"]
async fn lease_floor_report() {
    const ROWS: usize = 1_000_000;
    for partitions in [2usize, 4] {
        for lease_mib in [24usize, 32, 48, 64, 128] {
            for (name, sql) in [
                ("sort", "SELECT k, s FROM t ORDER BY k"),
                ("group", "SELECT s, count(*) FROM t GROUP BY s"),
                ("join", "SELECT count(*) FROM t a JOIN t b ON a.k = b.k"),
            ] {
                let (engine, _dir) = engine(partitions);
                let session = engine.session();
                session
                    .register("t", big_table(ROWS, "floor"), Some(ROWS as u64))
                    .unwrap();
                let lease = engine.grant(lease_mib << 20);
                let started = std::time::Instant::now();
                let result = collect(&session, sql).await;
                let outcome = match &result {
                    Ok(batches) => format!(
                        "ok ({} rows)",
                        batches.iter().map(|b| b.num_rows()).sum::<usize>()
                    ),
                    Err(error) => format!("{:?}", error.0.kind),
                };
                println!(
                    "partitions={partitions} lease={lease_mib:>3} MiB {name:<5} {outcome:<18} spilled {:>6.1} MiB in {:.1?}",
                    engine.spill_stats().bytes as f64 / (1 << 20) as f64,
                    started.elapsed()
                );
                drop(lease);
            }
        }
    }
}

/// How often a sort of a CSV file on the smallest lease fails, over repeats
/// (`QH_BATCH`, `QH_RESERVE`, `QH_PARTITIONS`, `QH_LEASE_MIB`, `QH_REPEATS`, `QH_ROWS`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "an exploration: prints a failure rate, asserts nothing"]
async fn lease_floor_csv_repeats() {
    let env = |name: &str, default: usize| -> usize {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let rows = env("QH_ROWS", 400_000);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.csv");
    {
        use std::io::Write;
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        writeln!(out, "id,name").unwrap();
        for id in (0..rows).rev() {
            writeln!(out, "{id},row-{id:08}-padding-to-make-the-rows-longer").unwrap();
        }
    }
    let mut failures = 0;
    let repeats = env("QH_REPEATS", 10);
    for _ in 0..repeats {
        let spill = dir.path().join("spill");
        let mut config = qh_analytics::session::EngineConfig::new(Some(spill));
        config.target_partitions = env("QH_PARTITIONS", 4);
        config.batch_size = env("QH_BATCH", 8192);
        config.spiller_reserve_divisor = env("QH_RESERVE", 4);
        config.merge_fan_in = env("QH_FANIN", 8);
        let engine = qh_analytics::session::Engine::new(config).unwrap();
        let session = engine.session();
        let format = qh_analytics_proto::FileFormat::Csv {
            has_header: true,
            delimiter: b',',
            quote: b'"',
        };
        session
            .register_file("big", path.to_str().unwrap(), &format)
            .await
            .unwrap();
        let _lease = engine.grant(env("QH_LEASE_MIB", 32) << 20);
        let stream = session
            .query("SELECT id, name FROM big ORDER BY name")
            .await
            .unwrap();
        if let Err(error) = futures::TryStreamExt::try_collect::<Vec<_>>(stream).await {
            failures += 1;
            println!("failed: {error}");
        }
    }
    println!("failures: {failures} of {repeats}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partitions_follow_the_lease() {
    let (engine, _dir) = engine(4);
    let session = engine.session();
    assert_eq!(session.partitions_now(), 1, "no lease, one partition");
    let small = engine.grant(8 << 20);
    assert_eq!(session.partitions_now(), 1);
    let more = engine.grant(24 << 20);
    assert_eq!(session.partitions_now(), 2, "32 MiB buys two");
    let most = engine.grant(96 << 20);
    assert_eq!(
        session.partitions_now(),
        4,
        "128 MiB buys the engine's four"
    );
    drop((small, more, most));
    assert_eq!(session.partitions_now(), 1);

    // And the statement really is planned for that many: EXPLAIN shows the scan's partitions.
    session.register("t", big_table(10, "x"), Some(10)).unwrap();
    let _lease = engine.grant(32 << 20);
    let plan = crate::common::texts(
        &collect(&session, "EXPLAIN SELECT k FROM t ORDER BY k")
            .await
            .unwrap(),
    );
    let physical = plan
        .iter()
        .find(|row| row[0].as_deref() == Some("physical_plan"))
        .unwrap();
    let text = physical[1].clone().unwrap();
    assert!(!text.contains("partitioning=RoundRobinBatch(4)"), "{text}");
}
