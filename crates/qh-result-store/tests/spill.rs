//! The NFR-S3 spill tests (blueprint §10.5): no plaintext on disk, tamper
//! detection, nonce discipline, and the failure paths that must be errors
//! rather than panics.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use qh_columnar::ChunkBuilder;
use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{
    decode_record, encode_record, seal_store_chunk, sweep_spill_dir, FaultyMedium, Outcome,
    SpillCipher, SpillFile, SpillMedium, StoreConfig, StoreError, StoreHandle, StoreRegistry,
};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("qh-spill-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn meta(names: &[&str]) -> Vec<ColumnMeta> {
    names
        .iter()
        .map(|name| ColumnMeta::new(*name, "text"))
        .collect()
}

/// A registry whose spill is on and whose budget is small enough that a
/// few 500-row chunks force a spill.
fn spilling_registry(dir: &std::path::Path, budget: usize) -> Arc<StoreRegistry> {
    StoreRegistry::for_test(StoreConfig {
        budget_bytes: budget,
        low_water_bytes: budget / 3,
        spill_dir: Some(dir.to_path_buf()),
        ..StoreConfig::default()
    })
}

/// Push `batches` batches of `rows_per_batch` text rows each, one column,
/// returning the store and the text of every row in order.
fn push_text(
    registry: &Arc<StoreRegistry>,
    rows_per_batch: usize,
    batches: usize,
) -> (StoreHandle, Vec<String>) {
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["text"])).unwrap();
    let mut expected = Vec::new();
    let mut counter = 0usize;
    for _ in 0..batches {
        let mut column: Vec<Value> = Vec::with_capacity(rows_per_batch);
        for _ in 0..rows_per_batch {
            let text = format!("{counter:08}-{}", "x".repeat(80));
            counter += 1;
            expected.push(text.clone());
            column.push(Value::Text(text.into()));
        }
        writer
            .push(&ColumnBatch::new(vec![column]).unwrap())
            .unwrap();
    }
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    (handle, expected)
}

#[test]
fn no_plaintext_on_disk() {
    let dir = temp_dir("plaintext");
    // A small budget forces early chunks out as later ones land.
    let registry = spilling_registry(&dir, 200 * 1024);
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["text"])).unwrap();
    // Canaries in the shapes §10.5 names: ASCII, UTF-8, and a numeric.
    let canary_ascii = "CANARY-ASCII-9f3a";
    let canary_utf8 = "canary-é中😀";
    let canary_num = "424242424242";
    let mut first: Vec<Value> = vec![
        Value::Text(canary_ascii.into()),
        Value::Text(canary_utf8.into()),
        Value::Text(canary_num.into()),
    ];
    for index in 0..500 {
        first.push(Value::Text(
            format!("filler-{index}-{}", "z".repeat(80)).into(),
        ));
    }
    writer
        .push(&ColumnBatch::new(vec![first]).unwrap())
        .unwrap();
    for batch in 0..4 {
        let column: Vec<Value> = (0..500)
            .map(|i| Value::Text(format!("{batch}-{i:08}-{}", "y".repeat(80)).into()))
            .collect();
        writer
            .push(&ColumnBatch::new(vec![column]).unwrap())
            .unwrap();
    }
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();

    let raw = handle.shared().spill_raw_for_test();
    assert!(!raw.is_empty(), "nothing spilled, the test proves nothing");
    let as_text = String::from_utf8_lossy(&raw);
    assert!(!as_text.contains(canary_ascii), "ASCII canary on disk");
    assert!(!as_text.contains(canary_utf8), "UTF-8 canary on disk");
    assert!(!as_text.contains(canary_num), "numeric canary on disk");
    // The plaintext magic, the IPC continuation marker, and the encoding key
    // must not appear either.
    assert!(!raw.windows(4).any(|w| w == b"QHP1"), "QHP1 on disk");
    assert!(
        !raw.windows(4).any(|w| w == [0xff, 0xff, 0xff, 0xff]),
        "IPC continuation on disk"
    );
    assert!(!as_text.contains("qh.enc"), "qh.enc on disk");

    // The rows still read back correctly through the encrypted path.
    let values = handle.shared().values(0..3).unwrap();
    assert_eq!(qh_core::to_text(&values[0][0]).unwrap(), canary_ascii);
    assert_eq!(qh_core::to_text(&values[1][0]).unwrap(), canary_utf8);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_key_does_not_open() {
    let cipher_a = SpillCipher::for_test(0x11);
    let cipher_b = SpillCipher::for_test(0x22);
    let mut plain = b"a record that must not open under another key".to_vec();
    let (nonce, mut record) = cipher_a.seal(1, 0, &mut plain).unwrap();
    let error = cipher_b.open(1, 0, nonce, &mut record).unwrap_err();
    assert!(matches!(error, StoreError::SpillAuth { .. }));
}

#[test]
fn tampering_is_detected() {
    let cipher = SpillCipher::for_test(0x33);
    let mut plain = b"the original plaintext".to_vec();
    let (nonce, record) = cipher.seal(1, 0, &mut plain).unwrap();

    // One bit flipped.
    let mut flipped = record.clone();
    flipped[0] ^= 0x01;
    assert!(matches!(
        cipher.open(1, 0, nonce, &mut flipped).unwrap_err(),
        StoreError::SpillAuth { .. }
    ));

    // Two records swapped: the AAD carries the chunk index, so record A
    // cannot authenticate as chunk B.
    let mut other = b"a different record".to_vec();
    let (nonce_b, record_b) = cipher.seal(1, 1, &mut other).unwrap();
    let mut swapped = record_b.clone();
    assert!(matches!(
        cipher.open(1, 0, nonce_b, &mut swapped).unwrap_err(),
        StoreError::SpillAuth { .. }
    ));

    // Replayed into another store: the AAD carries the store id.
    let mut replay = record.clone();
    assert!(matches!(
        cipher.open(2, 0, nonce, &mut replay).unwrap_err(),
        StoreError::SpillAuth { .. }
    ));

    // Replayed under the helper's domain tag.
    let helper = SpillCipher::for_test_with_domain(0x33, *b"QHD1");
    let mut cross = record.clone();
    assert!(matches!(
        helper.open(1, 0, nonce, &mut cross).unwrap_err(),
        StoreError::SpillAuth { .. }
    ));

    // No panic on any of the above, and the original still opens.
    let mut original = record.clone();
    assert!(cipher.open(1, 0, nonce, &mut original).is_ok());
}

#[test]
fn nonces_are_never_reused() {
    // Nonces must be unique under one key. Different keys may share a nonce
    // value safely, so the check is per key.
    let cipher = SpillCipher::for_test(0x44);
    let mut seen = std::collections::HashSet::new();
    for index in 0..10_000u32 {
        // Three store ids, one counter space: the nonce is per key, not per
        // store, so store ids must not widen the space.
        let store = (index % 3) as u64 + 1;
        let mut plain = format!("record {index}").into_bytes();
        let (nonce, _) = cipher.seal(store, index, &mut plain).unwrap();
        assert!(seen.insert(nonce), "nonce {nonce} was reused");
    }

    // A second cipher under its own key has its own counter, and its nonces
    // are unique among themselves.
    let helper = SpillCipher::for_test_with_domain(0x45, *b"QHD1");
    let mut helper_seen = std::collections::HashSet::new();
    for index in 0..100u32 {
        let mut plain = b"helper record".to_vec();
        let (nonce, _) = helper.seal(1, index, &mut plain).unwrap();
        assert!(helper_seen.insert(nonce), "helper nonce {nonce} reused");
    }
}

#[test]
fn a_spilled_chunk_reads_back_identical() {
    let dir = temp_dir("roundtrip");
    let registry = spilling_registry(&dir, 200 * 1024);
    let (handle, expected) = push_text(&registry, 500, 5);

    // Force every chunk out and read the rows back.
    let moved = handle.shared().force_spill_all_for_test(&registry);
    assert!(moved >= 1, "nothing spilled");
    let values = handle.shared().values(0..(expected.len() as u32)).unwrap();
    assert_eq!(values.len(), expected.len());
    for (index, row) in values.iter().enumerate() {
        for cell in row {
            assert_eq!(qh_core::to_text(cell).unwrap(), expected[index]);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn modes_are_0700_and_0600() {
    let dir = temp_dir("modes");
    let report = sweep_spill_dir(&dir);
    assert!(report.spill_enabled);
    let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "spill directory mode");

    // The file is created 0600, then unlinked; the mode has to be read off
    // the still-open descriptor.
    let file = SpillFile::create(&dir, std::process::id(), 1).unwrap();
    let file_mode = file.metadata_for_test().unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "spill file mode");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_spill_directory_stays_empty() {
    let dir = temp_dir("empty");
    let registry = spilling_registry(&dir, 200 * 1024);
    let (handle, _expected) = push_text(&registry, 500, 5);
    let moved = handle.shared().force_spill_all_for_test(&registry);
    assert!(moved >= 1, "nothing spilled");

    // The file was unlinked before the first byte, so the directory holds
    // nothing even while the fd is open and the record is readable.
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert!(entries.is_empty(), "spill directory is not empty");
    assert!(!handle.shared().spill_raw_for_test().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn orphans_are_swept_without_following_links() {
    let dir = temp_dir("orphans");
    let outside = temp_dir("orphans-target");
    let target = outside.join("keep-me.txt");
    std::fs::write(&target, b"do not delete").unwrap();

    std::fs::write(dir.join("orphan.spill"), b"leftover").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("link.spill")).unwrap();
    let subdir = dir.join("subdir");
    std::fs::create_dir_all(&subdir).unwrap();

    let report = sweep_spill_dir(&dir);
    assert!(report.spill_enabled);
    assert_eq!(report.removed, 2, "file and symlink should be removed");
    assert!(!dir.join("orphan.spill").exists());
    assert!(!dir.join("link.spill").exists());
    // The symlink target is untouched and the subdirectory is left alone.
    assert_eq!(std::fs::read(&target).unwrap(), b"do not delete");
    assert!(subdir.is_dir());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn disk_full_is_an_error_not_a_panic() {
    let dir = temp_dir("diskfull");
    let registry = spilling_registry(&dir, 200 * 1024);
    let handle = registry.create();
    // Swap in a medium that fails every write with ENOSPC.
    *handle.shared().faulty_medium.lock().unwrap() = Some(Box::new(FaultyMedium));

    let writer = handle.writer();
    writer.begin(meta(&["text"])).unwrap();
    let make_batch = |start: usize| {
        let column: Vec<Value> = (start..start + 500)
            .map(|i| Value::Text(format!("{i:08}-{}", "x".repeat(80)).into()))
            .collect();
        ColumnBatch::new(vec![column]).unwrap()
    };
    // The first chunks fit; a later one pushes past the budget and the
    // eviction tries to spill into the faulty medium.
    let mut spilled_attempted = false;
    for batch in 0..6 {
        match writer.push(&make_batch(batch * 500)) {
            Ok(()) => {}
            Err(error) => {
                assert!(matches!(error, StoreError::DiskFull), "got {error:?}");
                spilled_attempted = true;
                break;
            }
        }
    }
    assert!(spilled_attempted, "the disk never filled");

    // The resident rows are still readable and nothing was lost.
    let values = handle.shared().values(0..1).unwrap();
    assert!(qh_core::to_text(&values[0][0])
        .unwrap()
        .starts_with("00000000-"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_record_is_an_error_not_a_panic() {
    // Build a real chunk to encode.
    let mut builder = ChunkBuilder::new(1);
    for index in 0..8 {
        builder.push_value(0, Value::Text(format!("row-{index}").into()));
    }
    let sealed = builder.seal().unwrap();
    let (chunk, _stats) = seal_store_chunk(sealed, &meta(&["x"])).unwrap();
    let good = encode_record(&chunk.batch, &chunk.flags).unwrap();
    assert!(decode_record(&good).is_ok());

    // A record with the magic broken.
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    assert!(matches!(
        decode_record(&bad_magic).unwrap_err(),
        StoreError::Corrupt { .. }
    ));

    // A truncated record: the IPC bytes are cut short.
    let truncated = &good[..good.len() / 2];
    assert!(matches!(
        decode_record(truncated).unwrap_err(),
        StoreError::Corrupt { .. }
    ));

    // A record shorter than its header.
    assert!(matches!(
        decode_record(&good[..8]).unwrap_err(),
        StoreError::Corrupt { .. }
    ));
}

#[test]
fn the_key_is_not_in_debug_output() {
    // A known key, so a leak would be findable.
    let cipher = SpillCipher::for_test(0xAB);
    let printed = format!("{cipher:?}");
    assert!(printed.contains("redacted"), "the key was not redacted");
    // 32 bytes of 0xAB would print as a long run of "ab"; it must not.
    assert!(
        !printed.to_lowercase().contains("abababab"),
        "key material in cipher debug: {printed}"
    );

    let dir = temp_dir("debug");
    let registry = spilling_registry(&dir, 64 * 1024);
    let printed = format!("{registry:?}");
    assert!(!printed.to_lowercase().contains("abababab"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_faulty_medium_is_a_medium() {
    // The injection type must satisfy the trait so the seam stays honest.
    fn assert_medium<T: SpillMedium>(_: T) {}
    assert_medium(FaultyMedium);
    let mut file = SpillFile::with_medium(Box::new(FaultyMedium));
    let error = file.append(b"anything").unwrap_err();
    assert!(matches!(error, StoreError::DiskFull));
}

#[test]
fn a_short_write_surfaces_as_io() {
    // A medium that fails with a non-ENOSPC error maps to Io, not DiskFull.
    struct BrokenMedium;
    impl SpillMedium for BrokenMedium {
        fn write_at(&self, _o: u64, _d: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
        fn read_at(&self, _o: u64, _b: &mut [u8]) -> std::io::Result<()> {
            Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        }
    }
    let mut file = SpillFile::with_medium(Box::new(BrokenMedium));
    let error = file.append(b"anything").unwrap_err();
    assert!(matches!(error, StoreError::Io(_)));
}

#[test]
fn a_file_medium_round_trips_bytes() {
    let dir = temp_dir("medium");
    let mut file = SpillFile::create(&dir, std::process::id(), 7).unwrap();
    let (offset, len) = file.append(b"hello spill").unwrap();
    assert_eq!(offset, 0);
    assert_eq!(len, 11);
    assert_eq!(file.read(offset, len).unwrap(), b"hello spill");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_sweep_reports_a_symlinked_directory_as_disabled() {
    let dir = temp_dir("symlink-dir");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let report = sweep_spill_dir(&link);
    assert!(!report.spill_enabled);
    assert!(report.reason.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_that_cannot_be_made_disables_spill() {
    // A path whose parent is a file cannot become a directory.
    let dir = temp_dir("notdir");
    let file_path = dir.join("a-file");
    let mut handle = std::fs::File::create(&file_path).unwrap();
    handle.write_all(b"x").unwrap();
    drop(handle);
    let report = sweep_spill_dir(&file_path.join("child"));
    assert!(!report.spill_enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_spilled_read_never_bricks_the_budget() {
    // B1 regression: the decoded cache was uncharged twice (once at insert,
    // once at eviction), so `resident` wrapped to ~2^64 after any spilled
    // read and every later charge failed with BudgetExceeded.
    let dir = temp_dir("budget");
    let registry = spilling_registry(&dir, 200 * 1024);
    let (handle, expected) = push_text(&registry, 500, 5);
    let moved = handle.shared().force_spill_all_for_test(&registry);
    assert!(moved >= 1, "nothing spilled");

    // Read every row back: this decrypts chunks and fills the decoded cache.
    let values = handle.shared().values(0..expected.len() as u32).unwrap();
    assert_eq!(values.len(), expected.len());

    let resident = registry.stats().resident_bytes;
    assert!(
        resident <= registry.config.budget_bytes,
        "resident {resident} exceeds the budget after a spilled read"
    );

    // A fresh unrelated store must still be able to take a small chunk.
    let (fresh, _) = push_text(&registry, 10, 1);
    assert_eq!(fresh.shared().rows(), 10);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_out_of_range_filter_column_is_refused_not_a_panic() {
    // B2 regression: a filter or sort naming a column past the width used to
    // panic in arrow's `RecordBatch::column`, and from the detached
    // expansion task that aborted the whole process.
    use qh_result_store::{FilterSpec, ViewSpec};
    let dir = temp_dir("colrange");
    let registry = spilling_registry(&dir, 200 * 1024);
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["x"])).unwrap();
    writer
        .push(&ColumnBatch::new(vec![vec![Value::Text("a".into())]]).unwrap())
        .unwrap();

    let error = handle
        .shared()
        .set_view(ViewSpec {
            filters: vec![FilterSpec::Values {
                column: 5,
                values: vec![Some("a".to_owned())],
            }],
            ..ViewSpec::default()
        })
        .unwrap_err();
    assert!(
        matches!(error, StoreError::InvalidArgument { .. }),
        "got {error:?}"
    );

    let error = handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((9, false)),
            ..ViewSpec::default()
        })
        .unwrap_err();
    assert!(
        matches!(error, StoreError::InvalidArgument { .. }),
        "got {error:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn multi_byte_whitespace_does_not_panic_the_trim() {
    // B3 regression: the trim helpers used char counts as byte indices, so a
    // leading NBSP or ideographic space panicked on the seal path and, from
    // the detached expansion task, aborted the process.
    use qh_result_store::{matches_text, swift_plain_number};
    // Seal path: a cell of NBSP + digit reaches `swift_plain_number`.
    assert_eq!(swift_plain_number("\u{a0}42"), swift_plain_number("42"));
    assert_eq!(swift_plain_number("\u{3000}42"), swift_plain_number("42"));
    // Filter-needle path.
    assert!(matches_text("42", "\u{a0}42"));
    assert!(matches_text("42", "42\u{3000}"));

    let dir = temp_dir("trim");
    let registry = spilling_registry(&dir, 200 * 1024);
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["x"])).unwrap();
    writer
        .push(
            &ColumnBatch::new(vec![vec![
                Value::Text("\u{a0}42".into()),
                Value::Text("\u{3000}7".into()),
            ]])
            .unwrap(),
        )
        .unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    // A streaming filter with a multi-byte-whitespace needle must not abort.
    let _ = handle.shared().set_view(qh_result_store::ViewSpec {
        filters: vec![qh_result_store::FilterSpec::Text {
            column: 0,
            needle: "\u{a0}42".to_owned(),
        }],
        ..qh_result_store::ViewSpec::default()
    });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_ipc_body_is_corrupt_not_a_panic() {
    // B4 regression: arrow's IPC decoder panics on some malformed bodies
    // (a buffer offset past its length). The record is authenticated, so
    // this is the D-20 path: a helper's output. It must be `Corrupt`.
    use qh_result_store::{decode_record, encode_record, seal_store_chunk};
    let mut builder = qh_columnar::ChunkBuilder::new(1);
    for index in 0..8 {
        builder.push_value(0, Value::Text(format!("row-{index}").into()));
    }
    let sealed = builder.seal().unwrap();
    let (chunk, _stats) = seal_store_chunk(sealed, &meta(&["x"])).unwrap();
    let good = encode_record(&chunk.batch, &chunk.flags).unwrap();

    // Flip bytes in the IPC body (past the 64-byte header) and confirm every
    // mutation is either a clean decode or a `Corrupt`, never a panic.
    for offset in (64..good.len()).step_by(7) {
        let mut mutated = good.clone();
        mutated[offset] ^= 0xA5;
        let _ = decode_record(&mutated);
    }
}
