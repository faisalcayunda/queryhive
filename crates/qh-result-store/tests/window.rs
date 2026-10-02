//! The `QHW1` window: byte-exact layout, the 256-UTF-16 truncation, and the
//! guarantee that a windowed cell reads the same as `to_text` for every
//! encoding, resident and spilled (blueprint §11, §21.1).

use std::sync::Arc;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{
    render_window, ColumnFormat, CowCell, Outcome, StoreConfig, StoreHandle, StoreRegistry,
    WindowSpec, CELL_EMPTY, CELL_NULL, CELL_NUMERIC, CELL_TRUNCATED, FLAG_COMPLETE, FLAG_CUT,
    HEADER_LEN, TRUNCATE_UTF16,
};

fn registry() -> Arc<StoreRegistry> {
    StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    })
}

/// A finished store over `rows`, one `ColumnMeta` per column.
fn store(rows: Vec<Vec<Value>>, metas: Vec<ColumnMeta>) -> (Arc<StoreRegistry>, StoreHandle) {
    let registry = registry();
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(metas).expect("begin");
    let width = rows.first().map_or(0, Vec::len);
    let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(rows.len()); width];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            columns[index].push(cell.clone());
        }
    }
    writer
        .push(&ColumnBatch::new(columns).expect("a rectangular batch"))
        .expect("push");
    writer
        .finish(Outcome::Complete { truncated: false })
        .expect("finish");
    (registry, handle)
}

/// A finished store whose columns are given directly.
fn store_columns(
    columns: Vec<Vec<Value>>,
    metas: Vec<ColumnMeta>,
) -> (Arc<StoreRegistry>, StoreHandle) {
    let registry = registry();
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(metas).expect("begin");
    writer
        .push(&ColumnBatch::new(columns).expect("a rectangular batch"))
        .expect("push");
    writer
        .finish(Outcome::Complete { truncated: false })
        .expect("finish");
    (registry, handle)
}

fn text_meta(names: &[&str]) -> Vec<ColumnMeta> {
    names
        .iter()
        .map(|name| ColumnMeta::new(*name, "text"))
        .collect()
}

#[test]
fn the_header_is_byte_exact() {
    let (_registry, handle) = store(
        vec![vec![Value::Text("a".into()), Value::Text("bb".into())]],
        text_meta(&["x", "y"]),
    );
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();

    let spec = WindowSpec {
        rows: vec![0],
        columns: vec![0, 1],
        formats: vec![ColumnFormat::Raw, ColumnFormat::Raw],
        global_flags: FLAG_CUT | FLAG_COMPLETE,
    };
    let mut scratch = vec![CowCell::default(); 2];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();

    assert_eq!(&window.data[0..4], b"QHW1");
    assert_eq!(u16::from_le_bytes([window.data[4], window.data[5]]), 1);
    assert_eq!(window.global_flags(), FLAG_CUT | FLAG_COMPLETE);
    assert_eq!(
        u32::from_le_bytes([
            window.data[8],
            window.data[9],
            window.data[10],
            window.data[11]
        ]),
        0
    );
    assert_eq!(
        u32::from_le_bytes([
            window.data[12],
            window.data[13],
            window.data[14],
            window.data[15]
        ]),
        1
    );
    assert_eq!(
        u32::from_le_bytes([
            window.data[16],
            window.data[17],
            window.data[18],
            window.data[19]
        ]),
        2
    );
    assert_eq!(window.row_count, 1);
    assert_eq!(window.column_count, 2);
    // S2 + H equals the whole buffer, and the offsets are monotone.
    let heap_len = window.heap_len() as usize;
    let r = window.row_count as usize;
    let c = window.column_count as usize;
    assert_eq!(
        window.total_len(),
        HEADER_LEN + 4 * r + 4 * (r * c + 1) + r * c + heap_len
    );
    assert_eq!(window.source_rows(), vec![0]);
    assert_eq!(window.cell_text(0, 0), "a");
    assert_eq!(window.cell_text(0, 1), "bb");
}

#[test]
fn source_rows_follow_view_order() {
    let rows: Vec<Vec<Value>> = (0..5)
        .map(|i| vec![Value::Text(format!("r{i}").into())])
        .collect();
    let (_registry, handle) = store(rows, text_meta(&["x"]));
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();

    // Ask for rows in a scrambled view order.
    let spec = WindowSpec {
        rows: vec![4, 0, 2],
        columns: vec![0],
        formats: vec![ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();
    assert_eq!(window.source_rows(), vec![4, 0, 2]);
    assert_eq!(window.cell_text(0, 0), "r4");
    assert_eq!(window.cell_text(1, 0), "r0");
    assert_eq!(window.cell_text(2, 0), "r2");
}

#[test]
fn a_null_cell_carries_the_null_flag() {
    let (_registry, handle) = store(
        vec![vec![Value::Null], vec![Value::Text("".into())]],
        text_meta(&["x"]),
    );
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();
    let spec = WindowSpec {
        rows: vec![0, 1],
        columns: vec![0],
        formats: vec![ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();
    assert_eq!(window.cell_flags(0, 0) & CELL_NULL, CELL_NULL);
    // The stored empty string is EMPTY, not NULL.
    assert_eq!(window.cell_flags(1, 0) & CELL_EMPTY, CELL_EMPTY);
    assert_eq!(window.cell_flags(1, 0) & CELL_NULL, 0);
}

#[test]
fn long_text_is_truncated_at_a_grapheme_boundary() {
    let long: String = "a".repeat(1000);
    let (_registry, handle) = store(vec![vec![Value::Text(long.into())]], text_meta(&["x"]));
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();
    let spec = WindowSpec {
        rows: vec![0],
        columns: vec![0],
        formats: vec![ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();
    assert_eq!(window.cell_flags(0, 0) & CELL_TRUNCATED, CELL_TRUNCATED);
    assert_eq!(
        window.cell_text(0, 0).encode_utf16().count(),
        TRUNCATE_UTF16
    );

    // The same cell in full mode keeps every character and no TRUNCATED bit.
    let full = render_window(&chunk, &spec, &mut scratch, true).unwrap();
    assert_eq!(full.cell_text(0, 0).len(), 1000);
    assert_eq!(full.cell_flags(0, 0) & CELL_TRUNCATED, 0);
}

#[test]
fn cjk_and_zwj_emoji_truncate_cleanly() {
    // BMP text: each character is one UTF-16 unit.
    let cjk: String = "中".repeat(400);
    // A family emoji: one grapheme cluster built from several code points
    // joined by ZWJ. A naive byte cut would split it.
    let family = "👨‍👩‍👧‍👦".repeat(60);
    let (_registry, handle) = store(
        vec![vec![Value::Text(cjk.into()), Value::Text(family.into())]],
        text_meta(&["cjk", "zwj"]),
    );
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();
    let spec = WindowSpec {
        rows: vec![0],
        columns: vec![0, 1],
        formats: vec![ColumnFormat::Raw, ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 2];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();

    let cjk_cell = window.cell_text(0, 0);
    assert!(cjk_cell.encode_utf16().count() <= TRUNCATE_UTF16);
    assert!(cjk_cell.chars().all(|c| c == '中'));

    // The truncated ZWJ string must not end mid-cluster: it must be a whole
    // number of family emoji, each 11 UTF-16 units (7 code points, 4 of them
    // surrogate pairs -> 7 + 4 = 11).
    let zwj_cell = window.cell_text(0, 1);
    let units = zwj_cell.encode_utf16().count();
    assert!(units <= TRUNCATE_UTF16);
    assert_eq!(units % 11, 0, "truncated mid-grapheme: {zwj_cell:?}");
}

#[test]
fn window_text_matches_to_text_for_every_encoding() {
    let values = vec![
        Value::Bool(true),
        Value::Int(-7),
        Value::UInt(u64::MAX),
        Value::Float(1.5),
        Value::Decimal {
            unscaled: -12_345,
            scale: 3,
        },
        Value::Date { days: 0 },
        Value::Time {
            micros: 3_600_000_000,
        },
        Value::Timestamp {
            micros: 1_769_835_600_000_000,
            offset_secs: Some(0),
        },
        Value::Timestamp {
            micros: 1_769_835_600_000_000,
            offset_secs: None,
        },
        Value::Text("plain".into()),
        Value::Json(r#"{"a":1}"#.into()),
        Value::Bytes(vec![0x00, 0xff, 0x10]),
        Value::Null,
    ];
    let metas: Vec<ColumnMeta> = (0..values.len())
        .map(|i| ColumnMeta::new(format!("c{i}"), "text"))
        .collect();
    // One row, one value per column: each value gets its own encoding.
    let columns: Vec<Vec<Value>> = values.iter().cloned().map(|v| vec![v]).collect();
    let (_registry, handle) = store_columns(columns, metas);
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();

    for (index, original) in values.iter().enumerate() {
        let spec = WindowSpec {
            rows: vec![0],
            columns: vec![index],
            formats: vec![ColumnFormat::Raw],
            global_flags: 0,
        };
        let mut scratch = vec![CowCell::default(); 1];
        let window = render_window(&chunk, &spec, &mut scratch, true).unwrap();
        let expected = qh_core::to_text(original).unwrap_or_default();
        assert_eq!(
            window.cell_text(0, 0),
            expected,
            "encoding {index} disagreed with to_text"
        );
    }
}

#[test]
fn numeric_cells_light_the_numeric_flag() {
    let (_registry, handle) = store(
        vec![
            vec![Value::Text("42".into())],
            vec![Value::Text("abc".into())],
        ],
        text_meta(&["x"]),
    );
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let chunk = shared.load_chunk(reference.index).unwrap();
    let spec = WindowSpec {
        rows: vec![0, 1],
        columns: vec![0],
        formats: vec![ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    let window = render_window(&chunk, &spec, &mut scratch, false).unwrap();
    assert_eq!(window.cell_flags(0, 0) & CELL_NUMERIC, CELL_NUMERIC);
    assert_eq!(window.cell_flags(1, 0) & CELL_NUMERIC, 0);
}

#[test]
fn a_spilled_chunk_renders_identically() {
    let temp = std::env::temp_dir().join(format!("qh-window-spill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let registry = StoreRegistry::for_test(StoreConfig {
        spill_dir: Some(temp.clone()),
        ..StoreConfig::default()
    });
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(text_meta(&["x"])).unwrap();
    let column: Vec<Value> = (0..64)
        .map(|i| Value::Text(format!("row-{i}").into()))
        .collect();
    writer
        .push(&ColumnBatch::new(vec![column]).unwrap())
        .unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();

    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().unwrap();
    let before = shared.load_chunk(reference.index).unwrap();
    let spec = WindowSpec {
        rows: vec![0, 63],
        columns: vec![0],
        formats: vec![ColumnFormat::Raw],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    let resident = render_window(&before, &spec, &mut scratch, false).unwrap();
    // Drop the reader's reference so eviction sees the chunk as unborrowed.
    drop(before);

    let moved = shared.force_spill_all_for_test(&registry);
    assert!(moved >= 1, "nothing spilled");
    let after = shared.load_chunk(reference.index).unwrap();
    let spilled = render_window(&after, &spec, &mut scratch, false).unwrap();

    assert_eq!(resident.data, spilled.data);
    let _ = std::fs::remove_dir_all(&temp);
}
