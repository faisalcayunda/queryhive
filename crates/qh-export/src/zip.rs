//! The ZIP container, for the two places this crate needs one.
//!
//! Two callers, two trade-offs, one writer:
//!
//! - **`xlsx`** files each of its parts **stored** (method 0). The parts are XML the
//!   writer is still streaming as it goes, and one of them — the sheet — has no known
//!   size when its local header must be written, which is what flag bit 3 and a data
//!   descriptor are for.
//! - **`bundle`** puts a multi-part export into one download the way the Python
//!   engine did: `zipfile.ZipFile(..., "w", ZIP_DEFLATED, compresslevel=6)`. Its
//!   entries are whole files on disk, compressed as they are copied, so a 500 MB CSV
//!   never lands in memory.
//!
//! The archive's *bytes* are not compared with Python's `zipfile` for the bundle, and
//! cannot be: zlib and this deflate implementation are free to choose different
//! back-references, and both are correct. What is claimed is an archive whose
//! contents — names, bytes, CRCs — are the files that went in, verified by reading it
//! back with a reader that did not write it.
//!
//! Everything here is `pub(crate)`: the crate's public surface is
//! [`crate::bundle`], not a ZIP library.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::ExportError;

/// The fixed DOS timestamp.
///
/// 1980-01-01, the ZIP epoch. Recorded rather than read from the clock on purpose: the
/// container's timestamps say nothing the caller asked for, and a fixed value makes the
/// output reproducible, so a test can compare whole files instead of guessing which
/// bytes are allowed to differ.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021; // 1980-01-01

/// Stored: the bytes go in as they are.
const METHOD_STORED: u16 = 0;
/// Deflated: raw deflate, no zlib header, which is what the format specifies.
const METHOD_DEFLATE: u16 = 8;

/// The compression level Python's `bundle` asked zlib for.
const DEFLATE_LEVEL: u32 = 6;

/// A ZIP archive being written.
pub(crate) struct Zip {
    out: BufWriter<File>,
    offset: u64,
    entries: Vec<CentralEntry>,
}

struct CentralEntry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
    streamed: bool,
}

impl Zip {
    pub(crate) fn create(path: &Path) -> Result<Self, ExportError> {
        Ok(Self {
            out: BufWriter::new(File::create(path)?),
            offset: 0,
            entries: Vec::new(),
        })
    }

    pub(crate) fn write_data(&mut self, bytes: &[u8]) -> Result<(), ExportError> {
        self.out.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(())
    }

    /// Write an entry whose CRC and size are already known.
    pub(crate) fn add_stored(&mut self, name: &str, bytes: &[u8]) -> Result<(), ExportError> {
        let crc = Crc32::of(bytes);
        let offset = self.offset;
        let header = local_header(name, METHOD_STORED, crc, bytes.len() as u64, false)?;
        self.write_data(&header)?;
        self.write_data(bytes)?;
        self.entries.push(CentralEntry {
            name: name.to_owned(),
            method: METHOD_STORED,
            crc,
            compressed: bytes.len() as u64,
            size: bytes.len() as u64,
            offset,
            streamed: false,
        });
        Ok(())
    }

    /// Start an entry whose CRC and size are not known yet.
    ///
    /// Returns its index, which [`Self::end_streamed`] needs.
    pub(crate) fn begin_streamed(&mut self, name: &str) -> Result<usize, ExportError> {
        self.begin(name, METHOD_STORED)
    }

    fn begin(&mut self, name: &str, method: u16) -> Result<usize, ExportError> {
        let offset = self.offset;
        // Zeroes for the CRC and the sizes, and flag bit 3 to say so. A reader that
        // ignores the flag would see a broken entry, which is why the descriptor below
        // is written even though the central directory also carries the values.
        let header = local_header(name, method, 0, 0, true)?;
        self.write_data(&header)?;
        self.entries.push(CentralEntry {
            name: name.to_owned(),
            method,
            crc: 0,
            compressed: 0,
            size: 0,
            offset,
            streamed: true,
        });
        Ok(self.entries.len() - 1)
    }

    /// Close a streamed entry: the data descriptor, then the recorded values.
    pub(crate) fn end_streamed(
        &mut self,
        index: usize,
        crc: u32,
        compressed: u64,
        uncompressed: u64,
        name: &str,
    ) -> Result<(), ExportError> {
        let descriptor = data_descriptor(crc, compressed, uncompressed);
        self.write_data(&descriptor)?;
        let entry = self
            .entries
            .get_mut(index)
            .ok_or_else(|| ExportError::Usage {
                message: format!("internal: no zip entry at {index} for {name}"),
            })?;
        entry.crc = crc;
        entry.compressed = compressed;
        entry.size = uncompressed;
        Ok(())
    }

    /// Copy one file in, deflated, streaming it rather than holding it in memory.
    pub(crate) fn add_file(&mut self, name: &str, path: &Path) -> Result<(), ExportError> {
        let index = self.begin(name, METHOD_DEFLATE)?;
        let mut reader = BufReader::new(File::open(path)?);
        let mut crc = Crc32::new();
        let mut uncompressed = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];

        // Counted rather than measured: the deflate stream's length is only known once
        // it is finished, and the entry's header has already declared that it will be
        // stated in the descriptor below.
        let mut counter = Counted::new(&mut self.out);
        let mut encoder = DeflateEncoder::new(&mut counter, Compression::new(DEFLATE_LEVEL));
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            crc.update(&buffer[..read]);
            uncompressed += read as u64;
            encoder.write_all(&buffer[..read])?;
        }
        // Finishing writes the last deflate block; the returned writer is not needed.
        encoder.finish()?;
        let compressed = counter.written;

        self.offset += compressed;
        self.end_streamed(index, crc.value(), compressed, uncompressed, name)
    }

    /// The central directory and the end-of-directory record, which is what a reader
    /// navigates by.
    pub(crate) fn finish(&mut self) -> Result<(), ExportError> {
        let directory_start = self.offset;
        let entries = std::mem::take(&mut self.entries);
        for entry in &entries {
            let header = central_header(entry)?;
            self.write_data(&header)?;
        }
        let directory_size = self.offset - directory_start;

        let mut end = Vec::new();
        end.extend_from_slice(&0x06054b50u32.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes()); // this disk
        end.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        end.extend_from_slice(&(directory_size as u32).to_le_bytes());
        end.extend_from_slice(&(directory_start as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes()); // comment length
        self.write_data(&end)?;

        self.out.flush()?;
        Ok(())
    }
}

/// A writer that remembers how much went through it.
struct Counted<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Counted<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }
}

impl<W: Write> Write for Counted<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Zip `files` into one archive, for a download of a split export.
///
/// The archive holds the files **under their own names**, not under a path: a part is
/// `report_part01.csv` in the archive as it is on disk, which is what a person who
/// double-clicks the download expects to get. An archive of one file is still written
/// as an archive — the caller asked for one.
///
/// A file that cannot be read is an error, not a partial archive: half a bundle looks
/// exactly like a complete one until it is opened.
pub fn bundle(files: &[PathBuf], zip_path: &Path) -> Result<PathBuf, ExportError> {
    let mut zip = Zip::create(zip_path)?;
    for path in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| ExportError::Usage {
                message: format!("cannot bundle {}: it has no file name", path.display()),
            })?;
        zip.add_file(&name, path)?;
    }
    zip.finish()?;
    Ok(zip_path.to_path_buf())
}

fn local_header(
    name: &str,
    method: u16,
    crc: u32,
    size: u64,
    streamed: bool,
) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::with_capacity(30 + name.len());
    let flags: u16 = if streamed { 0x0008 } else { 0 };
    out.extend_from_slice(&0x04034b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&method.to_le_bytes());
    out.extend_from_slice(&DOS_TIME.to_le_bytes());
    out.extend_from_slice(&DOS_DATE.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    let size = if streamed { 0 } else { size };
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra length
    out.extend_from_slice(name.as_bytes());
    Ok(out)
}

fn data_descriptor(crc: u32, compressed: u64, uncompressed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&0x08074b50u32.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(compressed as u32).to_le_bytes());
    out.extend_from_slice(&(uncompressed as u32).to_le_bytes());
    out
}

fn central_header(entry: &CentralEntry) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::with_capacity(46 + entry.name.len());
    let flags: u16 = if entry.streamed { 0x0008 } else { 0 };
    out.extend_from_slice(&0x02014b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version made by
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&entry.method.to_le_bytes());
    out.extend_from_slice(&DOS_TIME.to_le_bytes());
    out.extend_from_slice(&DOS_DATE.to_le_bytes());
    out.extend_from_slice(&entry.crc.to_le_bytes());
    out.extend_from_slice(&(entry.compressed as u32).to_le_bytes());
    out.extend_from_slice(&(entry.size as u32).to_le_bytes());
    out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
    out.extend_from_slice(&(entry.offset as u32).to_le_bytes());
    out.extend_from_slice(entry.name.as_bytes());
    Ok(out)
}

/// CRC-32 as ZIP defines it: polynomial `0xEDB88320`, reflected, initial and final
/// value all ones.
pub(crate) struct Crc32 {
    state: u32,
}

impl Crc32 {
    pub(crate) fn new() -> Self {
        Self { state: 0xffff_ffff }
    }

    pub(crate) fn of(bytes: &[u8]) -> u32 {
        let mut crc = Self::new();
        crc.update(bytes);
        crc.value()
    }

    /// Bitwise rather than table-driven: a bundle's parts are read in 64 KB chunks and
    /// a 256-entry table would be a constant to keep correct for no measurable gain.
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(self.state & 1);
                self.state = (self.state >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }

    pub(crate) fn value(&self) -> u32 {
        !self.state
    }
}
