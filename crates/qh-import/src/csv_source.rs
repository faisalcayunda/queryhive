//! The CSV source: a real stream, one record at a time.
//!
//! `csv::Reader` over the file handle. `has_headers(false)` because the header is
//! read here, once, so it can be named before a data row is asked for —
//! `has_headers(true)` would hide the first row from the record loop without
//! handing it back in a form this crate can carry.
//!
//! `flexible(true)` so that a row whose field count differs from the header is *handed
//! back* rather than made a parse failure here: whether it is a rejected row, an
//! allowed trailing empty extra or an opted-in short row is the importer's call (PF-4),
//! made where the `ON_ERROR` policy lives.
//!
//! # Encoding
//!
//! The whole file is scanned **before** the first row is read, which is before the first
//! write: a file that is not valid in its declared encoding is refused up front, with the
//! line and byte, instead of failing at row 40 000 of a run that has no transaction to
//! roll back (`ON_ERROR=skip`, Trino). UTF-8 is the default and is never guessed away: a
//! Windows-1252 file read as UTF-8 would not fail everywhere, it would write mojibake, so
//! the caller names the code page (`ENCODING=cp1252`) and the refusal says so.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use qh_export::Codec;

use crate::{ImportError, Options, RawRow};

/// Counts the bytes that leave the file, for progress by bytes.
struct Counted<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.count.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// The high half of a single-byte code page, derived from the one table the export
/// writers already carry (`qh_export::Codec::encode`) so the two directions cannot drift.
/// `None` is a byte the code page leaves undefined.
fn high_half(codec: Codec) -> &'static [Option<char>; 128] {
    static TABLES: [OnceLock<[Option<char>; 128]>; 6] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    let slot = Codec::ALL
        .iter()
        .position(|candidate| *candidate == codec)
        .expect("every codec is in ALL");
    TABLES[slot].get_or_init(|| {
        let mut table = [None; 128];
        if codec != Codec::Ascii {
            for code in 0x80_u32..=0xFFFF {
                let Some(character) = char::from_u32(code) else {
                    continue;
                };
                let mut buffer = [0_u8; 4];
                if let [byte @ 0x80..=0xFF] = codec.encode(character.encode_utf8(&mut buffer))[..] {
                    table[usize::from(byte - 0x80)].get_or_insert(character);
                }
            }
        }
        table
    })
}

/// Turns a single-byte code page into UTF-8 as it is read.
struct Transcoder<R> {
    inner: R,
    table: &'static [Option<char>; 128],
    raw: Vec<u8>,
    out: Vec<u8>,
    at: usize,
}

impl<R: Read> Read for Transcoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at == self.out.len() {
            self.raw.resize(8 * 1024, 0);
            let read = self.inner.read(&mut self.raw)?;
            self.out.clear();
            self.at = 0;
            for &byte in &self.raw[..read] {
                if byte < 0x80 {
                    self.out.push(byte);
                } else if let Some(character) = self.table[usize::from(byte - 0x80)] {
                    let mut buffer = [0_u8; 4];
                    self.out
                        .extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                } else {
                    // The scan refused this file already; a file that changed under it
                    // is an error, not a replacement character.
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("byte 0x{byte:02X} is undefined in the file's code page"),
                    ));
                }
            }
            if read == 0 {
                return Ok(0);
            }
        }
        let take = buf.len().min(self.out.len() - self.at);
        buf[..take].copy_from_slice(&self.out[self.at..self.at + take]);
        self.at += take;
        Ok(take)
    }
}

/// Read the whole file once and refuse it if it is not valid in `codec`.
fn scan(path: &Path, codec: Codec) -> Result<(), ImportError> {
    let refuse = |line: u64, offset: u64, message: String| ImportError::Encoding {
        path: path.display().to_string(),
        line: line as usize,
        offset: offset as usize,
        message,
    };
    let mut file = std::fs::File::open(path).map_err(|error| ImportError::io(path, error))?;
    let table = (!codec.is_utf8()).then(|| high_half(codec));
    // A table with no hole cannot refuse anything (ISO-8859-1).
    if table.is_some_and(|table| table.iter().all(Option::is_some)) {
        return Ok(());
    }
    let mut buffer = vec![0_u8; 64 * 1024];
    let (mut carried, mut offset, mut line) = (0_usize, 0_u64, 1_u64);
    loop {
        let read = match file.read(&mut buffer[carried..]) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ImportError::io(path, error)),
        };
        let end = carried + read;
        let newlines = |bytes: &[u8]| bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
        if let Some(table) = table {
            if let Some(bad) = buffer[..end]
                .iter()
                .position(|byte| *byte >= 0x80 && table[usize::from(byte - 0x80)].is_none())
            {
                return Err(refuse(
                    line + newlines(&buffer[..bad]),
                    offset + bad as u64,
                    format!(
                        "byte 0x{:02X} is not a character in {}; the file is in another code \
                         page, or is binary",
                        buffer[bad],
                        codec.label()
                    ),
                ));
            }
            if read == 0 {
                return Ok(());
            }
            line += newlines(&buffer[..end]);
            offset += end as u64;
            continue;
        }
        match std::str::from_utf8(&buffer[..end]) {
            Ok(_) => {
                line += newlines(&buffer[..end]);
                offset += end as u64;
                carried = 0;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                // Cut mid-character at the end of the buffer: carry it to the next read.
                if error.error_len().is_none() && read > 0 {
                    line += newlines(&buffer[..valid]);
                    offset += valid as u64;
                    buffer.copy_within(valid..end, 0);
                    carried = end - valid;
                } else {
                    return Err(refuse(
                        line + newlines(&buffer[..valid]),
                        offset + valid as u64,
                        "not valid UTF-8; if this is Windows-1252 or Latin-1 text, import it \
                         with ENCODING=cp1252 (or latin-1)"
                            .to_owned(),
                    ));
                }
            }
        }
        if read == 0 {
            return Ok(());
        }
    }
}

/// An open CSV file, or an in-memory CSV for a test.
pub struct CsvSource {
    reader: csv::Reader<Box<dyn Read + Send>>,
    header: Option<Vec<String>>,
    path: String,
    want_header: bool,
    /// Bytes read from the file so far; `None` for a source over text in hand.
    bytes: Option<Arc<AtomicU64>>,
}

impl CsvSource {
    /// Open `path` as CSV.
    pub fn open(path: &Path, options: &Options) -> Result<Self, ImportError> {
        let codec = options.encoding.unwrap_or(Codec::Utf8);
        scan(path, codec)?;
        let file = std::fs::File::open(path).map_err(|error| ImportError::io(path, error))?;
        let count = Arc::new(AtomicU64::new(0));
        let counted = Counted {
            inner: file,
            count: Arc::clone(&count),
        };
        let reader: Box<dyn Read + Send> = if codec.is_utf8() {
            Box::new(counted)
        } else {
            Box::new(Transcoder {
                inner: counted,
                table: high_half(codec),
                raw: Vec::new(),
                out: Vec::new(),
                at: 0,
            })
        };
        let mut source = Self::build(reader, path.display().to_string(), options);
        source.bytes = Some(count);
        source.detect_header()?;
        Ok(source)
    }

    /// A source over text already in hand, for a test.
    pub fn from_text(text: String, options: &Options) -> Self {
        let mut source = Self::build(
            Box::new(std::io::Cursor::new(text.into_bytes())),
            String::from("<memory>"),
            options,
        );
        let _ = source.detect_header();
        source
    }

    fn build(reader: Box<dyn Read + Send>, path: String, options: &Options) -> Self {
        let mut builder = csv::ReaderBuilder::new();
        builder.has_headers(false).flexible(true);
        if let Some(delimiter) = options.delimiter {
            // `csv` takes a byte. A delimiter that is not one ASCII byte is
            // refused by the caller before this is reached.
            builder.delimiter(delimiter as u8);
        }
        Self {
            reader: builder.from_reader(reader),
            header: None,
            path,
            want_header: options.header,
            bytes: None,
        }
    }

    /// Read the first record as the header, when the caller asked for one. The
    /// header is consumed here so `next_row` never sees it.
    fn detect_header(&mut self) -> Result<(), ImportError> {
        if !self.want_header {
            return Ok(());
        }
        let mut record = csv::StringRecord::new();
        let read = self
            .reader
            .read_record(&mut record)
            .map_err(|error| ImportError::Csv {
                path: self.path.clone(),
                source: error,
            })?;
        if read {
            self.header = Some(record.iter().map(str::to_owned).collect());
        }
        Ok(())
    }

    /// The header row, when the file has one and it has been read.
    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref().filter(|header| !header.is_empty())
    }

    /// How many bytes of the file have been read (the reader buffers a little ahead).
    pub fn bytes_read(&self) -> Option<u64> {
        self.bytes
            .as_ref()
            .map(|count| count.load(Ordering::Relaxed))
    }

    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError> {
        let mut record = csv::StringRecord::new();
        let read = self
            .reader
            .read_record(&mut record)
            .map_err(|error| ImportError::Csv {
                path: self.path.clone(),
                source: error,
            })?;
        if !read {
            return Ok(None);
        }
        let line = record
            .position()
            .map(|position| position.line() as usize)
            .unwrap_or(0);
        let cells = record.iter().map(str::to_owned).collect();
        Ok(Some(RawRow {
            line,
            cells,
            numeric: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("qh-csv-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write the fixture");
        path
    }

    fn rows(path: &Path, encoding: Option<Codec>) -> Result<Vec<Vec<String>>, ImportError> {
        let options = Options {
            header: true,
            encoding,
            ..Options::default()
        };
        let mut source = CsvSource::open(path, &options)?;
        let mut rows = Vec::new();
        while let Some(row) = source.next_row()? {
            rows.push(row.cells);
        }
        Ok(rows)
    }

    #[test]
    fn the_decoding_tables_are_the_export_tables_read_backwards() {
        let cp1252 = high_half(Codec::Cp1252);
        assert_eq!(cp1252[0x00], Some('\u{20ac}'));
        assert_eq!(cp1252[0x69], Some('\u{e9}'));
        assert_eq!(cp1252[0x01], None, "0x81 is undefined in cp1252");
        assert!(high_half(Codec::Latin1).iter().all(Option::is_some));
        assert!(high_half(Codec::Ascii).iter().all(Option::is_none));
    }

    #[test]
    fn invalid_utf8_is_refused_before_a_row_is_read_and_names_the_line() {
        let path = file("latin.csv", b"a,b\n1,caf\xe9\n2,ok\n");
        let error = rows(&path, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("line 2"), "{message}");
        assert!(message.contains("byte offset 9"), "{message}");
        assert!(message.contains("ENCODING=cp1252"), "{message}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_cp1252_file_is_read_when_the_caller_says_so() {
        let path = file("cp1252.csv", b"a,b\n1,caf\xe9 \x80\n");
        assert_eq!(
            rows(&path, Some(Codec::Cp1252)).unwrap(),
            vec![vec!["1".to_owned(), "caf\u{e9} \u{20ac}".to_owned()]]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_byte_cp1252_leaves_undefined_is_refused() {
        let path = file("hole.csv", b"a\n\x81\n");
        let message = rows(&path, Some(Codec::Cp1252)).unwrap_err().to_string();
        assert!(
            message.contains("0x81") && message.contains("line 2"),
            "{message}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_character_split_across_the_scan_buffer_is_not_an_error() {
        // 64 KiB of ASCII, then a 3-byte character that straddles the buffer edge.
        let mut bytes = b"a\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 64 * 1024 - 3));
        bytes.extend("\u{20ac}\n".as_bytes());
        let path = file("straddle.csv", &bytes);
        assert_eq!(rows(&path, None).unwrap().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bytes_read_follows_the_file() {
        let path = file("bytes.csv", b"a\n1\n2\n");
        let mut source = CsvSource::open(
            &path,
            &Options {
                header: true,
                ..Options::default()
            },
        )
        .unwrap();
        while source.next_row().unwrap().is_some() {}
        assert_eq!(source.bytes_read(), Some(6));
        let _ = std::fs::remove_file(&path);
    }
}
