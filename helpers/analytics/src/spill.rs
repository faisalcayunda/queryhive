//! The helper's own spill: where DataFusion's sort, aggregation and join operators put
//! what does not fit the lease, encrypted (blueprint section 14.8, NFR-S3).
//!
//! * **Key.** A [`SpillCipher`] in the `QHD1` domain with a key of 32 random bytes made
//!   when the helper starts. It never leaves this process and is gone when the process
//!   is. The app's key is never sent here, and this process never reads the app's spill.
//!   `ring` does not scrub the AES schedule inside the key on drop; the key lives as long
//!   as the process, and no claim is made about wiping it from memory.
//! * **Files.** `qhd-<pid>-<file_id>-<16 hex>.spill` in the helper's spill directory,
//!   created `0600` with `create_new` and unlinked before the first byte is written, so
//!   the directory is empty at all times and the blocks die with the last file
//!   descriptor. The directory itself is made `0700` and swept at start by the same
//!   rules as the app's (section 9.7).
//! * **Records.** A writer buffers plaintext up to 1 MiB, or until a flush, and seals it as
//!   one record, with AAD `QHD1 | file_id (u64 LE) | record_index (u32 LE)`. A record is replayed or
//!   reordered only by an attacker who can write the file, and the AAD makes that fail
//!   authentication. The nonce comes from the cipher's counter and is kept in memory.
//!   DataFusion's spill pool reads a file while it is still being written and flushes
//!   after every batch to say "readable now", so a flush seals what is buffered.
//! * **Reading.** One record at a time, `pread`, opened in place, and handed to DataFusion
//!   as plaintext `Bytes`. DataFusion decodes its own spill without validating it, which
//!   is safe here because bytes that fail authentication are never handed out: the read
//!   fails with [`SpillError::Auth`] first.
//! * **No fallback.** If the directory is not safe (a symlink, not a directory, not
//!   creatable) there is no spill at all ([`SpillSetup::Disabled`]), and an operator that
//!   needs it fails with the budget message. The OS temp directory is never used.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use bytes::Bytes;
use datafusion::common::DataFusionError;
use datafusion::execution::{SpillFile, SpillWriter, TempFileFactory};
use futures::Stream;
use qh_result_store::{sweep_spill_dir, SpillCipher, SpillFile as BackingFile, StoreError};

/// The raw on-disk bytes of files, collected by the test hook.
pub type RawBytes = Arc<Mutex<Vec<Vec<u8>>>>;

/// Plaintext per record: the writer seals when it holds this much.
pub const RECORD_PLAINTEXT: usize = 1 << 20;

/// Why a spill operation failed. Travels inside a `DataFusionError` and is found again by
/// [`crate::session::classify`], which reports it as `ErrorKind::Spill`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpillError {
    /// A record failed authentication: damaged, swapped, replayed, or the wrong key.
    #[error("a spill record failed authentication")]
    Auth,
    /// The disk has no room for the spill.
    #[error("the disk has no room for the analytics spill")]
    DiskFull,
    /// The spill is over its quota.
    #[error("the analytics spill is over its quota")]
    Quota,
    /// Anything else the file system said.
    #[error("the analytics spill failed: {0}")]
    Io(String),
}

impl From<StoreError> for SpillError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::DiskFull => SpillError::DiskFull,
            StoreError::SpillAuth { .. } => SpillError::Auth,
            other => SpillError::Io(other.to_string()),
        }
    }
}

impl From<SpillError> for DataFusionError {
    fn from(error: SpillError) -> Self {
        DataFusionError::External(Box::new(error))
    }
}

impl From<SpillError> for std::io::Error {
    fn from(error: SpillError) -> Self {
        std::io::Error::other(error)
    }
}

/// What the spill directory turned out to be.
pub enum SpillSetup {
    Enabled(Arc<EncryptedTempFiles>),
    /// No spill. `reason` is for the log; the user is told the budget message.
    Disabled {
        reason: String,
    },
}

/// What has been spilled so far, for tests and the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpillStats {
    pub files: u64,
    /// Ciphertext bytes written, over the life of the process.
    pub bytes: u64,
}

/// The factory DataFusion asks for a spill file whenever an operator needs one.
pub struct EncryptedTempFiles {
    dir: PathBuf,
    cipher: Arc<SpillCipher>,
    next_file: AtomicU64,
    /// Ciphertext bytes on disk now, over every live file.
    used: Arc<AtomicU64>,
    quota: u64,
    files: AtomicU64,
    written: Arc<AtomicU64>,
    /// Test hook: every file, when it is dropped, deposits its raw on-disk bytes here.
    capture: Mutex<Option<RawBytes>>,
}

impl EncryptedTempFiles {
    /// Prepare `dir` and make a key. `quota` caps the ciphertext on disk at any moment.
    pub fn open(dir: &Path, quota: u64) -> SpillSetup {
        let report = sweep_spill_dir(dir);
        if !report.spill_enabled {
            return SpillSetup::Disabled {
                reason: report
                    .reason
                    .unwrap_or_else(|| "the spill directory is not safe".to_owned()),
            };
        }
        match SpillCipher::new_helper() {
            Ok(cipher) => SpillSetup::Enabled(Arc::new(Self {
                dir: dir.to_owned(),
                cipher: Arc::new(cipher),
                next_file: AtomicU64::new(1),
                used: Arc::new(AtomicU64::new(0)),
                quota,
                files: AtomicU64::new(0),
                written: Arc::new(AtomicU64::new(0)),
                capture: Mutex::new(None),
            })),
            Err(error) => SpillSetup::Disabled {
                reason: error.to_string(),
            },
        }
    }

    pub fn stats(&self) -> SpillStats {
        SpillStats {
            files: self.files.load(Ordering::SeqCst),
            bytes: self.written.load(Ordering::SeqCst),
        }
    }

    /// Ciphertext bytes on disk now.
    pub fn bytes_on_disk(&self) -> u64 {
        self.used.load(Ordering::SeqCst)
    }

    /// Make a file of the concrete type, for tests that need to see its bytes.
    pub fn create_file(&self) -> Result<Arc<EncryptedSpillFile>, SpillError> {
        let id = self.next_file.fetch_add(1, Ordering::SeqCst);
        let backing = BackingFile::create_named(&self.dir, "qhd", std::process::id(), id)?;
        self.files.fetch_add(1, Ordering::SeqCst);
        let capture = self
            .capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok(Arc::new(EncryptedSpillFile {
            shared: Arc::new(Shared {
                id,
                cipher: Arc::clone(&self.cipher),
                inner: RwLock::new(Inner {
                    backing,
                    records: Vec::new(),
                }),
                size: AtomicU64::new(0),
                used: Arc::clone(&self.used),
                written: Arc::clone(&self.written),
                quota: self.quota,
                capture,
            }),
        }))
    }

    /// Test hook: from now on, files deposit their raw bytes into the returned list when
    /// they are dropped. Lets a test look at what really reached the disk.
    #[doc(hidden)]
    pub fn capture_raw_for_test(&self) -> RawBytes {
        let sink = Arc::new(Mutex::new(Vec::new()));
        *self.capture.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&sink));
        sink
    }

    /// Test hook: seal under a different key than this factory's, as a forged or
    /// wrong-key record would be.
    #[doc(hidden)]
    pub fn cipher_for_test(&self) -> &SpillCipher {
        &self.cipher
    }
}

impl TempFileFactory for EncryptedTempFiles {
    fn create_temp_file(
        &self,
        _description: &str,
    ) -> datafusion::common::Result<Arc<dyn SpillFile>> {
        Ok(self.create_file()?)
    }
}

#[derive(Debug, Clone, Copy)]
struct Record {
    offset: u64,
    len: u32,
    nonce: u64,
}

struct Inner {
    backing: BackingFile,
    records: Vec<Record>,
}

struct Shared {
    id: u64,
    cipher: Arc<SpillCipher>,
    inner: RwLock<Inner>,
    /// Ciphertext bytes of this file.
    size: AtomicU64,
    used: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
    quota: u64,
    capture: Option<RawBytes>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(sink) = &self.capture {
            let raw = self
                .inner
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .backing
                .read_all_raw_for_test()
                .unwrap_or_default();
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(raw);
        }
        self.used
            .fetch_sub(self.size.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

impl Shared {
    /// Seal `plain` as the next record and append it.
    fn seal(&self, mut plain: Vec<u8>) -> Result<(), SpillError> {
        // The quota is checked before the write so a full spill never grows past it.
        let projected = self.used.load(Ordering::SeqCst) + plain.len() as u64 + 16;
        if projected > self.quota {
            return Err(SpillError::Quota);
        }
        let mut inner = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let index = inner.records.len() as u32;
        let (nonce, record) = self.cipher.seal(self.id, index, &mut plain)?;
        let (offset, len) = inner.backing.append(&record)?;
        inner.records.push(Record { offset, len, nonce });
        self.size.fetch_add(u64::from(len), Ordering::SeqCst);
        self.used.fetch_add(u64::from(len), Ordering::SeqCst);
        self.written.fetch_add(u64::from(len), Ordering::SeqCst);
        Ok(())
    }

    /// Read, authenticate and open record `index`. Nothing unauthenticated is returned.
    fn open(&self, index: usize, record: Record) -> Result<Bytes, SpillError> {
        let mut buf = {
            let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            inner.backing.read(record.offset, record.len)?
        };
        let plain = self
            .cipher
            .open(self.id, index as u32, record.nonce, &mut buf)
            .map_err(|_| SpillError::Auth)?;
        buf.truncate(plain);
        Ok(Bytes::from(buf))
    }
}

/// One spill file, as DataFusion sees it.
pub struct EncryptedSpillFile {
    shared: Arc<Shared>,
}

impl EncryptedSpillFile {
    /// Every byte on disk, ciphertext and tags, for the "no plaintext on disk" test.
    #[doc(hidden)]
    pub fn raw_bytes_for_test(&self) -> Vec<u8> {
        self.shared
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .backing
            .read_all_raw_for_test()
            .unwrap_or_default()
    }

    /// Flip one bit of the first record on disk, as an attacker with write access would.
    #[doc(hidden)]
    pub fn corrupt_first_record_for_test(&self) {
        let mut inner = self
            .shared
            .inner
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(record) = inner.records.first().copied() {
            let mut raw = inner.backing.read(record.offset, record.len).unwrap();
            raw[0] ^= 1;
            // The file only appends, so the damaged copy goes at the end and the
            // index is pointed at it: same effect as damaging the bytes in place.
            let (offset, len) = inner.backing.append(&raw).unwrap();
            inner.records[0] = Record {
                offset,
                len,
                ..record
            };
        }
    }

    /// The number of records written so far.
    #[doc(hidden)]
    pub fn records_for_test(&self) -> usize {
        self.shared
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .records
            .len()
    }
}

impl SpillFile for EncryptedSpillFile {
    /// Ciphertext bytes, so DataFusion's own quota counts what is really on disk.
    fn size(&self) -> Option<u64> {
        Some(self.shared.size.load(Ordering::SeqCst))
    }

    fn read_stream(
        &self,
    ) -> datafusion::common::Result<
        Pin<Box<dyn Stream<Item = datafusion::common::Result<Bytes>> + Send>>,
    > {
        let shared = Arc::clone(&self.shared);
        // Live, not a snapshot: DataFusion's spill pool opens the stream while the file is
        // still being written and only polls it for batches the writer has flushed. Each
        // step looks the next record up again and ends the stream where there is none.
        // One record per step, read on a blocking thread: a 1 MiB `pread` and an AES-GCM
        // open should not run on a worker that is also driving other partitions.
        let stream = futures::stream::unfold((shared, 0usize), |(shared, index)| async move {
            let record = shared
                .inner
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .records
                .get(index)
                .copied()?;
            let opener = Arc::clone(&shared);
            let item = match tokio::task::spawn_blocking(move || opener.open(index, record)).await {
                Ok(result) => result.map_err(DataFusionError::from),
                Err(join) => Err(DataFusionError::External(Box::new(SpillError::Io(
                    join.to_string(),
                )))),
            };
            Some((item, (shared, index + 1)))
        });
        Ok(Box::pin(stream))
    }

    fn open_writer(&self) -> datafusion::common::Result<Box<dyn SpillWriter>> {
        Ok(Box::new(EncryptedSpillWriter {
            shared: Arc::clone(&self.shared),
            buffer: Vec::new(),
        }))
    }
}

/// Buffers plaintext and seals it a megabyte at a time.
struct EncryptedSpillWriter {
    shared: Arc<Shared>,
    buffer: Vec<u8>,
}

impl std::io::Write for EncryptedSpillWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= RECORD_PLAINTEXT {
            let rest = self.buffer.split_off(RECORD_PLAINTEXT);
            let full = std::mem::replace(&mut self.buffer, rest);
            self.shared.seal(full)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // DataFusion's spill pool flushes after each batch and then lets a reader open the
        // file, so what was flushed has to be sealed and readable now. A short record costs
        // a tag and a nonce, which is the price of that contract.
        if !self.buffer.is_empty() {
            let rest = std::mem::take(&mut self.buffer);
            self.shared.seal(rest)?;
        }
        Ok(())
    }
}

impl SpillWriter for EncryptedSpillWriter {
    fn finish(&mut self) -> datafusion::common::Result<()> {
        if !self.buffer.is_empty() {
            let rest = std::mem::take(&mut self.buffer);
            self.shared.seal(rest)?;
        }
        Ok(())
    }
}
