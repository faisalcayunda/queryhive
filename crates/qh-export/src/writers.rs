//! The six writers. One struct each, mirroring the removed `exporter/writers.py`.
//!
//! Each one opens its file on construction, writes a row at a time, and finishes
//! with a trailer where its format needs one. Nothing accumulates rows: the point of
//! a streaming writer is that a hundred million rows cost the same memory as ten.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use qh_core::{to_text, ColumnMeta, Value};
use serde_json::Value as Json;

use crate::{escape, Codec, ExportError, ExportOptions, Format, Writer};

// --------------------------------------------------------------------------- //
// text and csv
// --------------------------------------------------------------------------- //

/// `.txt` and `.csv`, which differ only in their defaults.
///
/// The dialect is Python's `csv.writer` with `QUOTE_MINIMAL`: a field is quoted when
/// it contains the delimiter, the quote character, or a line terminator, and a quote
/// inside a quoted field is doubled. Getting this wrong produces a file that opens
/// fine and has the wrong number of columns, so it is spelled out rather than
/// delegated to a guess.
///
/// The file is written in the code page [`ExportOptions::encoding`] names, which is
/// what Python's `open(path, "w", encoding=…, errors="replace")` did. The text is
/// built as a `String` and encoded on the way out, so quoting and the line terminator
/// happen in characters — a delimiter that a code page can hold cannot be introduced
/// by the encoding step, and a `\r\n` stays two bytes.
pub struct DelimitedWriter {
    out: BufWriter<File>,
    codec: Codec,
    delimiter: char,
    quotechar: char,
    lineterminator: String,
    null_text: String,
}

impl DelimitedWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
        csv: bool,
    ) -> Result<Self, ExportError> {
        // Resolved before the file exists: an unknown name must not leave an empty file
        // behind for the caller to wonder about.
        let codec = Codec::resolve("ENCODING", &options.encoding)?;
        let mut out = BufWriter::new(File::create(path)?);

        // Written before anything else so that Excel reads the file as UTF-8, which is
        // what the Python engine's `bom` option did — and only when the file really is
        // UTF-8, the way Python gated it on the encoding name.
        if options.bom && codec.is_utf8() {
            out.write_all("\u{feff}".as_bytes())?;
        }

        let mut writer = Self {
            out,
            codec,
            // The format's own default, which is not the same as an empty delimiter.
            delimiter: options.delimiter.unwrap_or(if csv { ',' } else { '\t' }),
            quotechar: options.quotechar,
            lineterminator: options.lineterminator.clone(),
            null_text: options.null_text.clone(),
        };

        if options.header {
            // The raw names, not the escaped ones: a delimiter inside a column name is
            // what quoting is for.
            let header: Vec<String> = columns
                .iter()
                .map(|column| column.name.to_string())
                .collect();
            writer.write_fields(&header)?;
        }
        Ok(writer)
    }

    fn write_fields(&mut self, fields: &[String]) -> Result<(), ExportError> {
        let text = self.render_fields(fields);
        self.out.write_all(&self.codec.encode(&text))?;
        Ok(())
    }

    fn render_fields(&self, fields: &[String]) -> String {
        let mut out = String::new();
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push(self.delimiter);
            }
            let needs_quotes = field.contains(self.delimiter)
                || field.contains(self.quotechar)
                || field
                    .chars()
                    .any(|character| self.lineterminator.contains(character));
            if needs_quotes {
                out.push(self.quotechar);
                for character in field.chars() {
                    if character == self.quotechar {
                        out.push(self.quotechar);
                    }
                    out.push(character);
                }
                out.push(self.quotechar);
            } else {
                out.push_str(field);
            }
        }
        out.push_str(&self.lineterminator);
        out
    }
}

impl Writer for DelimitedWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        let fields: Vec<String> = row
            .iter()
            .map(|value| to_text(value).unwrap_or_else(|| self.null_text.clone()))
            .collect();
        self.write_fields(&fields)
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.out.flush()?;
        Ok(())
    }
}

/// Format a chunk of rows for the parallel path.
///
/// The text is returned unencoded, because a `String` cannot hold a code page that is
/// not UTF-8: whoever writes these chunks has to encode them with the same
/// [`Codec`] the writer would have used, or `ENCODING` is lost on that path. Nothing
/// drives it today — [`crate::render_rows`] is public for the day the render workers
/// exist — so this is where the encoding step belongs when that caller arrives.
pub(crate) fn render_delimited(
    format: Format,
    options: &ExportOptions,
    rows: &[Vec<Value>],
) -> String {
    let delimiter = options
        .delimiter
        .unwrap_or(if format == Format::Csv { ',' } else { '\t' });
    let quotechar = options.quotechar;
    let lineterminator = &options.lineterminator;
    let null_text = &options.null_text;

    let mut out = String::new();
    for row in rows {
        let fields: Vec<String> = row
            .iter()
            .map(|value| to_text(value).unwrap_or_else(|| null_text.clone()))
            .collect();
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push(delimiter);
            }
            let needs_quotes = field.contains(delimiter)
                || field.contains(quotechar)
                || field
                    .chars()
                    .any(|character| lineterminator.contains(character));
            if needs_quotes {
                out.push(quotechar);
                for character in field.chars() {
                    if character == quotechar {
                        out.push(quotechar);
                    }
                    out.push(character);
                }
                out.push(quotechar);
            } else {
                out.push_str(field);
            }
        }
        out.push_str(lineterminator);
    }
    out
}

// --------------------------------------------------------------------------- //
// json
// --------------------------------------------------------------------------- //

/// A JSON array, or one object per line.
///
/// Not renderable in parallel, and not because of the indentation: the array form
/// needs to know whether it is writing the first row, because the separator comes
/// *before* everything but the first. A chunk rendered without that knowledge
/// produces a file with a stray comma.
pub struct JsonWriter {
    out: BufWriter<File>,
    names: Vec<String>,
    lines: bool,
    indent: usize,
    first: bool,
}

impl JsonWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        let mut out = BufWriter::new(File::create(path)?);
        let lines = options.jsonl;
        if !lines {
            out.write_all(b"[\n")?;
        }
        Ok(Self {
            out,
            names: columns
                .iter()
                .map(|column| column.name.to_string())
                .collect(),
            lines,
            indent: options.indent,
            first: true,
        })
    }
}

impl Writer for JsonWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        let object = row_object(&self.names, row);

        if self.lines {
            // Python's default separators are ", " and ": " even without indentation.
            self.out
                .write_all(format!("{}\n", compact(&object)).as_bytes())?;
            return Ok(());
        }

        if !self.first {
            self.out.write_all(b",\n")?;
        }
        self.first = false;

        // Python writes the object indented and then prefixes *every* line with two
        // spaces, which is why the closing brace lines up under the opening one
        // rather than under the keys.
        let text = pretty(&object, self.indent, 0);
        // `split_inclusive` yields the closing brace as its own final segment, so
        // every line gets its prefix and there is no special case for the last one.
        for line in text.split_inclusive('\n') {
            self.out.write_all(b"  ")?;
            self.out.write_all(line.as_bytes())?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        if !self.lines {
            // An empty export closes on the same line as it opened; a non-empty one
            // gets a newline first so the `]` sits on its own line.
            if self.first {
                self.out.write_all(b"]\n")?;
            } else {
                self.out.write_all(b"\n]\n")?;
            }
        }
        self.out.flush()?;
        Ok(())
    }
}

/// The object for one row: column name to JSON-native value.
fn row_object(names: &[String], row: &[Value]) -> Json {
    let mut object = serde_json::Map::new();
    for (name, value) in names.iter().zip(row.iter()) {
        object.insert(name.clone(), qh_core::to_json_value(value));
    }
    Json::Object(object)
}

/// Python's `json.dumps(obj, ensure_ascii=False, default=str)`.
///
/// Compact, but with `", "` and `": "` separators — Python's defaults even without
/// indentation, so `{"a": 1, "b": 2}` and not `{"a":1,"b":2}`.
pub(crate) fn compact(value: &Json) -> String {
    match value {
        Json::Object(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, entry)| format!("{}: {}", scalar_string(key), compact(entry)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        Json::Array(items) => {
            let rendered: Vec<String> = items.iter().map(compact).collect();
            format!("[{}]", rendered.join(", "))
        }
        scalar => scalar_string_raw(scalar),
    }
}

/// Python's `json.dumps(obj, indent=n)` layout.
fn pretty(value: &Json, indent: usize, level: usize) -> String {
    let pad = |level: usize| " ".repeat(indent * level);
    match value {
        Json::Object(entries) if entries.is_empty() => "{}".to_owned(),
        Json::Object(entries) => {
            let mut out = String::from("{\n");
            for (index, (key, entry)) in entries.iter().enumerate() {
                out.push_str(&pad(level + 1));
                out.push_str(&scalar_string(key));
                out.push_str(": ");
                out.push_str(&pretty(entry, indent, level + 1));
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(level));
            out.push('}');
            out
        }
        Json::Array(items) if items.is_empty() => "[]".to_owned(),
        Json::Array(items) => {
            let mut out = String::from("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&pad(level + 1));
                out.push_str(&pretty(item, indent, level + 1));
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(level));
            out.push(']');
            out
        }
        scalar => scalar_string_raw(scalar),
    }
}

/// A string, escaped as JSON with `ensure_ascii=False`.
fn scalar_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

/// A scalar as JSON. `serde_json` matches Python for null, booleans, numbers and
/// strings, and neither escapes `/` nor non-ASCII.
fn scalar_string_raw(value: &Json) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

// --------------------------------------------------------------------------- //
// xml
// --------------------------------------------------------------------------- //

pub struct XmlWriter {
    out: BufWriter<File>,
    tags: Vec<String>,
    root: String,
    record: String,
}

impl XmlWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        let mut out = BufWriter::new(File::create(path)?);
        let root = options.xml_root.clone();
        let record = options.xml_record.clone();
        out.write_all(
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{root}>\n").as_bytes(),
        )?;
        Ok(Self {
            out,
            tags: columns.iter().map(|column| xml_tag(&column.name)).collect(),
            root,
            record,
        })
    }
}

impl Writer for XmlWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        let text = render_xml_row(&self.tags, &self.record, row);
        self.out.write_all(text.as_bytes())?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.out
            .write_all(format!("</{}>\n", self.root).as_bytes())?;
        self.out.flush()?;
        Ok(())
    }
}

fn render_xml_row(tags: &[String], record: &str, row: &[Value]) -> String {
    let mut out = format!("  <{record}>\n");
    for (tag, value) in tags.iter().zip(row.iter()) {
        match to_text(value) {
            // A NULL is an empty element marked as nil, which is XML's own way of
            // saying "this was null" rather than "this was missing".
            None => out.push_str(&format!("    <{tag} xsi:nil=\"true\"/>\n")),
            Some(text) => {
                // Control characters are not representable in XML 1.0 at all, so they
                // are dropped rather than escaped into something that does not parse.
                let cleaned: String = text
                    .chars()
                    .filter(|character| !is_xml_control(*character))
                    .collect();
                out.push_str(&format!("    <{tag}>{}</{tag}>\n", escape(&cleaned)));
            }
        }
    }
    out.push_str(&format!("  </{record}>\n"));
    out
}

/// `[\x00-\x08\x0b\x0c\x0e-\x1f]`, which XML cannot carry in any escaped form.
fn is_xml_control(character: char) -> bool {
    let code = character as u32;
    matches!(code, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f)
}

/// Turn a column name into something XML will accept as a tag.
///
/// Anything outside `[A-Za-z0-9_.-]` becomes `_`, and a name that starts with
/// anything but a letter or `_` gets one in front — `2024_total` would otherwise be
/// an invalid tag.
pub fn xml_tag(name: &str) -> String {
    let mut tag: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '_'
                || character == '.'
                || character == '-'
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let first_is_safe = tag
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_');
    if !first_is_safe {
        tag.insert(0, '_');
    }
    tag
}

pub(crate) fn render_xml(
    options: &ExportOptions,
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
) -> String {
    let tags: Vec<String> = columns.iter().map(|column| xml_tag(&column.name)).collect();
    let mut out = String::new();
    for row in rows {
        out.push_str(&render_xml_row(&tags, &options.xml_record, row));
    }
    out
}

// --------------------------------------------------------------------------- //
// html
// --------------------------------------------------------------------------- //

/// The page shell, byte for byte from the Python engine.
///
/// The braces that belong to the CSS are doubled because this goes through
/// `format!`, exactly as the Python template went through `str.format`.
const HTML_HEAD: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>{title}</title>
<style>
 body{{font:13px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;margin:24px;color:#1a1a1a}}
 table{{border-collapse:collapse;font-variant-numeric:tabular-nums}}
 th,td{{border:1px solid #d8d8d8;padding:4px 9px;text-align:left;vertical-align:top}}
 th{{background:#f2f2f2;position:sticky;top:0;font-weight:600}}
 tr:nth-child(even) td{{background:#fafafa}}
 td.null{{color:#999;font-style:italic}}
</style></head><body>
<h2>{title}</h2>
<table><thead><tr>{head}</tr></thead><tbody>
"#;

pub struct HtmlWriter {
    out: BufWriter<File>,
}

impl HtmlWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        // The file's stem when no title was given, which is what the Python engine
        // showed in the page heading.
        let title = match &options.title {
            Some(title) => title.clone(),
            None => path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let title = escape(&title);
        let head: String = columns
            .iter()
            .map(|column| format!("<th>{}</th>", escape(&column.name)))
            .collect();

        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(
            HTML_HEAD
                .replace("{title}", &title)
                .replace("{head}", &head)
                .as_bytes(),
        )?;
        Ok(Self { out })
    }
}

impl Writer for HtmlWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        self.out.write_all(render_html_row(row).as_bytes())?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.out.write_all(b"</tbody></table></body></html>\n")?;
        self.out.flush()?;
        Ok(())
    }
}

fn render_html_row(row: &[Value]) -> String {
    let mut out = String::from("<tr>");
    for value in row {
        match to_text(value) {
            // NULL is shown rather than left blank: in a browsed page an empty cell
            // and a null cell look the same, and the stylesheet is what tells them
            // apart.
            None => out.push_str("<td class=\"null\">NULL</td>"),
            Some(text) => out.push_str(&format!("<td>{}</td>", escape(&text))),
        }
    }
    out.push_str("</tr>\n");
    out
}

pub(crate) fn render_html(rows: &[Vec<Value>]) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&render_html_row(row));
    }
    out
}

// --------------------------------------------------------------------------- //
// sql
// --------------------------------------------------------------------------- //

/// `INSERT` statements, several rows per statement to keep a replay fast.
///
/// Not renderable in parallel: rows are batched, so a chunk's text depends on how
/// many rows came before it.
pub struct SqlWriter {
    out: BufWriter<File>,
    prefix: String,
    rows_per_insert: usize,
    buffer: Vec<String>,
}

impl SqlWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        let quote = options.sql_ident_quote;
        let table = match &options.sql_table {
            Some(table) => table.clone(),
            None => path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let cols: Vec<String> = columns
            .iter()
            .map(|column| {
                format!(
                    "{quote}{}{quote}",
                    column.name.replace(quote, &quote.to_string().repeat(2))
                )
            })
            .collect();

        Ok(Self {
            out: BufWriter::new(File::create(path)?),
            prefix: format!(
                "INSERT INTO {quote}{table}{quote} ({}) VALUES\n",
                cols.join(", ")
            ),
            rows_per_insert: options.sql_rows_per_insert.max(1),
            buffer: Vec::new(),
        })
    }

    fn flush(&mut self) -> Result<(), ExportError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let text = format!("{}{};\n", self.prefix, self.buffer.join(",\n"));
        self.out.write_all(text.as_bytes())?;
        self.buffer.clear();
        Ok(())
    }
}

impl Writer for SqlWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        let cells: Vec<String> = row.iter().map(literal).collect();
        self.buffer.push(format!("({})", cells.join(", ")));
        if self.buffer.len() >= self.rows_per_insert {
            self.flush()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.flush()?;
        self.out.flush()?;
        Ok(())
    }
}

/// One SQL literal.
///
/// The order of the checks is the Python engine's and it matters: `bool` is a
/// subclass of `int` in Python, so the boolean check has to come first or `true`
/// becomes `1`.
pub(crate) fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::Bool(flag) => if *flag { "TRUE" } else { "FALSE" }.to_owned(),
        // Numbers are written bare, because a quoted number is a string to every
        // database that reads this back.
        Value::Int(number) => number.to_string(),
        Value::UInt(number) => number.to_string(),
        Value::Float(number) => qh_core::render::format_float(*number),
        Value::Decimal { unscaled, scale } => qh_core::render::format_decimal(*unscaled, *scale),
        // Hex bytes, the same form SQL itself uses for a binary literal.
        Value::Bytes(bytes) => format!("X'{}'", qh_core::render::hex_encode(bytes)),
        // Everything else is a string: a timestamp written as a quoted ISO text is
        // what every dialect will parse back, and a doubled quote is SQL's own
        // escaping.
        other => {
            let text = to_text(other).unwrap_or_default();
            format!("'{}'", text.replace('\'', "''"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> Vec<ColumnMeta> {
        names
            .iter()
            .map(|name| ColumnMeta::new(*name, "varchar"))
            .collect()
    }

    fn write_text(
        format: Format,
        columns: &[ColumnMeta],
        options: &ExportOptions,
        rows: &[Vec<Value>],
    ) -> String {
        // Unique per call, because the test harness runs these in parallel threads of
        // one process: naming the file after the format alone had every CSV test
        // writing the same file and comparing against another test's output.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!(
            "out-{}-{unique}.{}",
            format.name(),
            format.extension()
        ));
        let mut writer = crate::open(format, &path, columns, options).expect("open");
        for row in rows {
            writer.write_row(row).expect("write_row");
        }
        writer.finish().expect("finish");
        std::fs::read_to_string(&path).expect("read back")
    }

    /// The same export, read back as bytes — for the tests whose point is that the file
    /// is *not* UTF-8, which a `String` cannot hold.
    fn write_bytes(
        format: Format,
        columns: &[ColumnMeta],
        options: &ExportOptions,
        rows: &[Vec<Value>],
    ) -> Vec<u8> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-export-bytes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!(
            "bytes-{}-{unique}.{}",
            format.name(),
            format.extension()
        ));
        let mut writer = crate::open(format, &path, columns, options).expect("open");
        for row in rows {
            writer.write_row(row).expect("write_row");
        }
        writer.finish().expect("finish");
        std::fs::read(&path).expect("read back")
    }

    #[test]
    fn encoding_names_the_code_page_a_delimited_file_is_written_in() {
        let rows = [vec![Value::Text("café — 名前".into())]];
        let write = |encoding: &str| {
            let options = ExportOptions {
                encoding: encoding.to_owned(),
                ..ExportOptions::default()
            };
            write_bytes(Format::Csv, &columns(&["a"]), &options, &rows)
        };

        // UTF-8 is the default and holds everything.
        assert_eq!(write("utf-8"), "a\r\ncafé — 名前\r\n".as_bytes());
        // cp1252 keeps the accented text and the em dash, and answers `?` for the two
        // characters it has no byte for — Python's `errors="replace"`.
        assert_eq!(write("cp1252"), b"a\r\ncaf\xe9 \x97 ??\r\n");
        // latin-1 keeps é and refuses the em dash: U+2014 is not in it, while cp1252 has
        // it at 0x97. This is the pair that must not be folded into one code page.
        assert_eq!(write("latin-1"), b"a\r\ncaf\xe9 ? ??\r\n");
        // The DOS pages put é at 0x82, and agree on the rest of this value.
        assert_eq!(write("cp437"), b"a\r\ncaf\x82 ? ??\r\n");
        // And the same writer with a tab: `txt` takes `ENCODING` too, since both
        // formats are one `DelimitedWriter`.
        let options = ExportOptions {
            encoding: "latin-1".to_owned(),
            ..ExportOptions::default()
        };
        assert_eq!(
            write_bytes(
                Format::Text,
                &columns(&["a", "b"]),
                &options,
                &[vec![Value::Text("café".into()), Value::Text("—".into())]],
            ),
            b"a\tb\r\ncaf\xe9\t?\r\n"
        );
    }

    #[test]
    fn a_bom_is_only_written_for_utf_8() {
        // The Python engine gated its BOM on the encoding name, and it has to: three
        // UTF-8 bytes at the head of a cp1252 file are not a byte-order mark, they are
        // corruption.
        let rows = [vec![Value::Text("café".into())]];
        let options = ExportOptions {
            bom: true,
            encoding: "cp1252".to_owned(),
            ..ExportOptions::default()
        };
        let got = write_bytes(Format::Csv, &columns(&["a"]), &options, &rows);
        assert_eq!(
            got, b"a\r\ncaf\xe9\r\n",
            "no BOM for a file that is not UTF-8"
        );
        // And the UTF-8 case still gets one.
        let options = ExportOptions {
            bom: true,
            ..ExportOptions::default()
        };
        let got = write_bytes(Format::Csv, &columns(&["a"]), &options, &rows);
        assert!(got.starts_with("\u{feff}".as_bytes()), "the BOM leads");
    }

    #[test]
    fn an_unknown_encoding_is_refused_and_no_file_is_left_behind() {
        let dir = std::env::temp_dir().join(format!("qh-export-refused-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("never-written.csv");
        let options = ExportOptions {
            encoding: "utf-7".to_owned(),
            ..ExportOptions::default()
        };
        let error = match crate::open(Format::Csv, &path, &columns(&["a"]), &options) {
            Err(error) => error,
            Ok(_) => panic!("an unknown code page must be refused, not ignored"),
        };
        let message = error.to_string();
        assert!(
            message.starts_with("unknown ENCODING 'utf-7';"),
            "{message}"
        );
        assert!(!path.exists(), "a refused export must not leave a file");
        // A format that does not take an encoding at all is unaffected: this is Python's
        // behaviour too, because its JSON writer never read the option.
        assert!(crate::open(Format::Json, &path, &columns(&["a"]), &options).is_ok());
    }

    #[test]
    fn csv_quotes_only_what_has_to_be_quoted() {
        let options = ExportOptions::default();
        let got = write_text(
            Format::Csv,
            &columns(&["a", "b", "c"]),
            &options,
            &[
                vec![Value::Text("plain".into()), Value::Int(1), Value::Null],
                // A comma, a quote and a newline each force quoting; note the doubled
                // quote inside.
                vec![
                    Value::Text("has,comma".into()),
                    Value::Text("has\"quote".into()),
                    Value::Text("has\nnewline".into()),
                ],
            ],
        );
        // `\r\n` line endings, which is what the Python engine wrote and what Excel
        // expects. A NULL is an empty cell, not the word NULL.
        assert_eq!(
            got,
            "a,b,c\r\nplain,1,\r\n\"has,comma\",\"has\"\"quote\",\"has\nnewline\"\r\n"
        );
    }

    #[test]
    fn text_is_the_same_writer_with_a_tab() {
        let got = write_text(
            Format::Text,
            &columns(&["a", "b"]),
            &ExportOptions::default(),
            &[vec![Value::Text("x".into()), Value::Int(2)]],
        );
        assert_eq!(got, "a\tb\r\nx\t2\r\n");
    }

    #[test]
    fn a_header_is_the_columns_and_can_be_turned_off() {
        let options = ExportOptions {
            header: false,
            ..ExportOptions::default()
        };
        let got = write_text(
            Format::Csv,
            &columns(&["a"]),
            &options,
            &[vec![Value::Int(1)]],
        );
        assert_eq!(got, "1\r\n");
    }

    #[test]
    fn a_bom_is_written_when_it_is_asked_for() {
        let options = ExportOptions {
            bom: true,
            ..ExportOptions::default()
        };
        let got = write_text(Format::Csv, &columns(&["a"]), &options, &[]);
        assert!(got.starts_with('\u{feff}'), "the BOM should lead the file");
        assert_eq!(got, "\u{feff}a\r\n");
    }

    #[test]
    fn bytes_are_hex_in_a_delimited_cell() {
        // The same rule as qh-core::render, and it is worth pinning at this layer too
        // because this is where a file is actually produced.
        let got = write_text(
            Format::Csv,
            &columns(&["a"]),
            &ExportOptions::default(),
            &[vec![Value::Bytes(vec![0x00, 0xff])]],
        );
        assert_eq!(got, "a\r\n00ff\r\n");
    }

    #[test]
    fn json_array_form_indents_the_python_way() {
        let got = write_text(
            Format::Json,
            &columns(&["a", "b"]),
            &ExportOptions::default(),
            &[
                vec![Value::Int(1), Value::Text("x".into())],
                vec![Value::Int(2), Value::Null],
            ],
        );
        // Every line carries two extra spaces, so the braces line up under the opening
        // bracket. This is what `"".join("  " + ln ...)` in the Python writer does.
        assert_eq!(
            got,
            "[\n  {\n    \"a\": 1,\n    \"b\": \"x\"\n  },\n  {\n    \"a\": 2,\n    \"b\": null\n  }\n]\n"
        );
    }

    #[test]
    fn an_empty_json_export_closes_on_one_line() {
        let got = write_text(
            Format::Json,
            &columns(&["a"]),
            &ExportOptions::default(),
            &[],
        );
        assert_eq!(got, "[\n]\n");
    }

    #[test]
    fn jsonl_writes_one_object_per_line_with_pythons_separators() {
        let options = ExportOptions {
            jsonl: true,
            ..ExportOptions::default()
        };
        let got = write_text(
            Format::Json,
            &columns(&["a", "b"]),
            &options,
            &[vec![Value::Int(1), Value::Text("x".into())]],
        );
        // ", " and ": " even without indentation: Python's default separators.
        assert_eq!(got, "{\"a\": 1, \"b\": \"x\"}\n");
    }

    #[test]
    fn a_number_stays_a_number_in_json() {
        // The bug this test exists for was real, in the layer below: integers rendered
        // as quoted strings, which hands a downstream reader strings where the query
        // returned numbers.
        let options = ExportOptions {
            jsonl: true,
            ..ExportOptions::default()
        };
        let got = write_text(
            Format::Json,
            &columns(&["n", "arr"]),
            &options,
            &[vec![
                Value::Int(1),
                Value::Array(vec![Value::Int(1), Value::Int(2)]),
            ]],
        );
        assert_eq!(got, "{\"n\": 1, \"arr\": [1, 2]}\n");
    }

    #[test]
    fn xml_uses_html_escaping_and_marks_a_null_as_nil() {
        let got = write_text(
            Format::Xml,
            &columns(&["a", "b"]),
            &ExportOptions::default(),
            &[
                vec![Value::Text("a\"b<c>&d'e".into()), Value::Null],
                vec![Value::Int(1), Value::Text("x".into())],
            ],
        );
        assert_eq!(
            got,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RECORDS>\n  <RECORD>\n    \
             <a>a&quot;b&lt;c&gt;&amp;d&#x27;e</a>\n    <b xsi:nil=\"true\"/>\n  </RECORD>\n  \
             <RECORD>\n    <a>1</a>\n    <b>x</b>\n  </RECORD>\n</RECORDS>\n"
        );
    }

    #[test]
    fn an_xml_tag_is_made_legal_without_losing_the_name() {
        // A space and a leading digit both have to be dealt with, and a name that is
        // already legal must not be touched.
        assert_eq!(xml_tag("total"), "total");
        assert_eq!(xml_tag("total amount"), "total_amount");
        assert_eq!(xml_tag("2024_total"), "_2024_total");
        assert_eq!(xml_tag("a-b.c"), "a-b.c");
        assert_eq!(xml_tag("café"), "caf_");
    }

    #[test]
    fn html_escapes_its_cells_and_shows_a_null() {
        let got = write_text(
            Format::Html,
            &columns(&["a"]),
            &ExportOptions::default(),
            &[vec![Value::Text("<b>".into())], vec![Value::Null]],
        );
        assert!(got.starts_with("<!doctype html>"));
        assert!(got.contains("<th>a</th>"));
        assert!(got.contains("<td>&lt;b&gt;</td>"));
        // Shown, not left blank: in a page an empty cell and a null cell look the same.
        assert!(got.contains("<td class=\"null\">NULL</td>"));
        assert!(got.ends_with("</tbody></table></body></html>\n"));
    }

    #[test]
    fn sql_writes_numbers_bare_and_strings_quoted() {
        // The table name is given explicitly so the assertion does not depend on the
        // temp file's name, which is unique per call.
        let options = ExportOptions {
            sql_table: Some("out".to_owned()),
            ..ExportOptions::default()
        };
        let got = write_text(
            Format::Sql,
            &columns(&["n", "t", "b", "raw"]),
            &options,
            &[vec![
                Value::Int(42),
                Value::Text("o'brien".into()),
                Value::Bool(true),
                Value::Bytes(vec![0x00, 0xff]),
            ]],
        );
        assert!(
            got.starts_with("INSERT INTO \"out\" (\"n\", \"t\", \"b\", \"raw\") VALUES\n"),
            "{got}"
        );
        // A quote is doubled; a boolean is TRUE and not 1, which is the order the
        // Python engine's checks are in; bytes are a hex literal.
        assert!(got.contains("(42, 'o''brien', TRUE, X'00ff')"), "{got}");
        assert!(got.ends_with(";\n"));
    }

    #[test]
    fn sql_batches_rows_and_writes_a_statement_per_batch() {
        let options = ExportOptions {
            sql_rows_per_insert: 2,
            sql_table: Some("t".to_owned()),
            ..ExportOptions::default()
        };
        let got = write_text(
            Format::Sql,
            &columns(&["a"]),
            &options,
            &[
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
        );
        // Two statements: two rows then one, which is the remainder.
        assert_eq!(
            got,
            "INSERT INTO \"t\" (\"a\") VALUES\n(1),\n(2);\nINSERT INTO \"t\" (\"a\") VALUES\n(3);\n"
        );
    }

    #[test]
    fn a_null_is_spelled_differently_in_every_format() {
        // The reason to_text returns an Option: each writer decides, and a shared
        // empty string would have taken that decision away from all of them.
        let row = [vec![Value::Null]];
        let cols = columns(&["a"]);
        let options = ExportOptions::default();

        assert!(write_text(Format::Csv, &cols, &options, &row).contains("\r\n\r\n"));
        assert!(write_text(Format::Json, &cols, &options, &row).contains("null"));
        assert!(write_text(Format::Xml, &cols, &options, &row).contains("xsi:nil"));
        assert!(write_text(Format::Html, &cols, &options, &row).contains(">NULL<"));
        assert!(write_text(Format::Sql, &cols, &options, &row).contains("(NULL)"));
    }

    #[test]
    fn rendering_a_chunk_gives_exactly_what_writing_it_row_by_row_would_have() {
        // The property that makes the parallel path safe, and the only reason
        // `render_rows` is allowed to exist. If this fails, using it from several
        // threads produces a file that differs from the sequential one.
        let cols = columns(&["a", "b"]);
        let options = ExportOptions::default();
        let rows: Vec<Vec<Value>> = (0..6)
            .map(|n| vec![Value::Int(n), Value::Text(format!("v{n}").into())])
            .collect();

        for format in [Format::Text, Format::Csv] {
            let sequential = {
                let mut out = String::new();
                for chunk in rows.chunks(2) {
                    out.push_str(&render_delimited(format, &options, chunk));
                }
                out
            };
            // The delimiter default differs per format, so the rendered chunk is
            // compared against the rendered whole rather than against a literal.
            let whole = render_delimited(format, &options, &rows);
            assert_eq!(sequential, whole, "{}", format.name());

            // And the writer's file is exactly its header followed by those rows. The
            // render path deliberately carries no header -- the writer writes that
            // when it opens, as Python's renderable writers do -- so this is the
            // assertion that ties the two paths together.
            let header = match format {
                Format::Csv => "a,b\r\n",
                _ => "a\tb\r\n",
            };
            let written = write_text(format, &cols, &options, &rows);
            assert_eq!(written, format!("{header}{whole}"), "{}", format.name());
        }
    }

    #[test]
    fn rendering_xml_and_html_in_chunks_matches_writing_it_all_at_once() {
        let cols = columns(&["a"]);
        let options = ExportOptions::default();
        let rows: Vec<Vec<Value>> = (0..5).map(|n| vec![Value::Int(n)]).collect();

        let mut chunked = String::new();
        for chunk in rows.chunks(2) {
            chunked.push_str(&render_xml(&options, &cols, chunk));
        }
        let whole = render_xml(&options, &cols, &rows);
        assert_eq!(chunked, whole);

        let mut chunked = String::new();
        for chunk in rows.chunks(2) {
            chunked.push_str(&render_html(chunk));
        }
        assert_eq!(chunked, render_html(&rows));
    }
}
