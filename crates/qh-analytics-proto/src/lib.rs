//! `QHA1`: the wire protocol between the app and `queryhive-analytics`.
//!
//! Both sides link this crate, which is why it depends on nothing heavier than
//! `serde` and the Arrow crates the app already has: DataFusion lives in the helper
//! only (blueprint `fase-6-data-plane.md` section 14.1).
//!
//! # Frames
//!
//! ```text
//! u32 LE payload length | u8 kind | u64 LE id | payload
//! ```
//!
//! The length counts the payload only and is at most [`MAX_FRAME_LEN`] (256 MiB).
//! A longer length, an unknown kind, or a stream that ends inside a frame is a
//! protocol violation ([`ProtoError`]); the side that sees one kills the peer rather
//! than trying to resynchronise. A stream that ends exactly between two frames is a
//! clean EOF ([`read_frame`] returns `Ok(None)`).
//!
//! | Kind | Payload |
//! |---|---|
//! | [`Kind::Control`] | one JSON document: [`AppMessage`] (app to helper) or [`HelperMessage`] (helper to app) |
//! | [`Kind::Batch`] | one complete Arrow IPC stream: schema, one record batch, end of stream (the same shape as a spill record body) |
//!
//! `id` is a correlation id. For `RunSql` and everything the helper says about that
//! query it is the query id the app chose. For [`HelperMessage::ChunkRequest`] it is a
//! request id the helper chose, and the app answers with a [`Kind::Batch`] frame (or a
//! [`AppMessage::ChunkError`]) carrying the same id, so an answer is never separated
//! from its request by another frame. For the replies to `OpenSession`,
//! `RegisterResult` and the other session requests it echoes the app's request id.
//!
//! # What crosses
//!
//! Arrow IPC bytes and JSON, never a Rust type, so the Arrow version in the helper
//! need not equal the one in the app. Decoding a [`Kind::Batch`] payload never
//! panics: Arrow's own decoder can panic on a malformed body, and a helper's output is
//! untrusted even when it arrived over our own pipe.

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use serde::{Deserialize, Serialize};

/// The protocol version both sides must speak (`Hello.protocol`).
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest payload a frame may carry.
pub const MAX_FRAME_LEN: u32 = 256 * 1024 * 1024;
/// Bytes before the payload: length, kind, id.
pub const HEADER_LEN: usize = 13;

/// Everything that can go wrong reading or writing a frame or a message.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("the stream ended inside a frame")]
    Truncated,
    #[error("frame length {len} is above the {MAX_FRAME_LEN} byte limit")]
    TooLarge { len: u64 },
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    #[error("a control message is not valid: {0}")]
    BadControl(String),
    #[error("a record batch is not valid: {0}")]
    BadBatch(String),
}

/// What a frame's payload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Control = 1,
    Batch = 2,
}

impl Kind {
    fn from_byte(byte: u8) -> Result<Self, ProtoError> {
        match byte {
            1 => Ok(Kind::Control),
            2 => Ok(Kind::Batch),
            other => Err(ProtoError::UnknownKind(other)),
        }
    }
}

/// One frame, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub id: u64,
    pub payload: Vec<u8>,
}

/// Write one frame. The caller owns flushing.
pub fn write_frame(
    writer: &mut impl Write,
    kind: Kind,
    id: u64,
    payload: &[u8],
) -> Result<(), ProtoError> {
    if payload.len() as u64 > u64::from(MAX_FRAME_LEN) {
        return Err(ProtoError::TooLarge {
            len: payload.len() as u64,
        });
    }
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    header[4] = kind as u8;
    header[5..].copy_from_slice(&id.to_le_bytes());
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    Ok(())
}

/// Read one frame. `Ok(None)` is a clean EOF between frames.
///
/// The length is checked before anything is allocated, and the payload is read
/// through `take`, so a header that promises 256 MiB and then stops costs only the
/// bytes that actually arrived.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, ProtoError> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(ProtoError::Truncated),
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if len > MAX_FRAME_LEN {
        return Err(ProtoError::TooLarge {
            len: u64::from(len),
        });
    }
    let kind = Kind::from_byte(header[4])?;
    let mut id = [0u8; 8];
    id.copy_from_slice(&header[5..]);
    let mut payload = Vec::new();
    let read = reader
        .take(u64::from(len))
        .read_to_end(&mut payload)
        .map_err(ProtoError::Io)?;
    if read != len as usize {
        return Err(ProtoError::Truncated);
    }
    Ok(Some(Frame {
        kind,
        id: u64::from_le_bytes(id),
        payload,
    }))
}

/// Serialise a control message and write it as a [`Kind::Control`] frame.
pub fn write_control<T: Serialize>(
    writer: &mut impl Write,
    id: u64,
    message: &T,
) -> Result<(), ProtoError> {
    let payload = serde_json::to_vec(message).map_err(|e| ProtoError::BadControl(e.to_string()))?;
    write_frame(writer, Kind::Control, id, &payload)
}

/// Parse a [`Kind::Control`] payload.
pub fn decode_control<'a, T: Deserialize<'a>>(payload: &'a [u8]) -> Result<T, ProtoError> {
    serde_json::from_slice(payload).map_err(|e| ProtoError::BadControl(e.to_string()))
}

/// One batch as a complete IPC stream: schema, the batch, end of stream.
pub fn encode_batch(batch: &RecordBatch) -> Result<Vec<u8>, ProtoError> {
    let bad = |e: arrow_schema::ArrowError| ProtoError::BadBatch(e.to_string());
    let mut out = Vec::new();
    {
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(&mut out, &batch.schema()).map_err(bad)?;
        writer.write(batch).map_err(bad)?;
        writer.finish().map_err(bad)?;
    }
    Ok(out)
}

/// Encode a batch and write it as a [`Kind::Batch`] frame.
pub fn write_batch(
    writer: &mut impl Write,
    id: u64,
    batch: &RecordBatch,
) -> Result<(), ProtoError> {
    write_frame(writer, Kind::Batch, id, &encode_batch(batch)?)
}

/// Decode a [`Kind::Batch`] payload: exactly one batch, validated, never a panic.
///
/// Uses the push decoder over the payload, not `StreamReader`: the reader allocates
/// the body length the message *claims* before reading it, so a damaged or hostile
/// header could ask for terabytes. The decoder only ever holds bytes that are in the
/// payload. It takes the `Vec` so the buffers of the batch can point into it.
pub fn decode_batch(payload: Vec<u8>) -> Result<RecordBatch, ProtoError> {
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        use arrow_schema::ArrowError;
        let mut buffer = arrow_buffer::Buffer::from_vec(payload);
        let mut decoder = arrow_ipc::reader::StreamDecoder::new();
        let batch = decoder
            .decode(&mut buffer)?
            .ok_or_else(|| ArrowError::ParseError("the stream holds no batch".into()))?;
        // What follows must be the end-of-stream marker and nothing else.
        if decoder.decode(&mut buffer)?.is_some() {
            return Err(ArrowError::ParseError(
                "the stream holds more than one batch".into(),
            ));
        }
        decoder.finish()?;
        Ok::<_, ArrowError>(batch)
    }));
    match decoded {
        Ok(Ok(batch)) => Ok(batch),
        Ok(Err(error)) => Err(ProtoError::BadBatch(error.to_string())),
        Err(_) => Err(ProtoError::BadBatch(
            "the IPC decoder panicked on this payload".into(),
        )),
    }
}

/// How a file registered with `RegisterFile` is read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case")]
pub enum FileFormat {
    Csv {
        has_header: bool,
        delimiter: u8,
        quote: u8,
    },
    Parquet,
}

/// A column as the UI sees it: the logical name and the Arrow type, spelled out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireColumn {
    pub name: String,
    pub type_name: String,
}

/// A table in a session: what `register_*` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<WireColumn>,
    /// Exact for a registered result, `None` for a file (not counted until read).
    pub rows: Option<u64>,
}

/// Which family an error belongs to; the app maps these onto `StoreFfiError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The SQL, a table name or a path is wrong.
    InvalidArgument,
    /// The lease (the memory budget) cannot hold what the query needs.
    TooLarge,
    /// The helper's own spill failed.
    Spill,
    /// Anything else, including a bug.
    Internal,
}

/// A 1-based line and column inside the SQL text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub kind: ErrorKind,
    pub message: String,
    pub position: Option<Position>,
}

/// App to helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    OpenSession {
        session: u64,
    },
    CloseSession {
        session: u64,
    },
    /// Make a finished result queryable under `name`. `schema` is the logical schema
    /// (section 5.4); `token` is what the helper must quote to read its chunks.
    RegisterResult {
        session: u64,
        name: String,
        token: String,
        schema: Schema,
        rows: u64,
        chunks: u32,
    },
    RegisterFile {
        session: u64,
        name: String,
        path: String,
        format: FileFormat,
    },
    Deregister {
        session: u64,
        name: String,
    },
    /// Run one statement. The header id is the query id.
    RunSql {
        session: u64,
        sql: String,
        /// The lease: the memory the app has set aside for this query.
        lease_bytes: u64,
        row_cap: u64,
    },
    /// Stop the query whose id is in the header.
    Cancel,
    /// The app could not serve a `ChunkRequest`; the header id is the request id.
    ChunkError {
        message: String,
    },
    Shutdown,
}

/// Helper to app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperMessage {
    /// First message, within five seconds of spawn.
    Hello { protocol: u32, build: String },
    /// The reply to `OpenSession`, `CloseSession`, `Deregister` and `Shutdown`.
    Ack,
    /// The reply to a session request that failed. The header id is the request id.
    Failed { error: WireError },
    /// The reply to `RegisterResult` and `RegisterFile`.
    TableInfo { info: TableInfo },
    /// "Send me chunk `chunk` of `token`, columns `columns` only." The header id is the
    /// request id; the answer is a `Batch` frame with the same id.
    ChunkRequest {
        session: u64,
        token: String,
        chunk: u32,
        columns: Vec<u32>,
    },
    /// The output columns of the running query. The header id is the query id.
    ResultColumns { columns: Vec<WireColumn> },
    /// Progress for the running query, at most once per 16 ms.
    Progress { rows: u64 },
    /// The last message about a query.
    Done {
        rows: u64,
        truncated: bool,
        cancelled: bool,
        error: Option<WireError>,
    },
}
