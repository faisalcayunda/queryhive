//! Seal throughput, run by hand in release:
//! `cargo test --release -p qh-result-store --test seal_bench -- --ignored --nocapture`

use qh_columnar::ChunkBuilder;
use qh_core::{ColumnMeta, Value};
use qh_result_store::seal_store_chunk;

const ROWS: usize = 4096;
const WIDTH: usize = 8;

fn build() -> qh_columnar::SealedChunk {
    let mut builder = ChunkBuilder::new(WIDTH);
    for row in 0..ROWS {
        for column in 0..WIDTH {
            let value = match column % 4 {
                0 => Value::Text(format!("{}.{:02}", row * 7, row % 100).into()),
                1 => Value::Text(format!("customer name {row}").into()),
                2 if row % 10 == 0 => Value::Null,
                2 => Value::Text(format!("{}", row * 31).into()),
                _ => Value::Int(row as i64),
            };
            builder.push_value(column, value);
        }
    }
    builder.seal().unwrap()
}

#[test]
#[ignore]
fn seal_throughput() {
    let columns: Vec<ColumnMeta> = (0..WIDTH)
        .map(|i| ColumnMeta::new(format!("c{i}"), "text"))
        .collect();
    let mut best = f64::MAX;
    for _ in 0..8 {
        let sealed = build();
        let start = std::time::Instant::now();
        std::hint::black_box(seal_store_chunk(sealed, &columns).unwrap());
        best = best.min(start.elapsed().as_secs_f64());
    }
    println!(
        "seal: best {:.2} ms/chunk, {:.2} Mcells/s",
        best * 1e3,
        (ROWS * WIDTH) as f64 / best / 1e6
    );
}
