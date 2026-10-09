//! Encrypted spill: AES-256-GCM records, one per chunk.
//!
//! One random key per registry (per process in the app), nonces from a
//! per-key counter starting at 1, AAD binding each record to its store and
//! chunk index. The record plaintext is an Arrow IPC stream plus the chunk
//! flags; files are `0600`, unlinked before the first byte is written, and
//! read with `pread` (no mmap: this crate forbids `unsafe`).
//!
//! `ring` does not zeroize the AES schedule inside `LessSafeKey` on drop.
//! That is accepted and stated here: the key lives as long as the process,
//! and no claim is made about scrubbing it from memory.

use std::fs::File;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::SecureRandom;

use crate::StoreError;

/// Tag domain for app spill records. Helper-operator records use `QHD1`
/// under their own key; the domain keeps the two apart even if a key ever
/// served both.
const DOMAIN_APP: [u8; 4] = *b"QHS2";
/// Tag domain for the analytics helper's operator spill records (§14.8). The
/// helper's own key and counter seal under it; no key is shared with the app, and
/// the tag keeps a helper record from authenticating as an app record even if
/// one key ever served both.
pub const DOMAIN_HELPER: [u8; 4] = *b"QHD1";
/// Record magic at plaintext offset 0.
const MAGIC: &[u8; 4] = b"QHP1";
/// Header: magic + IPC length + flags length, padded so the IPC buffer
/// starts at 64 and stays 16-byte aligned for zero-copy decode.
const HEADER_LEN: usize = 64;

/// AES-256-GCM with a per-registry key and a counter nonce. One per
/// registry; the helper builds its own and keys never cross processes.
pub struct SpillCipher {
    domain: [u8; 4],
    key: LessSafeKey,
    counter: AtomicU64,
}

impl std::fmt::Debug for SpillCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpillCipher")
            .field("domain", &String::from_utf8_lossy(&self.domain))
            .field("key", &"<redacted>")
            .finish()
    }
}

impl SpillCipher {
    /// A fresh random key under the app tag domain. Fails closed: on RNG
    /// failure spill stays off rather than falling back to a fixed key.
    pub fn new() -> Result<Self, StoreError> {
        Self::with_domain(DOMAIN_APP)
    }

    /// A fresh random key under the helper tag domain ([`DOMAIN_HELPER`]), for the
    /// analytics helper's operator spill.
    pub fn new_helper() -> Result<Self, StoreError> {
        Self::with_domain(DOMAIN_HELPER)
    }

    /// A fresh random key under a caller-chosen tag domain. The helper uses
    /// `*b"QHD1"` with its own process key (§14.8), which is why the domain
    /// is a parameter rather than a constant.
    pub fn with_domain(domain: [u8; 4]) -> Result<Self, StoreError> {
        let mut seed = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| StoreError::SpillUnavailable {
                reason: "the system random number generator failed".to_owned(),
            })?;
        let key = Self::from_seed(domain, &seed);
        zeroize::Zeroize::zeroize(&mut seed);
        Ok(key)
    }

    /// A fixed-key cipher for tests that craft records by hand. `#[doc(hidden)]`
    /// because a deterministic key has no business outside a test.
    #[doc(hidden)]
    pub fn for_test(seed_byte: u8) -> Self {
        Self::for_test_with_domain(seed_byte, DOMAIN_APP)
    }

    #[doc(hidden)]
    pub fn for_test_with_domain(seed_byte: u8, domain: [u8; 4]) -> Self {
        Self::from_seed(domain, &[seed_byte; 32])
    }

    fn from_seed(domain: [u8; 4], seed: &[u8; 32]) -> Self {
        // `AES_256_GCM` only refuses a key of the wrong length, and this one is
        // a literal 32 bytes, so the `expect` cannot fire. `ring` has no
        // fallible constructor that also takes ownership.
        let unbound = UnboundKey::new(&AES_256_GCM, seed)
            .expect("a 32-byte key is the right length for AES-256-GCM");
        Self {
            domain,
            key: LessSafeKey::new(unbound),
            counter: AtomicU64::new(1),
        }
    }

    /// Seal one record. Returns the nonce counter and `ciphertext || tag`.
    /// The nonce is never reused under this key: the counter only rises,
    /// and exhaustion refuses rather than wraps.
    pub fn seal(
        &self,
        store_id: u64,
        chunk_index: u32,
        plaintext: &mut Vec<u8>,
    ) -> Result<(u64, Vec<u8>), StoreError> {
        // Claim the counter before sealing, and refuse *without* wrapping
        // when it is exhausted. A `fetch_add` would roll to 0 and hand the
        // next caller a nonce that has already been used.
        //
        // `fetch_update` is renamed `try_update` in Rust 1.95, but the workspace MSRV is 1.85, so
        // the new name is not available yet. Silence the deprecation until the MSRV moves.
        #[allow(deprecated)]
        let counter = self
            .counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                if current == u64::MAX {
                    None
                } else {
                    Some(current + 1)
                }
            })
            .map_err(|_| StoreError::SpillUnavailable {
                reason: "nonce space exhausted".to_owned(),
            })?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes(counter));
        let tag = self
            .key
            .seal_in_place_separate_tag(
                nonce,
                Aad::from(aad(self.domain, store_id, chunk_index)),
                plaintext,
            )
            .map_err(|_| StoreError::SpillUnavailable {
                reason: "sealing failed".to_owned(),
            })?;
        let mut record = std::mem::take(plaintext);
        record.extend_from_slice(tag.as_ref());
        Ok((counter, record))
    }

    /// Open one record in place, returning the plaintext length (the tag
    /// stays at the buffer's end and the caller truncates it away).
    pub fn open(
        &self,
        store_id: u64,
        chunk_index: u32,
        nonce: u64,
        record: &mut [u8],
    ) -> Result<usize, StoreError> {
        let nonce = Nonce::assume_unique_for_key(nonce_bytes(nonce));
        self.key
            .open_in_place(
                nonce,
                Aad::from(aad(self.domain, store_id, chunk_index)),
                record,
            )
            .map(|plain| plain.len())
            .map_err(|_| StoreError::SpillAuth { chunk: chunk_index })
    }
}

/// Counter to a 12-byte nonce: four zero bytes, then big-endian counter.
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// `domain || store_id LE || chunk_index LE`: 16 bytes of AAD.
fn aad(domain: [u8; 4], store_id: u64, chunk_index: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&domain);
    out[4..12].copy_from_slice(&store_id.to_le_bytes());
    out[12..].copy_from_slice(&chunk_index.to_le_bytes());
    out
}

/// Serialize one chunk to record plaintext: `QHP1 || I || F || zeros ||
/// IPC stream || flags`. The schema rides in every record because the
/// physical schema may differ per chunk (D-2).
pub fn encode_record(
    batch: &arrow_array::RecordBatch,
    flags: &crate::chunk::ChunkFlags,
) -> Result<Vec<u8>, StoreError> {
    let mut ipc = Vec::new();
    {
        let mut writer = arrow_ipc::writer::StreamWriter::try_new(&mut ipc, &batch.schema())
            .map_err(|error| StoreError::Internal {
                detail: error.to_string(),
            })?;
        writer.write(batch).map_err(|error| StoreError::Internal {
            detail: error.to_string(),
        })?;
        writer.finish().map_err(|error| StoreError::Internal {
            detail: error.to_string(),
        })?;
    }
    let flag_bytes = encode_flags(flags, batch.num_rows());
    let mut buf = vec![0u8; HEADER_LEN];
    buf[..4].copy_from_slice(MAGIC);
    buf[4..8].copy_from_slice(&(ipc.len() as u32).to_le_bytes());
    buf[8..12].copy_from_slice(&(flag_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(&ipc);
    buf.extend_from_slice(&flag_bytes);
    Ok(buf)
}

fn encode_flags(flags: &crate::chunk::ChunkFlags, rows: usize) -> Vec<u8> {
    let width = flags.openable.len();
    let words = rows.div_ceil(8);
    let mut out = Vec::with_capacity(width + 2 * width * words);
    for column in 0..width {
        let mut present = 0u8;
        if flags.openable[column].is_some() {
            present |= 0b01;
        }
        if flags.numeric[column].is_some() {
            present |= 0b10;
        }
        out.push(present);
    }
    for column in 0..width {
        for bitmap in flags.openable[column]
            .iter()
            .chain(flags.numeric[column].iter())
        {
            out.extend_from_slice(bitmap.inner());
        }
    }
    out
}

/// Parse record plaintext back. The IPC decode runs with validation on;
/// unauthenticated bytes never reach it because `open` runs first.
pub fn decode_record(plain: &[u8]) -> Result<DecodedRecord, StoreError> {
    fn corrupt(detail: impl Into<String>) -> StoreError {
        StoreError::Corrupt {
            detail: detail.into(),
        }
    }
    let header = plain
        .get(..HEADER_LEN)
        .ok_or_else(|| corrupt("record shorter than header"))?;
    if header[..4] != MAGIC[..] {
        return Err(corrupt("record magic mismatch"));
    }
    let ipc_len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let flag_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let ipc = plain
        .get(HEADER_LEN..HEADER_LEN + ipc_len)
        .ok_or_else(|| corrupt("record shorter than its IPC length"))?;
    let flag_bytes = plain
        .get(HEADER_LEN + ipc_len..)
        .ok_or_else(|| corrupt("record shorter than its IPC length"))?;
    if flag_bytes.len() != flag_len {
        return Err(corrupt("flag length disagrees with the record"));
    }
    // Arrow's own decoder panics on some malformed IPC bodies (a buffer
    // offset past its length, most often). The record has already passed
    // GCM, so this is the D-20 case: a helper's output is untrusted even
    // when it authenticated. Catch the panic and report `Corrupt` rather
    // than aborting the process.
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut reader = arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(ipc), None)?;
        let batch = reader.next().ok_or_else(|| {
            arrow_schema::ArrowError::ParseError("record holds no batch".into())
        })??;
        if reader.next().is_some() {
            return Err(arrow_schema::ArrowError::ParseError(
                "record holds more than one batch".into(),
            ));
        }
        Ok::<_, arrow_schema::ArrowError>(batch)
    }));
    let batch = match decoded {
        Ok(Ok(batch)) => batch,
        Ok(Err(error)) => return Err(corrupt(error.to_string())),
        Err(_) => return Err(corrupt("the IPC decoder panicked on this record")),
    };
    let flags = decode_flags(flag_bytes, batch.num_columns(), batch.num_rows())?;
    for field in batch.schema().fields() {
        let enc =
            qh_columnar::encoding::encoding_of(field).map_err(|error| StoreError::Corrupt {
                detail: error.to_string(),
            })?;
        if !qh_columnar::encoding::check_compatible(enc, field.data_type()) {
            return Err(StoreError::Corrupt {
                detail: format!("encoding {} does not fit {}", enc, field.data_type()),
            });
        }
    }
    let encodings: Vec<qh_columnar::Encoding> = batch
        .schema()
        .fields()
        .iter()
        .map(|field| qh_columnar::encoding::encoding_of(field))
        .collect::<Result<_, _>>()
        .map_err(|error| StoreError::Corrupt {
            detail: error.to_string(),
        })?;
    Ok(DecodedRecord {
        batch,
        flags,
        encodings,
    })
}
/// A parsed spill record: the batch, its flags, and its encodings.
#[derive(Debug)]
pub struct DecodedRecord {
    pub batch: arrow_array::RecordBatch,
    pub flags: crate::chunk::ChunkFlags,
    pub encodings: Vec<qh_columnar::Encoding>,
}

fn decode_flags(
    bytes: &[u8],
    width: usize,
    rows: usize,
) -> Result<crate::chunk::ChunkFlags, StoreError> {
    fn corrupt(detail: impl Into<String>) -> StoreError {
        StoreError::Corrupt {
            detail: detail.into(),
        }
    }
    let words = rows.div_ceil(8);
    let (presence, mut rest) = bytes
        .split_at_checked(width)
        .ok_or_else(|| corrupt("flags truncated"))?;
    let mut flags = crate::chunk::ChunkFlags::empty(width);
    for (column, &present) in presence.iter().enumerate() {
        if present & !0b11 != 0 {
            return Err(corrupt("unknown flag presence bits"));
        }
        if present & 0b01 != 0 {
            let (bitmap, tail) = rest
                .split_at_checked(words)
                .ok_or_else(|| corrupt("bitmap truncated"))?;
            flags.openable[column] = Some(arrow_buffer::BooleanBuffer::new(bitmap.into(), 0, rows));
            rest = tail;
        }
        if present & 0b10 != 0 {
            let (bitmap, tail) = rest
                .split_at_checked(words)
                .ok_or_else(|| corrupt("bitmap truncated"))?;
            flags.numeric[column] = Some(arrow_buffer::BooleanBuffer::new(bitmap.into(), 0, rows));
            rest = tail;
        }
    }
    if !rest.is_empty() {
        return Err(corrupt("trailing bytes after flags"));
    }
    Ok(flags)
}

/// Byte storage behind one store's spill records. `pread`/`pwrite` only.
pub trait SpillMedium: Send + Sync {
    fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()>;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
    /// The file's own metadata, for the mode test. Defaults to an error so a
    /// non-file medium need not invent one.
    fn metadata(&self) -> io::Result<std::fs::Metadata> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// The production medium: one unlinked `0600` file per spilled store.
pub struct FileMedium {
    file: File,
}
impl SpillMedium for FileMedium {
    fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        self.file.write_all_at(data, offset)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.file.read_exact_at(buf, offset)
    }
    fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.file.metadata()
    }
}
/// One store's spill file: created `0600` with `create_new` (no clobber,
/// symlinks refused), then unlinked before the first byte is written, so
/// the directory is always empty and the blocks die with the last fd.
pub struct SpillFile {
    medium: Box<dyn SpillMedium>,
    next_offset: u64,
}

impl SpillFile {
    pub fn create(dir: &Path, pid: u32, store_id: u64) -> Result<Self, StoreError> {
        Self::create_named(dir, "qhs", pid, store_id)
    }

    /// The same, with the file name's prefix chosen by the owner: `qhs` for a store
    /// (`qhs-<pid>-<store_id>-<16 hex>.spill`), `qhd` for the analytics helper's
    /// operator files. The name only matters during the instant before the unlink,
    /// and to anyone listing a directory after a crash.
    pub fn create_named(dir: &Path, prefix: &str, pid: u32, id: u64) -> Result<Self, StoreError> {
        let mut random = [0u8; 8];
        ring::rand::SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| StoreError::SpillUnavailable {
                reason: "no randomness for spill name".to_owned(),
            })?;
        let mut name = format!("{prefix}-{pid}-{id}-");
        for byte in random {
            name.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
            name.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
        }
        let path = dir.join(format!("{name}.spill"));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(map_io)?;
        // Unlink before the first byte: `ENOENT` (another instance's sweep
        // won the race) is success. Any other unlink failure closes the fd
        // and disables spill — an un-unlinked spill file must never run.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                drop(file);
                return Err(map_io(error));
            }
        }
        Ok(Self {
            medium: Box::new(FileMedium { file }),
            next_offset: 0,
        })
    }

    /// A spill file over an injected medium, for the disk-full test. No
    /// directory is touched: the medium owns all the bytes.
    #[doc(hidden)]
    pub fn with_medium(medium: Box<dyn SpillMedium>) -> Self {
        Self {
            medium,
            next_offset: 0,
        }
    }

    /// Bytes appended so far (the file only grows; a released store drops the whole file).
    pub fn written_bytes(&self) -> u64 {
        self.next_offset
    }

    /// Append a sealed record; returns its offset and on-disk length.
    pub fn append(&mut self, record: &[u8]) -> Result<(u64, u32), StoreError> {
        let offset = self.next_offset;
        self.medium.write_at(offset, record).map_err(map_io)?;
        self.next_offset += record.len() as u64;
        Ok((offset, record.len() as u32))
    }

    pub fn read(&self, offset: u64, len: u32) -> Result<Vec<u8>, StoreError> {
        let mut buf = vec![0u8; len as usize];
        self.medium.read_at(offset, &mut buf).map_err(map_io)?;
        Ok(buf)
    }

    /// The file's own mode and length, from `fstat` on the still-open fd.
    ///
    /// The path is gone by the time this is useful, which is the point: the
    /// test that checks `0600` has to read the mode off the descriptor, and
    /// it has to be able to read it *after* the unlink.
    #[doc(hidden)]
    pub fn metadata_for_test(&self) -> io::Result<std::fs::Metadata> {
        self.medium.metadata()
    }

    #[doc(hidden)]
    pub fn read_raw_for_test(&self, offset: u64, len: u32) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len as usize];
        self.medium.read_at(offset, &mut buf)?;
        Ok(buf)
    }

    /// Every byte written so far, for the "no plaintext on disk" test.
    #[doc(hidden)]
    pub fn read_all_raw_for_test(&self) -> io::Result<Vec<u8>> {
        if self.next_offset == 0 {
            return Ok(Vec::new());
        }
        self.read_raw_for_test(0, self.next_offset as u32)
    }
}

/// Map I/O failures: full disks become `DiskFull`, everything else `Io`.
/// A full disk keeps chunks resident; the triggering run fails with a
/// message, and no other store is affected.
fn map_io(error: io::Error) -> StoreError {
    if error.kind() == io::ErrorKind::StorageFull || error.raw_os_error() == Some(28) {
        StoreError::DiskFull
    } else {
        StoreError::Io(error)
    }
}
/// A medium that fails writes with `ENOSPC`: the disk-full injection.
#[doc(hidden)]
pub struct FaultyMedium;
#[doc(hidden)]
impl SpillMedium for FaultyMedium {
    fn write_at(&self, _o: u64, _d: &[u8]) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(28))
    }
    fn read_at(&self, _o: u64, _b: &mut [u8]) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

/// What the startup sweep did. Logged by the host.
#[derive(Debug, Clone)]
pub struct SweepReport {
    pub removed: u32,
    pub spill_enabled: bool,
    pub reason: Option<String>,
}
/// Prepare the spill directory and sweep orphans. Symlinks are never
/// followed and recursion never happens: plain files and symlinks are
/// removed, subdirectories are left alone.
pub fn sweep_spill_dir(dir: &Path) -> SweepReport {
    let disabled = |reason: String| SweepReport {
        removed: 0,
        spill_enabled: false,
        reason: Some(reason),
    };
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return disabled("spill path is a symlink".into())
        }
        Ok(meta) if !meta.is_dir() => return disabled("spill path is not a directory".into()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return disabled(format!("cannot stat the spill directory: {error}")),
    }
    if let Err(error) = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
    {
        return disabled(format!("cannot create the spill directory: {error}"));
    }
    if let Err(error) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
        return disabled(format!("cannot secure the spill directory: {error}"));
    }
    let mut removed = 0u32;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return disabled(format!("cannot read the spill directory: {error}")),
    };
    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true);
        if is_dir {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    SweepReport {
        removed,
        spill_enabled: true,
        reason: None,
    }
}
