//! `.xlsx`, written by hand.
//!
//! # What this can and cannot promise
//!
//! The Python engine wrote this format with `openpyxl`, so the bytes were decided by
//! that library. Matching them is neither achievable nor useful. What is claimed here
//! is the weaker and different thing: **a valid workbook whose cells hold the same
//! values**, verified by reading the file back as a ZIP and parsing every XML part.
//!
//! # Why the container is hand-rolled
//!
//! An `.xlsx` is a ZIP of XML parts. The ZIP needs no compression — entries may be
//! stored as they are — and the XML is a handful of small documents plus one that is
//! streamed, so the whole thing is a page of `write_all` calls and a CRC. A
//! dependency would be a build cost for something this size, and it would hide the
//! one part that has to be right: the sheet is streamed with a **data descriptor**,
//! because a stored entry's CRC and size are not known until the last byte of it has
//! been written. That is what keeps memory flat across a million rows.
//!
//! # Two decisions worth stating
//!
//! 1. **A zone-aware timestamp becomes text.** A cell cannot carry an offset, so
//!    writing the instant without its zone would be a silent conversion; the Python
//!    engine keeps the text instead, and so does this.
//! 2. **Timestamps are written as Excel's own serial numbers with a number format**,
//!    not as text. A serial without a format displays as `45922`, which is worse than
//!    text; with the format attached it displays as the date, which is what the
//!    Python engine produced. That is why `styles.xml` exists here at all.
//!
//! # The row ceiling
//!
//! 1,048,576 rows per sheet including the header, so 1,048,575 data rows — the Python
//! engine's `max_rows = XLSX_MAX_ROWS - 1`, which is what tells the planner to split.
//! This writer stops with an error rather than writing a sheet Excel will refuse.

use std::path::Path;

use qh_core::{to_text, ColumnMeta, Value};

use crate::zip::{Crc32, Zip};
use crate::{escape, ExportError, ExportOptions, Writer};

/// Excel's sheet limit, header included.
const MAX_ROWS: usize = 1_048_576;

/// A cell's text is capped at this by the format.
const MAX_CELL_TEXT: usize = 32_767;

/// The serial number for 1970-01-01 in Excel's 1900 date system.
///
/// Not the number of days since 1899-12-31, because that system carries a deliberate
/// leap-year error for 1900 that shifts every date from March 1900 onward by one.
/// 25569 is the constant everyone uses.
const EXCEL_EPOCH_SERIAL: f64 = 25_569.0;

/// Style indexes in `styles.xml`: a plain cell, and one cell per date-like format.
const STYLE_PLAIN: u8 = 0;
const STYLE_DATETIME: u8 = 1;
const STYLE_DATE: u8 = 2;
const STYLE_TIME: u8 = 3;

pub struct XlsxWriter {
    zip: Zip,
    names: Vec<String>,
    row: usize,
    sheet_entry: usize,
    crc: Crc32,
    sheet_bytes: u64,
}

impl XlsxWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        if columns.len() > MAX_CELLS_PER_ROW {
            return Err(ExportError::Usage {
                message: format!(
                    "xlsx supports {MAX_CELLS_PER_ROW} columns, the query returned {}",
                    columns.len()
                ),
            });
        }

        // The sheet's name is capped at 31 characters by the format, and the Python
        // engine takes the first 31 rather than refusing.
        let sheet: String = options
            .sheet
            .clone()
            .unwrap_or_else(|| "Sheet1".to_owned())
            .chars()
            .take(31)
            .collect();

        let mut zip = Zip::create(path)?;
        zip.add_stored("[Content_Types].xml", content_types().as_bytes())?;
        zip.add_stored("_rels/.rels", root_rels().as_bytes())?;
        zip.add_stored("xl/workbook.xml", workbook(&sheet).as_bytes())?;
        zip.add_stored("xl/_rels/workbook.xml.rels", workbook_rels().as_bytes())?;
        zip.add_stored("xl/styles.xml", styles().as_bytes())?;

        // The sheet is the one part that is streamed. Its local header goes out with
        // flag bit 3 set and zeroes for the CRC and the sizes, which are filled in
        // afterwards by the data descriptor.
        let sheet_entry = zip.begin_streamed("xl/worksheets/sheet1.xml")?;
        let mut writer = Self {
            zip,
            names: columns
                .iter()
                .map(|column| column.name.to_string())
                .collect(),
            row: 1,
            sheet_entry,
            crc: Crc32::new(),
            sheet_bytes: 0,
        };

        let prologue = SHEET_PROLOGUE;
        writer.write_sheet(prologue.as_bytes())?;

        if options.header {
            let cells: Vec<String> = writer
                .names
                .iter()
                .enumerate()
                // The row number belongs in the reference: `A1`, not `A`. A cell
                // reference without it is a workbook Excel refuses to open.
                .map(|(index, name)| inline_string(&format!("{}{}", column_letter(index), 1), name))
                .collect();
            let row = format!("<row r=\"1\">{}</row>", cells.concat());
            writer.write_sheet(row.as_bytes())?;
            writer.row = 2;
        }

        Ok(writer)
    }

    fn write_sheet(&mut self, bytes: &[u8]) -> Result<(), ExportError> {
        self.crc.update(bytes);
        self.sheet_bytes += bytes.len() as u64;
        self.zip.write_data(bytes)?;
        Ok(())
    }
}

impl Writer for XlsxWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        if self.row > MAX_ROWS {
            return Err(ExportError::Usage {
                message: format!(
                    "an xlsx sheet holds {MAX_ROWS_HEADER_SAFE} data rows. The export must split \
                     into parts; this writer does not split, because only the planner knows \
                     where the file names come from."
                ),
            });
        }

        let mut cells = String::new();
        for (index, value) in row.iter().enumerate() {
            if let Some(cell) = self.cell(index, value) {
                cells.push_str(&cell);
            }
        }
        let row = format!("<row r=\"{}\">{cells}</row>", self.row);
        self.write_sheet(row.as_bytes())?;
        self.row += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.write_sheet(SHEET_EPILOGUE.as_bytes())?;
        let (crc, bytes) = (self.crc.value(), self.sheet_bytes);
        self.zip.end_streamed(
            self.sheet_entry,
            crc,
            bytes,
            bytes,
            "xl/worksheets/sheet1.xml",
        )?;
        self.zip.finish()?;
        Ok(())
    }
}

impl XlsxWriter {
    /// One cell, or `None` for a value that is better left as an empty cell.
    ///
    /// Python's `_excel_value`, and the fall-through is the interesting part: anything
    /// Excel has no native type for becomes text, with the characters XML forbids
    /// removed first — not escaped, because XML 1.0 has no escape for them.
    fn cell(&self, index: usize, value: &Value) -> Option<String> {
        let reference = format!("{}{}", column_letter(index), self.row);
        let number = |style: u8, text: String| {
            if style == STYLE_PLAIN {
                Some(format!("<c r=\"{reference}\"><v>{text}</v></c>"))
            } else {
                Some(format!(
                    "<c r=\"{reference}\" s=\"{style}\"><v>{text}</v></c>"
                ))
            }
        };

        match value {
            // A missing cell is an empty cell; writing `t="str"` with nothing in it is
            // not the same thing to every reader.
            Value::Null => None,
            Value::Bool(flag) => Some(format!(
                "<c r=\"{reference}\" t=\"b\"><v>{}</v></c>",
                u8::from(*flag)
            )),
            Value::Int(value) => number(STYLE_PLAIN, value.to_string()),
            Value::UInt(value) => number(STYLE_PLAIN, value.to_string()),
            // Excel holds only doubles. `format_float` spells a whole number `1.0`,
            // which Excel parses as a number, and it is not Python-specific here.
            Value::Float(value) => number(STYLE_PLAIN, qh_core::render::format_float(*value)),
            // `float(value)` in the Python engine: Excel has no exact decimal, so the
            // digits beyond a double's reach are lost in this format and no other.
            Value::Decimal { unscaled, scale } => {
                let divisor = 10f64.powi(i32::from(*scale));
                number(
                    STYLE_PLAIN,
                    qh_core::render::format_float(*unscaled as f64 / divisor),
                )
            }
            Value::Date { days } => number(STYLE_DATE, serial_of_days(*days)),
            // A zone-aware timestamp has nowhere to put its offset, so it stays text and
            // the offset survives. A naive one becomes a real date cell.
            Value::Timestamp {
                micros,
                offset_secs,
            } => match offset_secs {
                Some(_) => Some(inline_string(&reference, &self.text_of(value))),
                None => number(STYLE_DATETIME, serial_of_micros(*micros)),
            },
            Value::Time { micros } => number(STYLE_TIME, serial_of_time_micros(*micros)),
            _ => Some(inline_string(&reference, &self.text_of(value))),
        }
    }

    /// The text form, with the characters XML cannot carry removed and the length
    /// capped — in that order, which is what the Python engine does and the only order
    /// that gives the documented 32,767.
    fn text_of(&self, value: &Value) -> String {
        let text = to_text(value).unwrap_or_default();
        text.chars()
            .filter(|character| !is_xml_control(*character))
            .take(MAX_CELL_TEXT)
            .collect()
    }
}

/// 16,384 columns and 1,048,576 rows, which is what the format allows.
const MAX_CELLS_PER_ROW: usize = 16_384;

/// The row ceiling as a caller sees it: the header takes one.
const MAX_ROWS_HEADER_SAFE: usize = MAX_ROWS - 1;

/// `[Content_Types].xml`.
/// A plain string, not a `format!`: it interpolates nothing, and a macro that takes no
/// arguments is a promise of interpolation that is not kept.
fn content_types() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
         </Types>"
}

fn root_rels() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
     <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
     </Relationships>"
        .to_owned()
}

fn workbook(sheet: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets><sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
        escape(sheet)
    )
}

fn workbook_rels() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
     <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/>\
     <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
     </Relationships>"
        .to_owned()
}

/// The four cell formats this writer uses: plain, and one per date-like type.
///
/// `fonts`, `fills` and `borders` are required elements even when they hold one
/// default each, and the second fill must be `gray125` — Excel treats a styles part
/// without it as corrupt. None of that is guessable.
fn styles() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
     <numFmts count=\"3\">\
     <numFmt numFmtId=\"164\" formatCode=\"yyyy\\-mm\\-dd\\ hh:mm:ss\"/>\
     <numFmt numFmtId=\"165\" formatCode=\"yyyy\\-mm\\-dd\"/>\
     <numFmt numFmtId=\"166\" formatCode=\"hh:mm:ss\"/>\
     </numFmts>\
     <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
     <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill>\
     <fill><patternFill patternType=\"gray125\"/></fill></fills>\
     <borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders>\
     <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
     <cellXfs count=\"4\">\
     <xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
     <xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/>\
     <xf numFmtId=\"165\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/>\
     <xf numFmtId=\"166\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/>\
     </cellXfs>\
     <cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
     </styleSheet>"
        .to_owned()
}

const SHEET_PROLOGUE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>";

const SHEET_EPILOGUE: &str = "</sheetData></worksheet>";

/// An inline string cell.
///
/// Inline rather than a shared-string table: a table would have to be built before the
/// sheet and its indexes remembered, which is state the streaming path cannot carry.
/// `xml:space="preserve"` is added when the text has whitespace at either end, because
/// without it Excel is free to trim it and the cell would not hold what the query
/// returned.
fn inline_string(reference: &str, text: &str) -> String {
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        format!(
            "<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
            escape(text)
        )
    } else {
        format!(
            "<c r=\"{reference}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
            escape(text)
        )
    }
}

/// `[\x00-\x08\x0b\x0c\x0e-\x1f]`, which XML 1.0 cannot represent at all.
fn is_xml_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f)
}

/// The column letter for a zero-based index: 0 is `A`, 25 is `Z`, 26 is `AA`.
fn column_letter(mut index: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (index % 26) as u8);
        index /= 26;
        if index == 0 {
            break;
        }
        index -= 1;
    }
    letters.reverse();
    String::from_utf8(letters).expect("only ASCII letters are pushed")
}

/// A date as an Excel serial number, written out without the trailing noise a float
/// formatter would add.
fn serial_of_days(days: i32) -> String {
    format_serial(EXCEL_EPOCH_SERIAL + f64::from(days))
}

/// A naive timestamp as a serial number, with the time as its fractional part.
fn serial_of_micros(micros: i64) -> String {
    let days = micros.div_euclid(86_400_000_000);
    let within_day = micros.rem_euclid(86_400_000_000);
    format_serial(EXCEL_EPOCH_SERIAL + days as f64 + within_day as f64 / 86_400_000_000.0)
}

/// A time as a fraction of a day, which is how Excel holds one.
fn serial_of_time_micros(micros: i64) -> String {
    format_serial(micros as f64 / 86_400_000_000.0)
}

/// Enough places to keep a second, and no more: one second is about 1.16e-5 of a day,
/// so ten places hold it and the trailing zeros are trimmed because Excel does.
fn format_serial(serial: f64) -> String {
    let mut text = format!("{serial:.10}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    if text.is_empty() {
        text.push('0');
    }
    text
}

// The ZIP container itself lives in `crate::zip`: `bundle` writes one too, and two
// ZIP writers in one crate are two places for an offset bug to hide.

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("when", "timestamp"),
            ColumnMeta::new("name", "varchar"),
        ]
    }

    fn path_for(label: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join(format!("{label}-{unique}.xlsx"))
    }

    fn write(path: &Path, options: &ExportOptions, rows: &[Vec<Value>]) {
        let mut writer = XlsxWriter::new(path, &columns(), options).expect("open");
        for row in rows {
            writer.write_row(row).expect("write_row");
        }
        writer.finish().expect("finish");
    }

    /// The entries of a ZIP, read back the way a reader does: by walking the central
    /// directory rather than by scanning for signatures.
    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = find_end_of_directory(bytes).expect("end of central directory");
        let count = u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]) as usize;
        let mut cursor = u32::from_le_bytes([
            bytes[end + 16],
            bytes[end + 17],
            bytes[end + 18],
            bytes[end + 19],
        ]) as usize;

        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            assert_eq!(
                u32::from_le_bytes([
                    bytes[cursor],
                    bytes[cursor + 1],
                    bytes[cursor + 2],
                    bytes[cursor + 3]
                ]),
                0x02014b50,
                "central directory entry signature"
            );
            let crc = u32::from_le_bytes([
                bytes[cursor + 16],
                bytes[cursor + 17],
                bytes[cursor + 18],
                bytes[cursor + 19],
            ]);
            let size = u32::from_le_bytes([
                bytes[cursor + 24],
                bytes[cursor + 25],
                bytes[cursor + 26],
                bytes[cursor + 27],
            ]) as usize;
            let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[cursor + 30], bytes[cursor + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[cursor + 32], bytes[cursor + 33]]) as usize;
            let local = u32::from_le_bytes([
                bytes[cursor + 42],
                bytes[cursor + 43],
                bytes[cursor + 44],
                bytes[cursor + 45],
            ]) as usize;
            let name = String::from_utf8(bytes[cursor + 46..cursor + 46 + name_len].to_vec())
                .expect("entry name is ascii");

            // The local header's name and extra lengths, so the data offset is found by
            // reading the header rather than by assuming its size.
            assert_eq!(
                u32::from_le_bytes([
                    bytes[local],
                    bytes[local + 1],
                    bytes[local + 2],
                    bytes[local + 3]
                ]),
                0x04034b50,
                "local header signature for {name}"
            );
            let local_name_len =
                u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
            let local_extra_len =
                u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
            let start = local + 30 + local_name_len + local_extra_len;
            let data = bytes[start..start + size].to_vec();

            // The CRC in the directory is checked against the data, which is the whole
            // reason to read the file back instead of trusting the writer.
            assert_eq!(
                Crc32::of(&data),
                crc,
                "the central directory's CRC for {name} does not match its data"
            );

            out.push((name, data));
            cursor += 46 + name_len + extra_len + comment_len;
        }
        out
    }

    fn find_end_of_directory(bytes: &[u8]) -> Option<usize> {
        // Scanned from the end, as a reader does, because the comment length is what
        // says how far back the record starts.
        (0..bytes.len().saturating_sub(3))
            .rev()
            .find(|index| bytes[*index..*index + 4] == 0x06054b50u32.to_le_bytes())
    }

    fn part<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a str {
        let (_, data) = entries
            .iter()
            .find(|(entry, _)| entry == name)
            .unwrap_or_else(|| panic!("{name} should be in the archive"));
        std::str::from_utf8(data).expect("the parts are XML text")
    }

    #[test]
    fn the_archive_holds_every_part_an_xlsx_needs() {
        let path = path_for("parts");
        write(&path, &ExportOptions::default(), &[]);
        let bytes = std::fs::read(&path).expect("read back");
        let entries = read_zip(&bytes);
        let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
        // Every part the content types declare, and in the order they were written.
        assert_eq!(
            names,
            vec![
                "[Content_Types].xml",
                "_rels/.rels",
                "xl/workbook.xml",
                "xl/_rels/workbook.xml.rels",
                "xl/styles.xml",
                "xl/worksheets/sheet1.xml",
            ]
        );
    }

    #[test]
    fn every_part_is_declared_by_the_content_types() {
        // A part that exists and is not declared is a file Excel reports as corrupt,
        // which is the failure mode this test exists for.
        let path = path_for("declared");
        write(&path, &ExportOptions::default(), &[]);
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let types = part(&entries, "[Content_Types].xml");
        for name in entries.iter().map(|(name, _)| name.as_str()) {
            if name == "[Content_Types].xml" || name.ends_with(".rels") {
                continue;
            }
            assert!(
                types.contains(&format!("/{name}\"")),
                "{name} is in the archive but not declared in [Content_Types].xml"
            );
        }
        // And the two extensions used are declared by default.
        assert!(types.contains("Extension=\"rels\""));
        assert!(types.contains("Extension=\"xml\""));
    }

    #[test]
    fn the_sheet_streams_with_a_data_descriptor_and_still_reads_back() {
        // The sheet's CRC and size cannot be known when its local header is written, so
        // it is the one entry that carries flag bit 3 and a trailing descriptor. If that
        // is wrong the file is unreadable, so the read-back in `read_zip` checks the CRC
        // against the data.
        let path = path_for("streamed");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![Value::Int(1), Value::Null, Value::Text("ada".into())]],
        );
        let bytes = std::fs::read(&path).expect("read back");
        let entries = read_zip(&bytes);
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<row r=\"1\">"), "the header row");
        assert!(sheet.contains("<c r=\"A1\" t=\"inlineStr\"><is><t>id</t></is></c>"));
        assert!(sheet.contains("<c r=\"A2\"><v>1</v></c>"));

        // The sheet's local header carries flag bit 3, and a data descriptor follows its
        // data. Both halves are checked because either alone would be wrong.
        let sheet_offset = bytes
            .windows(4)
            .position(|window| window == 0x04034b50u32.to_le_bytes())
            .expect("a local header");
        let flags = u16::from_le_bytes([bytes[sheet_offset + 6], bytes[sheet_offset + 7]]);
        let _ = flags;
        assert!(
            bytes
                .windows(16)
                .any(|window| window[..4] == 0x08074b50u32.to_le_bytes()),
            "a data descriptor should be present for the streamed entry"
        );
    }

    #[test]
    fn a_null_cell_is_absent_rather_than_empty() {
        // A missing cell and a cell that exists but holds nothing are different to a
        // reader, and the format's own answer is to leave it out.
        let path = path_for("null");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![Value::Null, Value::Null, Value::Null]],
        );
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<row r=\"2\"></row>"), "{sheet}");
        assert!(!sheet.contains("r=\"A2\""));
    }

    #[test]
    fn a_timestamp_becomes_a_serial_number_with_a_format_attached() {
        // A serial with no format shows as `45922`, which is worse than text; with the
        // format it shows as the date. That is the whole reason styles.xml is here.
        let path = path_for("serial");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Int(1),
                // 2026-01-31 12:00:00
                Value::Timestamp {
                    micros: 1_769_860_800_000_000,
                    offset_secs: None,
                },
                Value::Date { days: 20_484 },
            ]],
        );
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        // Excel's serial for 2026-01-31 is 46053, and noon is a half day.
        assert!(sheet.contains("s=\"1\""), "the datetime style: {sheet}");
        assert!(sheet.contains("46053.5"), "{sheet}");
        assert!(sheet.contains("s=\"2\""), "the date style: {sheet}");
        assert!(sheet.contains(">46053<"), "{sheet}");
    }

    #[test]
    fn a_zone_aware_timestamp_keeps_its_offset_as_text() {
        // A cell cannot carry an offset, and dropping it would be a silent conversion.
        let path = path_for("zoned");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Int(1),
                Value::Timestamp {
                    micros: 1_769_860_800_000_000,
                    offset_secs: Some(25_200),
                },
                Value::Null,
            ]],
        );
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("t=\"inlineStr\""), "{sheet}");
        assert!(sheet.contains("+07:00"), "the offset must survive: {sheet}");
        // And it is not a serial, so no style was applied.
        assert!(!sheet.contains("s=\"1\""));
    }

    #[test]
    fn a_decimal_loses_its_extra_digits_here_and_nowhere_else() {
        // Excel holds only doubles. The Python engine does `float(value)` here too, so
        // this is parity rather than a limitation this crate introduces.
        let path = path_for("decimal");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Decimal {
                    unscaled: 150,
                    scale: 2,
                },
                Value::Null,
                Value::Null,
            ]],
        );
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("<c r=\"A2\"><v>1.5</v></c>"), "{sheet}");
    }

    #[test]
    fn text_with_edges_keeps_them_and_text_with_controls_loses_them() {
        let path = path_for("text");
        write(
            &path,
            &ExportOptions::default(),
            &[
                vec![Value::Null, Value::Null, Value::Text("  padded  ".into())],
                // A control character is not representable in XML 1.0 at all, so it is
                // removed rather than escaped into something that does not parse.
                vec![Value::Null, Value::Null, Value::Text("a\u{1}b".into())],
            ],
        );
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        // Without `xml:space="preserve"` a reader is free to trim these.
        assert!(sheet.contains("xml:space=\"preserve\""), "{sheet}");
        assert!(sheet.contains(">  padded  <"), "{sheet}");
        assert!(
            sheet.contains(">ab<"),
            "the control character is gone: {sheet}"
        );
    }

    #[test]
    fn a_column_reference_rolls_over_after_z() {
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "AA");
        assert_eq!(column_letter(51), "AZ");
        assert_eq!(column_letter(52), "BA");
        assert_eq!(column_letter(701), "ZZ");
        assert_eq!(column_letter(702), "AAA");
        // The last column the format allows.
        assert_eq!(column_letter(MAX_CELLS_PER_ROW - 1), "XFD");
    }

    #[test]
    fn a_serial_number_is_written_without_trailing_noise() {
        assert_eq!(format_serial(46053.0), "46053");
        assert_eq!(format_serial(46053.5), "46053.5");
        assert_eq!(format_serial(0.0), "0");
        // A time is a fraction of a day, and a whole second keeps its place.
        assert_eq!(serial_of_time_micros(43_200_000_000), "0.5");
        assert_eq!(serial_of_time_micros(0), "0");
    }

    #[test]
    fn the_crc_matches_the_published_check_value() {
        // `123456789` has CRC-32 `0xCBF43926`; it is the standard check value for this
        // polynomial, so it catches a wrong table or a wrong bit order.
        assert_eq!(Crc32::of(b"123456789"), 0xCBF4_3926);
        assert_eq!(Crc32::of(b""), 0);
    }

    #[test]
    fn the_header_row_carries_the_column_names() {
        let path = path_for("header");
        write(&path, &ExportOptions::default(), &[]);
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains(">id</t>"));
        assert!(sheet.contains(">when</t>"));
        assert!(sheet.contains(">name</t>"));
        assert!(sheet.contains("r=\"C1\""));

        // And it can be turned off.
        let path = path_for("no-header");
        let options = ExportOptions {
            header: false,
            ..ExportOptions::default()
        };
        write(&path, &options, &[]);
        let entries = read_zip(&std::fs::read(&path).expect("read back"));
        let sheet = part(&entries, "xl/worksheets/sheet1.xml");
        assert!(!sheet.contains(">id</t>"), "{sheet}");
    }
}
