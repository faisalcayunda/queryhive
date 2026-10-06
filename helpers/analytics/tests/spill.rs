//! The helper's encrypted spill (blueprint section 14.8, NFR-S3).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use crate::common::{big_table, collect, engine, texts};
use datafusion::execution::SpillFile;
use futures::TryStreamExt;
use qh_analytics::session::classify;
use qh_analytics::spill::{EncryptedTempFiles, SpillSetup, RECORD_PLAINTEXT};
use qh_analytics_proto::ErrorKind;

fn factory(dir: &std::path::Path, quota: u64) -> std::sync::Arc<EncryptedTempFiles> {
    match EncryptedTempFiles::open(dir, quota) {
        SpillSetup::Enabled(files) => files,
        SpillSetup::Disabled { reason } => panic!("spill is off: {reason}"),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// UTF-16 little endian bytes of ASCII text.
fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sort_larger_than_its_lease_spills_correctly_and_leaves_no_plaintext() {
    const ROWS: usize = 2_000_000;
    const CANARY: &str = "QHCANARY-spill";
    let (engine, dir) = engine(4);
    let captured = engine.spill().unwrap().capture_raw_for_test();
    let session = engine.session();
    session
        .register("t", big_table(ROWS, CANARY), Some(ROWS as u64))
        .unwrap();
    let lease = engine.grant(32 << 20);

    // `k` runs from ROWS down to 1, so `ORDER BY k` reverses the table.
    let mut stream = session
        .query("SELECT k, s FROM t ORDER BY k")
        .await
        .unwrap();
    let mut expected = 1i64;
    let mut seen = 0usize;
    while let Some(batch) = stream.try_next().await.unwrap() {
        let keys = batch
            .column(0)
            .as_any()
            .downcast_ref::<datafusion::arrow::array::Int64Array>()
            .unwrap();
        for row in 0..batch.num_rows() {
            assert_eq!(keys.value(row), expected, "row {seen} is out of order");
            expected += 1;
            seen += 1;
        }
    }
    assert_eq!(seen, ROWS);
    drop(stream);
    drop(lease);

    let stats = engine.spill_stats();
    assert!(
        stats.files >= 1 && stats.bytes > 1 << 20,
        "the sort did not spill: {stats:?}"
    );
    // The directory was empty the whole time (files are unlinked on creation) and is now.
    let spill_dir = dir.path().join("spill");
    assert_eq!(std::fs::read_dir(&spill_dir).unwrap().count(), 0);
    assert!(crate::common::eventually(5, || engine.spill().unwrap().bytes_on_disk() == 0).await);

    // What reached the disk, byte for byte: no canary in any encoding, no IPC magic. The
    // needles are long on purpose: a four-byte pattern turns up by chance in tens of
    // megabytes of ciphertext (one in 2^32 per position), so the short markers are checked
    // on a small file in `short_markers_are_not_in_the_clear`.
    let files = captured.lock().unwrap();
    assert!(!files.is_empty(), "no spill file was captured");
    for (index, raw) in files.iter().enumerate() {
        assert!(!raw.is_empty());
        for needle in [
            CANARY.as_bytes().to_vec(),
            utf16(CANARY),
            b"ARROW1".to_vec(),
            b"qh.enc".to_vec(),
        ] {
            assert!(
                !contains(raw, &needle),
                "file {index} holds {needle:?} in the clear"
            );
        }
    }
}

#[tokio::test]
async fn a_record_that_was_damaged_is_an_error_not_wrong_data() {
    let dir = tempfile::tempdir().unwrap();
    let files = factory(&dir.path().join("spill"), 1 << 30);
    let file = files.create_file().unwrap();
    let payload: Vec<u8> = (0..(2 * RECORD_PLAINTEXT + 123))
        .map(|i| (i * 7 % 251) as u8)
        .collect();
    let mut writer = file.open_writer().unwrap();
    writer.write_all(&payload).unwrap();
    writer.finish().unwrap();
    assert_eq!(file.records_for_test(), 3, "1 MiB, 1 MiB, and the rest");

    // Intact: the plaintext comes back exactly.
    let read: Vec<u8> = file
        .read_stream()
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .concat();
    assert_eq!(read, payload);

    // Damaged: the first record fails, and no byte of it is handed out.
    file.corrupt_first_record_for_test();
    let mut stream = file.read_stream().unwrap();
    let error = futures::StreamExt::next(&mut stream)
        .await
        .unwrap()
        .unwrap_err();
    let wire = classify(&error);
    assert_eq!(wire.kind, ErrorKind::Spill, "{error}");
    assert!(wire.message.contains("authentication"), "{}", wire.message);
}

#[tokio::test]
async fn records_do_not_open_under_another_files_identity() {
    // The AAD binds a record to its file id and its position: a record copied into another
    // file does not authenticate there.
    let dir = tempfile::tempdir().unwrap();
    let files = factory(&dir.path().join("spill"), 1 << 30);
    let cipher = files.cipher_for_test();
    let mut plain = b"operator state".to_vec();
    let (nonce, record) = cipher.seal(1, 0, &mut plain).unwrap();
    for (file, index) in [(2u64, 0u32), (1, 1)] {
        let mut copy = record.clone();
        assert!(
            cipher.open(file, index, nonce, &mut copy).is_err(),
            "file {file} record {index}"
        );
    }
    let mut own = record;
    assert!(cipher.open(1, 0, nonce, &mut own).is_ok());
}

#[tokio::test]
async fn the_quota_stops_a_spill_that_would_outgrow_it() {
    let dir = tempfile::tempdir().unwrap();
    let files = factory(&dir.path().join("spill"), 3 * 1024 * 1024);
    let file = files.create_file().unwrap();
    let mut writer = file.open_writer().unwrap();
    // Four megabytes of plaintext against a three megabyte quota: the third record is
    // refused, and the error is a spill error, not a panic or a silent short file.
    let error = writer
        .write_all(&vec![7u8; 4 * RECORD_PLAINTEXT])
        .and_then(|()| writer.finish().map_err(std::io::Error::other))
        .unwrap_err();
    let inner = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<qh_analytics::spill::SpillError>());
    assert_eq!(
        inner,
        Some(&qh_analytics::spill::SpillError::Quota),
        "{error}"
    );
    assert!(files.bytes_on_disk() <= 3 * 1024 * 1024);
}

#[test]
fn files_are_private_and_never_visible_in_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let spill = dir.path().join("spill");
    let files = factory(&spill, 1 << 30);
    let mode = std::fs::metadata(&spill).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "the spill directory");
    let _file = files.create_file().unwrap();
    assert_eq!(
        std::fs::read_dir(&spill).unwrap().count(),
        0,
        "the file is unlinked"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_session_cannot_read_the_first_ones_tables() {
    // Not spill, but the same isolation promise: names are per session.
    let (engine, _dir) = engine(2);
    let first = engine.session();
    let second = engine.session();
    first.register("r", big_table(10, "a"), Some(10)).unwrap();
    second.register("r", big_table(3, "b"), Some(3)).unwrap();
    let _lease = engine.grant(1 << 20);
    let a = texts(&collect(&first, "SELECT count(*) FROM r").await.unwrap());
    let b = texts(&collect(&second, "SELECT count(*) FROM r").await.unwrap());
    assert_eq!(a, vec![vec![Some("10".to_owned())]]);
    assert_eq!(b, vec![vec![Some("3".to_owned())]]);
}

#[test]
fn short_markers_are_not_in_the_clear() {
    // The IPC continuation marker, the domain tag and the record magic, in a record small
    // enough that finding them by chance is out of the question.
    let dir = tempfile::tempdir().unwrap();
    let files = factory(&dir.path().join("spill"), 1 << 30);
    let file = files.create_file().unwrap();
    let mut plain = Vec::new();
    for _ in 0..64 {
        plain.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        plain.extend_from_slice(b"QHD1 QHP1 ARROW1 qh.enc ");
    }
    let mut writer = file.open_writer().unwrap();
    writer.write_all(&plain).unwrap();
    writer.finish().unwrap();
    let raw = file.raw_bytes_for_test();
    assert!(raw.len() > plain.len(), "ciphertext plus a tag");
    for needle in [&[0xffu8; 4][..], b"QHD1", b"QHP1", b"ARROW1", b"qh.enc"] {
        assert!(
            !contains(&raw, needle),
            "{needle:?} is on disk in the clear"
        );
    }
}

/// DataFusion's spill pool (used by `RepartitionExec`) opens the reader while the file is
/// still being written and flushes after every batch to say "readable now". A spill that
/// ended the stream early dropped those batches without an error.
#[tokio::test]
async fn the_spill_pool_reads_batches_while_the_file_is_still_being_written() {
    use datafusion::arrow::array::{ArrayRef, Int64Array};
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;
    use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
    use datafusion::execution::runtime_env::RuntimeEnvBuilder;
    use datafusion::physical_plan::metrics::{ExecutionPlanMetricsSet, SpillMetrics};
    use datafusion::physical_plan::spill::spill_pool::spsc_channel;
    use datafusion::physical_plan::SpillManager;

    let dir = tempfile::tempdir().unwrap();
    let files = factory(&dir.path().join("spill"), 1 << 30);
    let env = RuntimeEnvBuilder::new()
        .with_disk_manager_builder(
            DiskManagerBuilder::default().with_mode(DiskManagerMode::Custom(files.clone())),
        )
        .build_arc()
        .unwrap();
    let schema = std::sync::Arc::new(Schema::new(vec![Field::new("k", DataType::Int64, false)]));
    let metrics = SpillMetrics::new(&ExecutionPlanMetricsSet::new(), 0);
    let manager = std::sync::Arc::new(SpillManager::new(env, metrics, schema.clone()));
    let (writer, mut reader) = spsc_channel(1 << 30, manager);

    let batch = |from: i64| {
        let keys: ArrayRef = std::sync::Arc::new(Int64Array::from_iter_values(from..from + 1000));
        RecordBatch::try_new(schema.clone(), vec![keys]).unwrap()
    };
    async fn next(
        reader: &mut datafusion::execution::SendableRecordBatchStream,
    ) -> Result<datafusion::common::Result<Option<RecordBatch>>, tokio::time::error::Elapsed> {
        tokio::time::timeout(std::time::Duration::from_secs(10), reader.try_next()).await
    }
    // Push one batch, read it back before the next goes in, as a repartition does.
    for round in 0..5 {
        writer.push_batch(&batch(round * 1000)).unwrap();
        let got = next(&mut reader)
            .await
            .expect("the reader waited for ever")
            .unwrap()
            .expect("the reader ended early with a batch written");
        assert_eq!(got.num_rows(), 1000, "round {round}");
    }
    // Two in a row, then the writer ends: both come back, then the stream ends.
    writer.push_batch(&batch(5000)).unwrap();
    writer.push_batch(&batch(6000)).unwrap();
    drop(writer);
    let mut rows = 0;
    while let Some(got) = next(&mut reader).await.unwrap().unwrap() {
        rows += got.num_rows();
    }
    assert_eq!(rows, 2000);
}

/// End to end under a small lease, with a consumer that reads slowly so the operators
/// upstream hold their buffers: what spills must come back as the same rows, and the
/// query must say so if it cannot. (The repartition path itself is pinned by the pool test
/// above; this query's spill comes from the aggregate, and no SQL we found makes
/// `RepartitionExec` spill on demand.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slowly_read_aggregate_under_a_small_lease_returns_the_rows_of_a_roomy_run() {
    const ROWS: usize = 400_000;
    const SQL: &str = "SELECT s, count(*), sum(k) FROM t GROUP BY s";
    async fn slow_rows(session: &qh_analytics::session::Session) -> Vec<Vec<Option<String>>> {
        let mut stream = session.query(SQL).await.unwrap();
        let mut batches = Vec::new();
        while let Some(batch) = stream.try_next().await.unwrap() {
            batches.push(batch);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let mut rows = texts(&batches);
        rows.sort();
        rows
    }
    let (engine, _dir) = engine(4);
    let session = engine.session();
    session
        .register("t", big_table(ROWS, "QHCANARY-repart"), Some(ROWS as u64))
        .unwrap();
    let roomy = engine.grant(2 << 30);
    let expected = slow_rows(&session).await;
    drop(roomy);
    assert_eq!(expected.len(), ROWS);

    let before = engine.spill_stats().files;
    let tight = engine.grant(12 << 20);
    let got = slow_rows(&session).await;
    drop(tight);
    assert!(
        engine.spill_stats().files > before,
        "the query did not spill"
    );
    assert_eq!(got.len(), expected.len(), "rows went missing");
    assert_eq!(got, expected);
}
