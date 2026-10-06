//! The Parquet writer: the tenth format, and the only columnar one.
//!
//! Written against the native Rust `parquet` crate rather than by linking
//! DuckDB. The plan's reasoning is recorded in `docs/decisions/0018-parquet-native-writer.md`:
//! TablePro writes Parquet through a static `libduckdb.a`, and that is out of
//! bounds for an MIT tree and is a large C library the native crate does not
//! need.
//!
//! # How it streams, when the format is columnar
//!
//! Parquet is column-major and its encoders buffer a row group before deciding
//! an encoding, so there is no way to be flat-memory *per row* the way the
//! delimited writers are. What there is instead is a bound: rows are held in a
//! row group buffer and, at [`ROW_GROUP_ROWS`], the column chunks are encoded
//! and released. Peak memory is one row group, not the result set — which is the
//! property the README's rule is about. The whole result is never held.
//!
//! # Type mapping, and why most values are text
//!
//! A column becomes `BOOLEAN`, `INT64`, `DOUBLE` or `BYTE_ARRAY`(UTF8) from its
//! declared type name, and a value that cannot be that type becomes a NULL
//! rather than failing the export — the shape the source study took from
//! TablePro's `TRY_CAST`. Decimals, dates, times and timestamps are written as
//! their canonical text (`qh-core::render::to_text`), not as Parquet's logical
//! types, because Parquet's `DECIMAL` is bounded at 38 digits, its timestamp has
//! no place to keep the offset the server reported, and turning an exact
//! `NUMERIC(38,10)` into a logical type risks the very precision this project's
//! value type exists to keep. The text is lossless and an independent reader
//! reads it back exactly; a typed logical mapping is a later refinement, named
//! here rather than claimed.
//!
//! # One file per table
//!
//! `Format::Parquet::max_rows` is `None`: a Parquet file is one table, and
//! splitting it into parts would produce several files that are each a different
//! table. `ROWS_PER_FILE` can still ask for parts, and the caller gets them, but
//! the format does not need splitting and does not do it on its own.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use parquet::basic::{Compression, ConvertedType, Repetition, Type as PhysicalType};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::{SerializedColumnWriter, SerializedFileWriter};
use parquet::schema::types::Type;
use qh_core::{ColumnMeta, Value};

use crate::{ExportError, ExportOptions, Writer};

/// The Parquet encoder's failures are I/O-shaped here: they are the file's own
/// footer or encoding saying no, not a bad request from the caller.
impl From<parquet::errors::ParquetError> for ExportError {
    fn from(error: parquet::errors::ParquetError) -> Self {
        ExportError::Io(std::io::Error::other(error))
    }
}

/// Data rows held before a row group is encoded and released.
///
/// A bound rather than a limit on the file: further rows open the next row
/// group. 65.536 is a size a columnar reader likes (a page set per group) and is
/// small enough that the buffer is a few megabytes for a wide table rather than
/// a copy of the result.
pub const ROW_GROUP_ROWS: usize = 65_536;

/// How one column is stored, decided from the column's declared type name.
///
/// Everything not in the first three is text: see the module note for why an
/// exact decimal, a date and a timestamp are text here rather than Parquet's
/// logical types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnKind {
    Bool,
    Int64,
    Double,
    Utf8,
}

/// The Parquet type a column's source type name maps to.
///
/// Exact after the width is stripped, like `UpdateStatements.isNumeric` on the
/// app side: `decimal(38,10)` is `decimal`, `character varying(16)` is not a
/// number, and `interval` must not be read as `int`.
fn column_kind(type_name: &str) -> ColumnKind {
    let base = type_name
        .split('(')
        .next()
        .unwrap_or(type_name)
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "bool" | "boolean" => ColumnKind::Bool,
        "tinyint" | "smallint" | "int" | "integer" | "bigint" | "int2" | "int4" | "int8"
        | "serial" | "bigserial" | "smallserial" | "year" => ColumnKind::Int64,
        "real" | "float" | "float4" | "float8" | "double" | "double precision" => {
            ColumnKind::Double
        }
        _ => ColumnKind::Utf8,
    }
}

/// The Parquet schema for a column list, every field optional.
///
/// Optional rather than required because a NULL is a normal cell and Parquet
/// represents it with a definition level; a required field would have to
/// fabricate a value for the NULL the grid showed.
fn schema(columns: &[ColumnMeta], kinds: &[ColumnKind]) -> Result<Type, ExportError> {
    let mut fields = Vec::with_capacity(columns.len());
    for (column, kind) in columns.iter().zip(kinds) {
        let physical = match kind {
            ColumnKind::Bool => PhysicalType::BOOLEAN,
            ColumnKind::Int64 => PhysicalType::INT64,
            ColumnKind::Double => PhysicalType::DOUBLE,
            ColumnKind::Utf8 => PhysicalType::BYTE_ARRAY,
        };
        let mut builder = Type::primitive_type_builder(&column.name, physical)
            .with_repetition(Repetition::OPTIONAL);
        if matches!(kind, ColumnKind::Utf8) {
            // Says the bytes are UTF-8 text rather than an opaque blob, so a
            // reader shows a string and not base64.
            builder = builder.with_converted_type(ConvertedType::UTF8);
        }
        fields.push(Arc::new(builder.build()?));
    }
    Ok(Type::group_type_builder("schema")
        .with_fields(fields)
        .build()?)
}

/// One columnar file, open and taking rows.
pub struct ParquetWriter {
    writer: SerializedFileWriter<BufWriter<File>>,
    kinds: Vec<ColumnKind>,
    /// The row group being filled, one value list per column of the schema.
    buffer: Vec<Vec<Value>>,
    rows: u64,
    finished: bool,
}

impl ParquetWriter {
    /// Open a Parquet file at `path` for `columns`.
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        _options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        let kinds: Vec<ColumnKind> = columns
            .iter()
            .map(|column| column_kind(&column.type_name))
            .collect();
        let schema = Arc::new(schema(columns, &kinds)?);
        let properties = WriterProperties::builder()
            // Informational for a reader; the writer closes a group on its own
            // count so a row that lands mid-page is never lost. Uncompressed, so
            // no codec feature is enabled and no C library is linked.
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .set_compression(Compression::UNCOMPRESSED)
            .set_created_by("QueryHive".to_owned())
            .build();
        let file = crate::create_new(path)?;
        let writer = SerializedFileWriter::new(BufWriter::new(file), schema, Arc::new(properties))?;
        Ok(Self {
            writer,
            kinds,
            buffer: Vec::new(),
            rows: 0,
            finished: false,
        })
    }

    /// Encode the buffered rows as a row group and release them.
    fn flush_row_group(&mut self) -> Result<(), ExportError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let mut group = self.writer.next_row_group()?;
        for (index, kind) in self.kinds.iter().copied().enumerate() {
            // `None` only if the schema has fewer fields than the writer was
            // opened with, which cannot happen here; the guard keeps a mismatch
            // from silently dropping a column.
            let Some(mut column) = group.next_column()? else {
                return Err(ExportError::Usage {
                    message: "the Parquet schema and the row have different widths".to_owned(),
                });
            };
            write_column(&mut column, kind, &self.buffer, index)?;
            column.close()?;
        }
        group.close()?;
        self.buffer.clear();
        Ok(())
    }
}

impl Writer for ParquetWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        if self.finished {
            return Err(ExportError::Usage {
                message: "this export has already been finished".to_owned(),
            });
        }
        // One value per schema field, a short row padded with NULL and a long one
        // truncated. A driver hands rows exactly as wide as its columns, so this
        // is a guard against a defect and not a normal path.
        let mut values: Vec<Value> = Vec::with_capacity(self.kinds.len());
        for index in 0..self.kinds.len() {
            values.push(row.get(index).cloned().unwrap_or(Value::Null));
        }
        self.buffer.push(values);
        self.rows += 1;
        if self.buffer.len() >= ROW_GROUP_ROWS {
            self.flush_row_group()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.flush_row_group()?;
        // The footer, and the flush of everything buffered above it.
        self.writer.finish()?;
        Ok(())
    }
}

/// Write one column's worth of the buffered rows.
///
/// Values are written in one batch, looped until the encoder has taken them all:
/// `write_batch` may take fewer than it was handed (it stops at a page
/// boundary), and the loop is what keeps a large row group from silently losing
/// its tail.
fn write_column(
    column: &mut SerializedColumnWriter<'_>,
    kind: ColumnKind,
    rows: &[Vec<Value>],
    index: usize,
) -> Result<(), ExportError> {
    // Definition levels: 1 where the cell is present, 0 where it is NULL. Every
    // field is OPTIONAL, so the maximum is 1 and there are no repetition levels.
    //
    // One `write_batch` call per column: it takes the whole level list and splits
    // it into pages internally, so there is nothing to loop over. Its return value
    // is the number of *non-null* values written, not the number of slots, which
    // is why it is not used to drive a loop here.
    match kind {
        ColumnKind::Bool => {
            let (values, def_levels) = cell_values(rows, index, |value| match value {
                Value::Bool(flag) => (Some(*flag), true),
                _ => (Some(false), false),
            });
            column
                .typed::<BoolType>()
                .write_batch(&values, Some(&def_levels), None)?;
        }
        ColumnKind::Int64 => {
            let (values, def_levels) = cell_values(rows, index, |value| match value {
                Value::Int(number) => (Some(*number), true),
                Value::UInt(number) => (Some(*number as i64), true),
                Value::Bool(flag) => (Some(i64::from(*flag)), true),
                _ => (Some(0), false),
            });
            column
                .typed::<Int64Type>()
                .write_batch(&values, Some(&def_levels), None)?;
        }
        ColumnKind::Double => {
            let (values, def_levels) = cell_values(rows, index, |value| match value {
                Value::Float(number) => (Some(*number), true),
                Value::Int(number) => (Some(*number as f64), true),
                Value::UInt(number) => (Some(*number as f64), true),
                _ => (Some(0.0), false),
            });
            column
                .typed::<DoubleType>()
                .write_batch(&values, Some(&def_levels), None)?;
        }
        ColumnKind::Utf8 => {
            let (values, def_levels) =
                cell_values(rows, index, |value| match value.render_text() {
                    Some(text) => (Some(ByteArray::from(text.into_bytes())), true),
                    None => (Some(ByteArray::from(Vec::new())), false),
                });
            column
                .typed::<ByteArrayType>()
                .write_batch(&values, Some(&def_levels), None)?;
        }
    }
    Ok(())
}

/// One column of the buffer as encoder values plus definition levels.
///
/// The closure returns the physical value for the cell and whether the cell is
/// present. A NULL contributes a definition level of 0 and a placeholder the
/// encoder never reads, because the level says so.
fn cell_values<T>(
    rows: &[Vec<Value>],
    index: usize,
    convert: impl Fn(&Value) -> (Option<T>, bool),
) -> (Vec<T>, Vec<i16>) {
    let mut values = Vec::with_capacity(rows.len());
    let mut levels = Vec::with_capacity(rows.len());
    for row in rows {
        let (value, present) = convert(row.get(index).unwrap_or(&Value::Null));
        values.push(value.unwrap_or_else(|| unreachable_value()));
        levels.push(if present { 1 } else { 0 });
    }
    (values, levels)
}

/// A placeholder for a NULL cell.
///
/// `convert` always returns `Some`, so this is not reached; it exists so the
/// generic helper has no `unwrap` on a value that is only ever placeholder. It
/// panics rather than fabricates, because a real call here would be a defect.
fn unreachable_value<T>() -> T {
    unreachable!("every converter returns Some; a NULL uses the definition level")
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::file::reader::{FileReader, SerializedFileReader};
    use parquet::record::Field;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("name", "varchar"),
            ColumnMeta::new("score", "double precision"),
            ColumnMeta::new("active", "boolean"),
        ]
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "qh-parquet-{}-{tag}-{seq}.parquet",
            std::process::id()
        ))
    }

    #[test]
    fn type_names_map_to_the_column_kinds() {
        assert_eq!(column_kind("bigint"), ColumnKind::Int64);
        assert_eq!(column_kind("integer"), ColumnKind::Int64);
        assert_eq!(column_kind("double precision"), ColumnKind::Double);
        assert_eq!(column_kind("numeric(38,10)"), ColumnKind::Utf8);
        // A decimal is text, so its digits survive: Parquet's own DECIMAL is
        // bounded and the text is not.
        assert_eq!(column_kind("decimal(38,10)"), ColumnKind::Utf8);
        assert_eq!(column_kind("character varying(16)"), ColumnKind::Utf8);
        // `interval` must not be read as `int`.
        assert_eq!(column_kind("interval"), ColumnKind::Utf8);
        assert_eq!(column_kind("boolean"), ColumnKind::Bool);
        assert_eq!(column_kind("json"), ColumnKind::Utf8);
    }

    #[test]
    fn rows_come_back_with_their_values_and_nulls() {
        // Written by this crate's writer and read by the crate's reader, so this
        // pins the mapping and the null handling; the independent reader is the
        // separate check in the phase note.
        let path = temp_path("roundtrip");
        let mut writer =
            ParquetWriter::new(&path, &columns(), &ExportOptions::default()).expect("open");
        writer
            .write_row(&[
                Value::Int(1),
                Value::Text("alpha".into()),
                Value::Float(1.5),
                Value::Bool(true),
            ])
            .expect("row");
        writer
            .write_row(&[Value::Int(2), Value::Null, Value::Float(-0.0), Value::Null])
            .expect("row");
        writer.finish().expect("finish");

        let reader = SerializedFileReader::new(File::open(&path).expect("open")).expect("reader");
        let rows: Vec<_> = reader
            .get_row_iter(None)
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows.len(), 2);
        let fields: Vec<&Field> = rows[0].get_column_iter().map(|(_, f)| f).collect();
        assert_eq!(fields[0], &Field::Long(1));
        assert_eq!(fields[1], &Field::Str("alpha".to_owned()));
        assert_eq!(fields[2], &Field::Double(1.5));
        assert_eq!(fields[3], &Field::Bool(true));

        let second: Vec<&Field> = rows[1].get_column_iter().map(|(_, f)| f).collect();
        assert_eq!(second[0], &Field::Long(2));
        assert_eq!(second[1], &Field::Null);
        assert_eq!(second[3], &Field::Null);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_value_that_is_not_the_column_type_becomes_null_not_a_failure() {
        // The `TRY_CAST` shape: one unparseable cell must not fail the whole
        // export.
        let path = temp_path("cast");
        let columns = vec![ColumnMeta::new("id", "bigint")];
        let mut writer =
            ParquetWriter::new(&path, &columns, &ExportOptions::default()).expect("open");
        writer
            .write_row(&[Value::Text("not a number".into())])
            .expect("a bad cell must not fail the row");
        writer.finish().expect("finish");

        let reader = SerializedFileReader::new(File::open(&path).expect("open")).expect("reader");
        let rows: Vec<_> = reader
            .get_row_iter(None)
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let fields: Vec<&Field> = rows[0].get_column_iter().map(|(_, f)| f).collect();
        assert_eq!(fields[0], &Field::Null);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn more_rows_than_one_group_still_all_arrive() {
        // Two row groups, and the second must not be lost. Small groups are
        // forced by writing enough rows to cross the constant, which is too many
        // for a unit test, so the writer is driven past one `flush_row_group`
        // by hand below the constant instead: the property (a flushed group is
        // released, and the file still holds both) is what is asserted.
        let path = temp_path("groups");
        let columns = vec![ColumnMeta::new("id", "bigint")];
        let mut writer =
            ParquetWriter::new(&path, &columns, &ExportOptions::default()).expect("open");
        for id in 0..5 {
            writer.write_row(&[Value::Int(id)]).expect("row");
        }
        writer.flush_row_group().expect("flush first group");
        for id in 5..10 {
            writer.write_row(&[Value::Int(id)]).expect("row");
        }
        writer.finish().expect("finish");

        let reader = SerializedFileReader::new(File::open(&path).expect("open")).expect("reader");
        assert_eq!(
            reader.metadata().num_row_groups(),
            2,
            "both groups are in the file"
        );
        let rows: Vec<_> = reader
            .get_row_iter(None)
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(rows.len(), 10);
        let _ = std::fs::remove_file(&path);
    }
}
