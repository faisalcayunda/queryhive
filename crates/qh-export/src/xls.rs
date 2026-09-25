//! `.xls` — OLE2 container, BIFF8 records.
//!
//! # The whole sheet is buffered, and that is inherent
//!
//! BIFF8 has no streaming mode. Cells reference the shared string table by index, so
//! every string in the sheet must be known before the first row can be written. The
//! Python engine buffers for the same reason — its own notes say so, and it is why
//! `max_rows` exists and why the export splits into parts. A million-row stream cannot
//! be written to this format by anyone, so buffering here is parity, not a shortcut.
//!
//! # Verification
//!
//! Unlike `xlsx`, whose bytes belong to `openpyxl`, this format was reverse-engineered
//! from `xlwt`'s own output and then checked by reading the result back with `xlrd` —
//! an independent reader. Two defects were found that way and are worth naming, because
//! both would have produced a file that looks fine until something tries to open it:
//!
//! 1. The STYLE record must be the **built-in** form, with bit 15 of the first field
//!    set. Without it a reader takes the record for a user-defined style and looks for
//!    a name string that is not there.
//! 2. BOUNDSHEET carries a **name flags byte** after the length. Leaving it out makes a
//!    reader treat the first letter of the sheet name as that flag, and it then tries to
//!    decode the rest as UTF-16.
//!
//! # Two decisions
//!
//! - **RK is not used**, though `xlwt` uses it. RK packs a number into four bytes with
//!   three encodings selected by its low bits; I derived it from the bytes twice and got
//!   it wrong twice. NUMBER writes an unambiguous f64. Four more bytes per cell, no
//!   guessing.
//! - **A date is a NUMBER cell with a date format**, not a FORMULA record. `xlwt` does
//!   the same and `xlrd` reads it back as a date rather than a bare number, which is the
//!   property that matters.
//!
//! # The mini-FAT is never needed
//!
//! An OLE2 document addresses small streams through a second allocation table, which is
//! the fiddly half of the container. `xlwt` sidesteps it entirely by padding the Workbook
//! stream to exactly the 4096-byte mini-stream cutoff, and this does the same.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use qh_core::{to_text, ColumnMeta, Value};

use crate::{ExportError, ExportOptions, Writer};

/// BIFF8's row ceiling, header included. From the Python source's `XLS_MAX_ROWS`.
const MAX_ROWS: usize = 65_536;

/// BIFF8's column ceiling.
const MAX_COLUMNS: usize = 256;

/// A cell's text, which the format caps at this.
const MAX_CELL_TEXT: usize = 32_767;

/// The start of the Workbook stream's padding: the size at which a stream stops needing
/// the mini allocation table.
const MINI_STREAM_CUTOFF: usize = 4_096;

/// OLE2 sectors are 512 bytes.
const SECTOR: usize = 512;

/// A record's payload is capped here, and an SST longer than this continues.
const MAX_RECORD_DATA: usize = 8_224;

/// The serial number for 1970-01-01 in Excel's 1900 date system.
const EPOCH_SERIAL: f64 = 25_569.0;

/// BIFF8 format ids. 164 and below are reserved for built-ins, so custom formats start
/// at 164; the ids below are the ones this writer defines.
const FMT_DATE: u16 = 165;
const FMT_TIME: u16 = 166;

/// XF indexes. 0..=14 are style XFs, cell XFs start at 15.
const XF_PLAIN: u16 = 15;
const XF_DATE: u16 = 16;
const XF_TIME: u16 = 17;
const STYLE_XF_COUNT: usize = 15;

/// One buffered cell.
enum Cell {
    /// A number, with `date` selecting the date format rather than the plain one.
    Number {
        value: f64,
        style: u16,
    },
    /// An index into the shared string table.
    Text(usize),
    Bool(bool),
    Blank,
}

pub struct XlsWriter {
    path: PathBuf,
    columns: Vec<ColumnMeta>,
    sheet_name: String,
    rows: Vec<Vec<Cell>>,
    strings: Vec<String>,
    indexes: HashMap<String, usize>,
}

impl XlsWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        if columns.is_empty() {
            return Err(ExportError::Usage {
                message: "an xls sheet needs at least one column".to_owned(),
            });
        }
        if columns.len() > MAX_COLUMNS {
            return Err(ExportError::Usage {
                message: format!(
                    "xls supports {MAX_COLUMNS} columns, the query returned {}. Use xlsx or csv.",
                    columns.len()
                ),
            });
        }

        // The format caps a sheet name at 31 characters, and the Python engine takes the
        // first 31 rather than refusing.
        let sheet_name: String = options
            .sheet
            .clone()
            .unwrap_or_else(|| "Sheet1".to_owned())
            .chars()
            .take(31)
            .collect();

        let mut writer = Self {
            path: path.to_path_buf(),
            columns: columns.to_vec(),
            sheet_name,
            rows: Vec::new(),
            strings: Vec::new(),
            indexes: HashMap::new(),
        };

        if options.header {
            let names: Vec<String> = writer.columns.iter().map(|c| c.name.to_string()).collect();
            let row = names
                .into_iter()
                .map(|name| Cell::Text(writer.intern(&name)))
                .collect();
            writer.rows.push(row);
        }

        Ok(writer)
    }

    /// The table index for a string, adding it if it is new.
    ///
    /// Deduplicated, which is what makes this a *shared* string table and the reason the
    /// sheet must be buffered at all.
    fn intern(&mut self, text: &str) -> usize {
        if let Some(index) = self.indexes.get(text) {
            return *index;
        }
        let index = self.strings.len();
        self.strings.push(text.to_owned());
        self.indexes.insert(text.to_owned(), index);
        index
    }

    /// A cell's text, capped and with the characters the format cannot carry removed.
    ///
    /// The cap is applied after the filtering, which is the order the Python engine uses
    /// and the only one that yields the documented 32,767.
    fn cell_text(&self, value: &Value) -> String {
        to_text(value)
            .unwrap_or_default()
            .chars()
            .take(MAX_CELL_TEXT)
            .collect()
    }

    fn cell_for(&mut self, value: &Value) -> Cell {
        match value {
            Value::Null => Cell::Blank,
            Value::Bool(flag) => Cell::Bool(*flag),
            Value::Int(number) => Cell::Number {
                value: *number as f64,
                style: XF_PLAIN,
            },
            Value::UInt(number) => Cell::Number {
                value: *number as f64,
                style: XF_PLAIN,
            },
            Value::Float(number) => Cell::Number {
                value: *number,
                style: XF_PLAIN,
            },
            // Excel holds only doubles, so the digits beyond one are lost here and in
            // every other Excel format. The Python engine does `float(value)` too.
            Value::Decimal { unscaled, scale } => Cell::Number {
                value: *unscaled as f64 / 10f64.powi(i32::from(*scale)),
                style: XF_PLAIN,
            },
            Value::Date { days } => Cell::Number {
                value: EPOCH_SERIAL + f64::from(*days),
                style: XF_DATE,
            },
            Value::Time { micros } => Cell::Number {
                value: *micros as f64 / 86_400_000_000.0,
                style: XF_TIME,
            },
            // A cell cannot carry a zone offset, so a zone-aware instant stays text and
            // the offset survives. A naive one becomes a real date cell.
            Value::Timestamp {
                micros,
                offset_secs,
            } => match offset_secs {
                Some(_) => {
                    let text = self.cell_text(value);
                    Cell::Text(self.intern(&text))
                }
                None => {
                    let days = micros.div_euclid(86_400_000_000);
                    let within = micros.rem_euclid(86_400_000_000);
                    Cell::Number {
                        value: EPOCH_SERIAL + days as f64 + within as f64 / 86_400_000_000.0,
                        style: XF_DATE,
                    }
                }
            },
            other => {
                let text = self.cell_text(other);
                Cell::Text(self.intern(&text))
            }
        }
    }
}

impl Writer for XlsWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        if self.rows.len() >= MAX_ROWS {
            return Err(ExportError::Usage {
                message: format!(
                    "an xls sheet holds {MAX_ROWS} rows including the header. The export must \
                     split into parts; this writer does not split, because only the planner \
                     knows where the file names come from."
                ),
            });
        }
        if row.len() > MAX_COLUMNS {
            return Err(ExportError::Usage {
                message: format!(
                    "xls supports {MAX_COLUMNS} columns, the row has {}",
                    row.len()
                ),
            });
        }
        let cells: Vec<Cell> = row.iter().map(|value| self.cell_for(value)).collect();
        self.rows.push(cells);
        Ok(())
    }

    /// Write the whole document.
    ///
    /// Everything happens here because BIFF8 makes it necessary: the shared string table
    /// has to exist before the first cell record, and the container has to record the
    /// stream's size, which is not known until the last record is written.
    fn finish(&mut self) -> Result<(), ExportError> {
        let stream = self.workbook_stream()?;
        let document = ole2(&stream);
        let mut file = std::fs::File::create(&self.path)?;
        file.write_all(&document)?;
        file.flush()?;
        Ok(())
    }
}

impl XlsWriter {
    /// The Workbook stream: the globals, then the one sheet.
    fn workbook_stream(&self) -> Result<Vec<u8>, ExportError> {
        let mut globals = Vec::new();
        globals.extend_from_slice(&bof(0x0005));
        globals.extend_from_slice(&record(0x0042, &1200u16.to_le_bytes())); // codepage: UTF-16
        globals.extend_from_slice(&format_record(FMT_DATE, "YYYY-MM-DD"));
        globals.extend_from_slice(&format_record(FMT_TIME, "hh:mm:ss"));
        globals.extend_from_slice(&font_record());
        // The style XFs, then the cell XFs. Cell XFs start at index 15, which is why
        // XF_PLAIN is 15 and not 0.
        for _ in 0..STYLE_XF_COUNT {
            globals.extend_from_slice(&xf_record(0, 0, true));
        }
        globals.extend_from_slice(&xf_record(0, 0, false));
        globals.extend_from_slice(&xf_record(0, FMT_DATE, false));
        globals.extend_from_slice(&xf_record(0, FMT_TIME, false));
        globals.extend_from_slice(&style_record(0));

        // The SST is built before anything that points past it, because the sheet
        // offset recorded in BOUNDSHEET depends on the table's length -- and the table
        // can run to any number of CONTINUE records.
        let sst = sst_records(&self.strings);
        let boundsheet_len = 4 + 8 + self.sheet_name.len();
        let sheet_offset = globals.len() + boundsheet_len + sst.len() + 4;

        globals.extend_from_slice(&boundsheet_record(sheet_offset as u32, &self.sheet_name));
        globals.extend_from_slice(&sst);
        globals.extend_from_slice(&record(0x000A, &[])); // end of globals

        Ok([globals, self.sheet_stream()?].concat())
    }

    /// The one worksheet.
    fn sheet_stream(&self) -> Result<Vec<u8>, ExportError> {
        let columns = self.columns.len();
        let mut sheet = Vec::new();
        sheet.extend_from_slice(&bof(0x0010));
        sheet.extend_from_slice(&record(
            0x0200,
            &dimensions_data(self.rows.len() as u32, columns as u16),
        ));

        for (index, row) in self.rows.iter().enumerate() {
            let number = index as u16;
            for (position, cell) in row.iter().enumerate() {
                let column = position as u16;
                match cell {
                    Cell::Number { value, style } => {
                        let mut data = Vec::with_capacity(14);
                        data.extend_from_slice(&number.to_le_bytes());
                        data.extend_from_slice(&column.to_le_bytes());
                        data.extend_from_slice(&style.to_le_bytes());
                        data.extend_from_slice(&value.to_le_bytes());
                        sheet.extend_from_slice(&record(0x0203, &data));
                    }
                    Cell::Text(index) => {
                        let mut data = Vec::with_capacity(10);
                        data.extend_from_slice(&number.to_le_bytes());
                        data.extend_from_slice(&column.to_le_bytes());
                        data.extend_from_slice(&XF_PLAIN.to_le_bytes());
                        data.extend_from_slice(&(*index as u32).to_le_bytes());
                        sheet.extend_from_slice(&record(0x00FD, &data));
                    }
                    Cell::Bool(flag) => {
                        let mut data = Vec::with_capacity(10);
                        data.extend_from_slice(&number.to_le_bytes());
                        data.extend_from_slice(&column.to_le_bytes());
                        data.extend_from_slice(&XF_PLAIN.to_le_bytes());
                        data.push(u8::from(*flag));
                        data.push(0); // not an error value
                        sheet.extend_from_slice(&record(0x0205, &data));
                    }
                    // A blank cell is recorded rather than skipped: the Python engine's
                    // writer emits one, and a reader that counts cells sees the column
                    // present in the row.
                    Cell::Blank => {
                        let mut data = Vec::with_capacity(6);
                        data.extend_from_slice(&number.to_le_bytes());
                        data.extend_from_slice(&column.to_le_bytes());
                        data.extend_from_slice(&XF_PLAIN.to_le_bytes());
                        sheet.extend_from_slice(&record(0x0201, &data));
                    }
                }
            }
        }
        sheet.extend_from_slice(&record(0x000A, &[]));
        Ok(sheet)
    }
}

// ------------------------------------------------------------------- BIFF8 records

/// A record: type, length, payload.
fn record(record_type: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&record_type.to_le_bytes());
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Beginning of file. Type 5 is the workbook globals, 16 is a worksheet.
fn bof(kind: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    for field in [0x0600u16, kind, 0x0DBB, 0x07CC] {
        data.extend_from_slice(&field.to_le_bytes());
    }
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0x0006u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    record(0x0809, &data)
}

/// A number format. Ids 164 and up are the custom ones.
fn format_record(id: u16, code: &str) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&id.to_le_bytes());
    data.extend_from_slice(&(code.len() as u16).to_le_bytes());
    data.push(0x00); // compressed, which is what an ASCII format code is
    data.extend_from_slice(code.as_bytes());
    record(0x041E, &data)
}

/// The one font, Arial 10.
fn font_record() -> Vec<u8> {
    let name = b"Arial";
    let mut data = Vec::with_capacity(21);
    data.extend_from_slice(&200u16.to_le_bytes()); // height, in twentieths of a point
    data.extend_from_slice(&0u16.to_le_bytes()); // attributes
    data.extend_from_slice(&0x7FFFu16.to_le_bytes()); // colour: automatic
    data.extend_from_slice(&400u16.to_le_bytes()); // weight: normal
    data.extend_from_slice(&0u16.to_le_bytes()); // superscript or subscript
    data.push(0); // underline
    data.push(0); // family
    data.push(1); // charset
    data.push(0); // reserved
    data.push(name.len() as u8);
    data.push(0); // compressed
    data.extend_from_slice(name);
    record(0x0031, &data)
}

/// A cell format: which font, which number format, and how to align.
///
/// The layout is `xlwt`'s own documented one, and two fields are worth explaining.
/// A style XF sets `fStyle` and puts 0x0FFF in the parent field; a cell XF does neither
/// and inherits from style XF 0.
fn xf_record(font: u16, format: u16, style: bool) -> Vec<u8> {
    let parent: u16 = if style { 0x0FFF } else { 0x0000 };
    // Bit 2 marks a style XF, bit 0 marks the cell as locked.
    let protection: u16 = if style { 0x0005 } else { 0x0001 };
    let mut data = Vec::with_capacity(20);
    data.extend_from_slice(&font.to_le_bytes());
    data.extend_from_slice(&format.to_le_bytes());
    data.extend_from_slice(&((parent << 4) | protection).to_le_bytes());
    data.push(0x20); // bottom vertical alignment, horizontal general
    data.push(0); // rotation
    data.push(0); // indent
                  // Which attributes override the parent. A style XF leaves the number format to the
                  // parent; a cell XF overrides everything it was given.
    data.push(if style { 0xF4 } else { 0xF8 });
    data.extend_from_slice(&[0u8; 4]); // border line styles
    data.extend_from_slice(&[0u8; 4]); // border colours and fill pattern
    data.extend_from_slice(&0x20C0u16.to_le_bytes()); // pattern and background colours
    debug_assert_eq!(data.len(), 20);
    record(0x00E0, &data)
}

/// The built-in "Normal" style, which every workbook needs.
///
/// Bit 15 must be set: with it clear a reader takes the record for a user-defined style
/// and looks for a name string that is not there. That defect was found by reading the
/// output back with xlrd, and the failure was an index error inside the reader.
fn style_record(xf_index: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&(0x8000 | xf_index).to_le_bytes());
    data.push(0x00); // built-in id 0: Normal
    data.push(0xFF); // not a row or column outline level
    record(0x0293, &data)
}

/// Where the sheet's stream begins, and what it is called.
fn boundsheet_record(offset: u32, name: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(8 + name.len());
    data.extend_from_slice(&offset.to_le_bytes());
    data.push(0); // not hidden
    data.push(0); // worksheet
    data.push(name.len() as u8);
    // The name flags byte. Easy to leave out, and without it a reader takes the first
    // letter of the name as the flag and then tries to decode the rest as UTF-16.
    data.push(0x00); // compressed
    data.extend_from_slice(name.as_bytes());
    record(0x0085, &data)
}

fn dimensions_data(rows: u32, columns: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(14);
    data.extend_from_slice(&0u32.to_le_bytes()); // first row
    data.extend_from_slice(&rows.to_le_bytes()); // one past the last row
    data.extend_from_slice(&0u16.to_le_bytes()); // first column
    data.extend_from_slice(&columns.to_le_bytes()); // one past the last column
    data.extend_from_slice(&0u16.to_le_bytes()); // reserved
    data
}

/// The shared string table, as the SST record plus however many CONTINUEs it needs.
///
/// The rule that makes this fiddly, and it is the format's rather than one reader's: a
/// record's payload stops at 8224 bytes, and when the boundary falls **inside a string's
/// characters** the continuation must start by repeating that string's option byte, so
/// the reader knows whether the remaining characters are 8-bit or 16-bit. A split
/// between strings needs no such marker. xlrd implements exactly this, which is how the
/// rule was confirmed rather than assumed.
fn sst_records(strings: &[String]) -> Vec<u8> {
    let mut chunks: Vec<Vec<u8>> = vec![sst_header(strings.len())];

    for text in strings {
        let compressed = text.chars().all(|character| (character as u32) < 256);
        let options: u8 = if compressed { 0x00 } else { 0x01 };
        let unit = if compressed { 1 } else { 2 };
        let characters: Vec<u8> = if compressed {
            text.chars().map(|character| character as u8).collect()
        } else {
            text.encode_utf16()
                .flat_map(|unit| unit.to_le_bytes())
                .collect()
        };

        // The count, then the option byte, then the characters. The header must not
        // straddle a boundary, so a new record starts if it would not fit whole.
        if chunks.last().expect("at least one chunk").len() + 3 + unit > MAX_RECORD_DATA {
            chunks.push(Vec::new());
        }
        let count = text.chars().count() as u16;
        let chunk = chunks.last_mut().expect("at least one chunk");
        chunk.extend_from_slice(&count.to_le_bytes());
        chunk.push(options);

        let mut index = 0usize;
        while index < characters.len() {
            let chunk = chunks.last_mut().expect("at least one chunk");
            let space = MAX_RECORD_DATA - chunk.len();
            // Rounded down to a whole unit, because a split inside a 16-bit character
            // would corrupt it.
            let take = space.min(characters.len() - index) / unit * unit;
            if take == 0 {
                // Not even one whole unit fits, so the continuation begins with the
                // option byte repeated.
                chunks.push(vec![options]);
                continue;
            }
            let chunk = chunks.last_mut().expect("at least one chunk");
            chunk.extend_from_slice(&characters[index..index + take]);
            index += take;
            if index < characters.len() {
                chunks.push(vec![options]);
            }
        }
    }

    let mut out = Vec::new();
    for (position, chunk) in chunks.iter().enumerate() {
        // The first carries the table, the rest continue it.
        let record_type = if position == 0 { 0x00FC } else { 0x003C };
        out.extend_from_slice(&record(record_type, chunk));
    }
    out
}

/// The SST's own header: how many strings were written, and how many are distinct.
fn sst_header(count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    out
}

// ----------------------------------------------------------------- OLE2 container

const FREESECT: u32 = 0xFFFF_FFFF;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FATSECT: u32 = 0xFFFF_FFFD;

/// Wrap a Workbook stream in the compound document a reader expects.
fn ole2(stream: &[u8]) -> Vec<u8> {
    // Padded to the mini-stream cutoff, which is what lets the mini allocation table be
    // skipped entirely: a stream of this size is addressed by the main one.
    let mut stream = stream.to_vec();
    if stream.len() < MINI_STREAM_CUTOFF {
        stream.resize(MINI_STREAM_CUTOFF, 0);
    }
    if stream.len() % SECTOR != 0 {
        let padded = stream.len().div_ceil(SECTOR) * SECTOR;
        stream.resize(padded, 0);
    }

    let sectors = stream.len() / SECTOR;
    let fat_sector = sectors;
    let directory_sector = sectors + 1;

    // The stream's chain, then the allocation table's own sector, then the directory's.
    let mut entries: Vec<u32> = (0..sectors).map(|i| (i + 1) as u32).collect();
    if let Some(last) = entries.last_mut() {
        *last = ENDOFCHAIN;
    }
    entries.push(FATSECT);
    entries.push(ENDOFCHAIN);
    entries.resize(128, FREESECT);
    let mut fat = Vec::with_capacity(512);
    for entry in &entries {
        fat.extend_from_slice(&entry.to_le_bytes());
    }

    let mut header = Vec::with_capacity(512);
    header.extend_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    header.extend_from_slice(&[0u8; 16]); // class id
    header.extend_from_slice(&0x003Eu16.to_le_bytes()); // minor version
    header.extend_from_slice(&3u16.to_le_bytes()); // major version: 512-byte sectors
    header.extend_from_slice(&0xFFFEu16.to_le_bytes()); // little endian
    header.extend_from_slice(&9u16.to_le_bytes()); // sector size as a power of two
    header.extend_from_slice(&6u16.to_le_bytes()); // mini sector size, same
    header.extend_from_slice(&[0u8; 6]); // reserved
    header.extend_from_slice(&0u32.to_le_bytes()); // directory sectors, zero for version 3
    header.extend_from_slice(&1u32.to_le_bytes()); // one allocation table sector
    header.extend_from_slice(&(directory_sector as u32).to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // transaction signature
    header.extend_from_slice(&(MINI_STREAM_CUTOFF as u32).to_le_bytes());
    header.extend_from_slice(&ENDOFCHAIN.to_le_bytes()); // no mini allocation table
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&ENDOFCHAIN.to_le_bytes()); // no further table sectors
    header.extend_from_slice(&0u32.to_le_bytes());
    // The table of table sectors: the first entry names the allocation sector, and the
    // rest say there are no more.
    for index in 0..109 {
        let entry = if index == 0 {
            fat_sector as u32
        } else {
            FREESECT
        };
        header.extend_from_slice(&entry.to_le_bytes());
    }
    debug_assert_eq!(header.len(), 512);

    let mut directory = Vec::with_capacity(512);
    // The root's child is the stream below it; a stream has no children.
    // No siblings for either entry. Zero would mean "entry zero", which for the root is
    // itself, and a reader walking the directory as a tree then recurses until it gives
    // up. FREESECT is the value that means "there is none".
    directory.extend_from_slice(&directory_entry(
        "Root Entry",
        5,
        FREESECT,
        FREESECT,
        1,
        ENDOFCHAIN,
        0,
    ));
    directory.extend_from_slice(&directory_entry(
        "Workbook",
        2,
        FREESECT,
        FREESECT,
        FREESECT,
        0,
        stream.len(),
    ));
    directory.resize(SECTOR, 0);

    [header, stream, fat, directory].concat()
}

/// One 128-byte directory entry.
fn directory_entry(
    name: &str,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    let encoded: Vec<u8> = name
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .chain([0, 0])
        .collect();
    out.extend_from_slice(&encoded);
    out.resize(64, 0);
    out.extend_from_slice(&(encoded.len() as u16).to_le_bytes());
    out.push(kind);
    out.push(1); // black, which is what a one-entry tree wants
    out.extend_from_slice(&left.to_le_bytes());
    out.extend_from_slice(&right.to_le_bytes());
    out.extend_from_slice(&child.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]); // class id
    out.extend_from_slice(&0u32.to_le_bytes()); // state bits
    out.extend_from_slice(&[0u8; 16]); // created and modified times
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&(size as u64).to_le_bytes());
    out.resize(128, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("name", "varchar"),
            ColumnMeta::new("when", "date"),
            ColumnMeta::new("amount", "double"),
        ]
    }

    fn path_for(label: &str) -> PathBuf {
        // Unique per call. A fixed name had two tests truncating each other's file in
        // the other writers, and the symptom was a panic a long way from the cause.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-xls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join(format!("{label}-{unique}.xls"))
    }

    fn write(path: &Path, options: &ExportOptions, rows: &[Vec<Value>]) {
        let mut writer = XlsWriter::new(path, &columns(), options).expect("open");
        for row in rows {
            writer.write_row(row).expect("write_row");
        }
        writer.finish().expect("finish");
    }

    /// A small reader for the container, so the tests check the file the way a consumer
    /// does instead of trusting the writer's own arithmetic.
    fn workbook_stream(bytes: &[u8]) -> Vec<u8> {
        assert_eq!(
            &bytes[..8],
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
            "the compound document signature"
        );
        let sector = 1usize << u16::from_le_bytes([bytes[30], bytes[31]]);
        let directory_sector =
            u32::from_le_bytes([bytes[48], bytes[49], bytes[50], bytes[51]]) as usize;
        let fat_sector = u32::from_le_bytes([bytes[76], bytes[77], bytes[78], bytes[79]]) as usize;

        let sector_at =
            |index: usize| &bytes[sector + index * sector..sector + (index + 1) * sector];
        let mut fat = Vec::new();
        for entry in sector_at(fat_sector).chunks(4) {
            fat.push(u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]));
        }

        // Directory entry 1 is the stream; entry 0 is the root storage.
        let directory = sector_at(directory_sector);
        let entry = &directory[128..256];
        let start = u32::from_le_bytes([entry[116], entry[117], entry[118], entry[119]]) as usize;
        let size = u32::from_le_bytes([entry[120], entry[121], entry[122], entry[123]]) as usize;

        let mut stream = Vec::with_capacity(size);
        let mut current = start;
        while current != 0xFFFF_FFFE && stream.len() < size {
            stream.extend_from_slice(sector_at(current));
            current = fat[current] as usize;
        }
        stream.truncate(size);
        stream
    }

    /// Every record in a stream, as (type, payload).
    fn records(stream: &[u8]) -> Vec<(u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset + 4 <= stream.len() {
            let kind = u16::from_le_bytes([stream[offset], stream[offset + 1]]);
            let length = u16::from_le_bytes([stream[offset + 2], stream[offset + 3]]) as usize;
            // The stream is zero-padded to the mini-stream cutoff. A zero type with a
            // zero length is padding, not a record -- and stopping at the first EOF
            // instead would stop at the end of the globals and never see the sheet.
            if kind == 0 && length == 0 {
                break;
            }
            if offset + 4 + length > stream.len() {
                break;
            }
            let payload = stream[offset + 4..offset + 4 + length].to_vec();
            out.push((kind, payload));
            offset += 4 + length;
        }
        out
    }

    /// Decode an SST record plus its continuations the way a reader must: one logical
    /// stream, where a continuation that begins inside a string starts with that
    /// string's option byte.
    fn read_sst(chunks: &[Vec<u8>]) -> Vec<String> {
        let mut strings = Vec::new();
        let count = u32::from_le_bytes([chunks[0][0], chunks[0][1], chunks[0][2], chunks[0][3]]);
        let mut index = 0usize;
        let mut position = 8usize;
        while strings.len() < count as usize {
            let chunk = &chunks[index];
            let characters = u16::from_le_bytes([chunk[position], chunk[position + 1]]) as usize;
            let options = chunk[position + 2];
            position += 3;
            let compressed = options & 0x01 == 0;
            let mut collected = String::new();
            let mut got = 0usize;
            loop {
                let chunk = &chunks[index];
                let unit = if compressed { 1 } else { 2 };
                let needed = characters - got;
                let available = std::cmp::min((chunk.len() - position) / unit, needed);
                let bytes = &chunk[position..position + available * unit];
                if compressed {
                    collected.extend(bytes.iter().map(|byte| char::from(*byte)));
                } else {
                    let units: Vec<u16> = bytes
                        .chunks(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect();
                    if let Ok(text) = String::from_utf16(&units) {
                        collected.push_str(&text);
                    }
                }
                position += available * unit;
                got += available;
                if got == characters {
                    break;
                }
                index += 1;
                position = 1;
                // The rule under test: a continuation that resumes inside a string
                // repeats the option byte.
                let chunk = &chunks[index];
                assert_eq!(
                    chunk[0], options,
                    "the continuation repeats the option byte"
                );
            }
            strings.push(collected);
        }
        strings
    }

    #[test]
    fn the_container_is_a_compound_document_a_reader_can_walk() {
        let path = path_for("container");
        write(&path, &ExportOptions::default(), &[]);
        let bytes = std::fs::read(&path).expect("read back");

        assert_eq!(
            &bytes[..8],
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
            "OLE2 signature",
        );
        assert_eq!(
            u16::from_le_bytes([bytes[24], bytes[25]]),
            0x003E,
            "minor version"
        );
        assert_eq!(
            u16::from_le_bytes([bytes[26], bytes[27]]),
            3,
            "major version"
        );
        assert_eq!(
            u16::from_le_bytes([bytes[28], bytes[29]]),
            0xFFFE,
            "little endian"
        );
        assert_eq!(
            u16::from_le_bytes([bytes[30], bytes[31]]),
            9,
            "512-byte sectors"
        );
        assert_eq!(
            u16::from_le_bytes([bytes[32], bytes[33]]),
            6,
            "64-byte mini sectors"
        );
        let cutoff = u32::from_le_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]);
        assert_eq!(cutoff, MINI_STREAM_CUTOFF as u32);
        // The stream is padded past the cutoff, which is what makes the mini allocation
        // table unnecessary. The header must agree that there is none.
        assert_eq!(
            u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]),
            ENDOFCHAIN
        );
        assert_eq!(
            u32::from_le_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]),
            ENDOFCHAIN
        );

        // The directory's two entries, by name.
        let directory_sector =
            u32::from_le_bytes([bytes[48], bytes[49], bytes[50], bytes[51]]) as usize;
        let directory = &bytes[512 + directory_sector * 512..512 + (directory_sector + 1) * 512];
        let name_of = |entry: &[u8]| {
            let length = u16::from_le_bytes([entry[64], entry[65]]) as usize;
            String::from_utf16(
                &entry[..length - 2]
                    .chunks(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<u16>>(),
            )
            .expect("the directory names are ASCII")
        };
        assert_eq!(name_of(&directory[0..128]), "Root Entry");
        assert_eq!(name_of(&directory[128..256]), "Workbook");
        assert_eq!(directory[66], 5, "the root is a storage");
        assert_eq!(directory[128 + 66], 2, "the workbook is a stream");
        assert_eq!(
            u32::from_le_bytes([directory[76], directory[77], directory[78], directory[79]]),
            1,
            "the root's child is the workbook",
        );
        // No siblings on either entry. Zero would mean entry zero, which for the root is
        // itself and sends a reader that walks the directory as a tree into infinite
        // recursion -- which is exactly what happened before this was fixed.
        for offset in [68, 72] {
            assert_eq!(
                u32::from_le_bytes([
                    directory[offset],
                    directory[offset + 1],
                    directory[offset + 2],
                    directory[offset + 3]
                ]),
                FREESECT,
                "the root's sibling pointer",
            );
        }
        assert_eq!(
            u32::from_le_bytes([
                directory[128 + 76],
                directory[128 + 77],
                directory[128 + 78],
                directory[128 + 79]
            ]),
            FREESECT,
            "the workbook has no children",
        );

        // The file is exactly the header, the stream's sectors, the table and the
        // directory, with nothing left over.
        let sectors = (bytes.len() - 512) / 512;
        assert_eq!(
            u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]),
            0,
            "version 3"
        );
        assert_eq!(
            u32::from_le_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]),
            1,
            "one FAT sector"
        );
        assert_eq!(bytes.len(), 512 + sectors * 512);
    }

    #[test]
    fn the_globals_carry_what_a_reader_looks_for_before_the_sheet() {
        let path = path_for("globals");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Int(1),
                Value::Text("ada".into()),
                Value::Date { days: 20_484 },
                Value::Float(1.5),
            ]],
        );
        let stream = workbook_stream(&std::fs::read(&path).expect("read back"));
        let all = records(&stream);

        // The globals begin with a BOF of type 5, and the codepage is UTF-16.
        assert_eq!(all[0].0, 0x0809, "BOF");
        assert_eq!(
            u16::from_le_bytes([all[0].1[2], all[0].1[3]]),
            0x0005,
            "workbook globals"
        );
        let codepage = all
            .iter()
            .find(|(kind, _)| *kind == 0x0042)
            .expect("codepage");
        assert_eq!(u16::from_le_bytes([codepage.1[0], codepage.1[1]]), 1200);

        // A font, the style XFs, three cell XFs and the built-in Normal style.
        assert_eq!(
            all.iter().filter(|(kind, _)| *kind == 0x0031).count(),
            1,
            "one font"
        );
        assert_eq!(
            all.iter().filter(|(kind, _)| *kind == 0x00E0).count(),
            STYLE_XF_COUNT + 3,
            "fifteen style XFs and three cell XFs",
        );
        let style = all.iter().find(|(kind, _)| *kind == 0x0293).expect("style");
        let first = u16::from_le_bytes([style.1[0], style.1[1]]);
        assert_eq!(first & 0x8000, 0x8000, "the built-in bit must be set");
        assert_eq!(first & 0x0FFF, 0, "pointing at style XF 0");

        // The boundsheet names the sheet and points at a real offset in the stream.
        let boundsheet = all
            .iter()
            .find(|(kind, _)| *kind == 0x0085)
            .expect("boundsheet");
        let offset = u32::from_le_bytes([
            boundsheet.1[0],
            boundsheet.1[1],
            boundsheet.1[2],
            boundsheet.1[3],
        ]) as usize;
        assert_eq!(boundsheet.1[6], 6, "name length");
        assert_eq!(&boundsheet.1[8..14], b"Sheet1");
        assert_eq!(
            &stream[offset..offset + 2],
            &[0x09, 0x08],
            "the sheet's BOF"
        );
        assert_eq!(
            u16::from_le_bytes([stream[offset + 2], stream[offset + 3]]),
            16,
            "a worksheet",
        );

        // The SST is the last thing before the globals end.
        assert_eq!(all.iter().filter(|(kind, _)| *kind == 0x00FC).count(), 1);
    }

    #[test]
    fn a_date_is_a_number_cell_wearing_the_date_format() {
        // Worth pinning because it is why no FORMULA record is needed: a date is an
        // ordinary number whose XF carries the date format, and a reader that honours
        // the format shows a date rather than 46053.
        let path = path_for("date");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Int(7),
                Value::Null,
                Value::Date { days: 20_484 },
                Value::Null,
            ]],
        );
        let stream = workbook_stream(&std::fs::read(&path).expect("read back"));
        let all = records(&stream);

        let number = all
            .iter()
            .find(|(kind, data)| *kind == 0x0203 && data[2..4] == 2u16.to_le_bytes())
            .expect("a number cell in column two");
        // The XF must be the date one, or the cell displays as a number.
        assert_eq!(u16::from_le_bytes([number.1[4], number.1[5]]), XF_DATE);
        let value = f64::from_le_bytes(number.1[6..14].try_into().expect("eight bytes"));
        assert_eq!(value, EPOCH_SERIAL + 20_484.0);
        assert_eq!(value, 46_053.0, "2026-01-31 in Excel's 1900 system");

        // And the row's plain number uses the plain format, not the date one.
        let plain = all
            .iter()
            .find(|(kind, data)| *kind == 0x0203 && data[2..4] == 0u16.to_le_bytes())
            .expect("a number cell in column zero");
        assert_eq!(u16::from_le_bytes([plain.1[4], plain.1[5]]), XF_PLAIN);

        // A null is written as a blank record rather than skipped.
        assert!(all.iter().any(|(kind, _)| *kind == 0x0201), "a blank cell");
    }

    #[test]
    fn a_zone_aware_timestamp_stays_text_with_its_offset() {
        let path = path_for("zoned");
        write(
            &path,
            &ExportOptions::default(),
            &[vec![
                Value::Null,
                Value::Null,
                Value::Timestamp {
                    micros: 1_769_860_800_000_000,
                    offset_secs: Some(25_200),
                },
                Value::Null,
            ]],
        );
        let stream = workbook_stream(&std::fs::read(&path).expect("read back"));
        let all = records(&stream);
        let sst: Vec<Vec<u8>> = all
            .iter()
            .filter(|(kind, _)| *kind == 0x00FC || *kind == 0x003C)
            .map(|(_, data)| data.clone())
            .collect();
        let strings = read_sst(&sst);
        assert!(
            strings.iter().any(|text| text.contains("+07:00")),
            "the offset must survive: {strings:?}",
        );
        // No number cell in column two, because the cell is text.
        assert!(!all
            .iter()
            .any(|(kind, data)| *kind == 0x0203 && data[2..4] == 2u16.to_le_bytes()));
    }

    #[test]
    fn the_string_table_round_trips_through_its_own_encoding() {
        let cases = vec![
            "id".to_owned(),
            "ada".to_owned(),
            // Below 256 throughout, so it should take the compressed path.
            "café".to_owned(),
            // The euro sign forces the whole string to sixteen bits.
            "café €—".to_owned(),
            String::new(),
            "  padded  ".to_owned(),
        ];
        let bytes = sst_records(&cases);
        let chunks: Vec<Vec<u8>> = records(&bytes).into_iter().map(|(_, data)| data).collect();
        assert_eq!(chunks.len(), 1, "a small table needs no continuation");
        assert_eq!(read_sst(&chunks), cases);
    }

    #[test]
    fn a_long_table_continues_and_a_string_can_straddle_the_boundary() {
        // The path that is easiest to get wrong, so it is tested with the cases that
        // reach it: enough strings to overflow the record, and one string alone that is
        // larger than a whole record.
        let mut cases: Vec<String> = (0..400).map(|i| format!("row-{i}")).collect();
        cases.push("x".repeat(9_000)); // larger than one record's payload
        cases.push("€".repeat(5_000)); // sixteen bits per character, so also larger

        let bytes = sst_records(&cases);
        let chunks: Vec<Vec<u8>> = records(&bytes).into_iter().map(|(_, data)| data).collect();

        assert!(
            chunks.len() > 2,
            "the table must have continued: {} records",
            chunks.len()
        );
        // Every record honours the payload cap.
        for chunk in &chunks {
            assert!(
                chunk.len() <= MAX_RECORD_DATA,
                "a record overflowed: {}",
                chunk.len()
            );
        }

        // And the whole table reads back, which is what the option-byte rule is for. If
        // a continuation failed to repeat it, this disagrees.
        assert_eq!(read_sst(&chunks), cases);
    }

    #[test]
    fn a_sixteen_bit_string_is_never_cut_inside_a_character() {
        // A continuation must not split a code unit, or the reader decodes garbage.
        let cases: Vec<String> = vec!["€".repeat(6_000)];
        let chunks: Vec<Vec<u8>> = records(&sst_records(&cases))
            .into_iter()
            .map(|(_, data)| data)
            .collect();
        assert!(chunks.len() >= 2);
        // The first chunk holds the header plus whole characters: the character bytes
        // are (its length - 8 byte table header - 3 byte string header) and that must be
        // even.
        let characters = chunks[0].len() - 8 - 3;
        assert_eq!(
            characters % 2,
            0,
            "an odd number of bytes means a split code unit"
        );
        assert_eq!(read_sst(&chunks), cases);
    }

    #[test]
    fn the_row_and_column_ceilings_are_refused_rather_than_written() {
        let path = path_for("limits");
        let options = ExportOptions::default();
        let mut writer = XlsWriter::new(&path, &columns(), &options).expect("open");
        // 256 columns is the format's limit, and the writer takes the header's place.
        let row = vec![Value::Null; 300];
        let error = writer
            .write_row(&row)
            .expect_err("300 columns must be refused");
        assert!(matches!(error, ExportError::Usage { .. }));
    }

    #[test]
    fn the_sheet_name_is_capped_rather_than_refused() {
        // The Python engine takes the first 31 characters, and the BOUNDSHEET length is
        // a single byte, so a longer name would otherwise be truncated by a cast.
        let path = path_for("name");
        let options = ExportOptions {
            sheet: Some("a".repeat(60)),
            ..ExportOptions::default()
        };
        write(&path, &options, &[]);
        let stream = workbook_stream(&std::fs::read(&path).expect("read back"));
        let boundsheet = records(&stream)
            .into_iter()
            .find(|(kind, _)| *kind == 0x0085)
            .expect("boundsheet");
        assert_eq!(boundsheet.1[6], 31, "the name length written");
        assert_eq!(&boundsheet.1[8..39], "a".repeat(31).as_bytes());
    }

    #[test]
    fn a_large_table_with_straddling_strings_is_left_on_disk_for_a_real_reader() {
        // The continuation path, through an actual file rather than only through the
        // encoder: short distinct strings to overflow the record, and two strings far
        // larger than one record so that a continuation must resume inside a string.
        // The file is deliberately not deleted -- the external check reads it with xlrd,
        // which is the verification for this format.
        let path = path_for("large");
        let mut writer =
            XlsWriter::new(&path, &columns(), &ExportOptions::default()).expect("open");
        let compressed = "x".repeat(9_000);
        let wide = "\u{20ac}".repeat(5_000);
        for index in 0..300u32 {
            writer
                .write_row(&[
                    Value::Int(i64::from(index)),
                    Value::Text(format!("row-{index}").into()),
                    Value::Text(compressed.clone().into()),
                    Value::Text(wide.clone().into()),
                ])
                .expect("write_row");
        }
        writer.finish().expect("finish");

        // And it still parses as records, so a structural defect shows up here rather
        // than only in the external reader.
        let stream = workbook_stream(&std::fs::read(&path).expect("read back"));
        let all = records(&stream);
        let chunks: Vec<Vec<u8>> = all
            .iter()
            .filter(|(kind, _)| *kind == 0x00FC || *kind == 0x003C)
            .map(|(_, data)| data.clone())
            .collect();
        assert!(chunks.len() > 1, "the table must have needed continuations");
        let strings = read_sst(&chunks);
        assert_eq!(
            strings.len(),
            306,
            "four column names, three hundred distinct row labels, and two long strings",
        );
        assert!(strings.iter().any(|text| text.len() == 9_000));
    }
}
