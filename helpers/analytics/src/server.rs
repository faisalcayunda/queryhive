//! The protocol loop: frames in on stdin, frames out on stdout (blueprint sections
//! 14.3 to 14.6).
//!
//! # Threads
//!
//! * The **reader** is the calling thread. It reads one frame at a time, decodes it, and
//!   either answers on the spot (opening a session, registering a result) or hands the
//!   work to the runtime (running SQL, registering a file).
//! * The **writer** is one thread that owns the output. Everything that wants to say
//!   something sends it down one bounded channel, so frames are never interleaved and a
//!   slow app slows the query that is producing output (the channel is the back-pressure).
//! * Queries and chunk fetches run on the runtime, which `main` builds through `qh-rt`.
//!
//! # Who owns what
//!
//! Chunks are pulled by the helper and served by the app. A scan sends a
//! [`HelperMessage::ChunkRequest`] with a fresh request id and waits on a one-shot for the
//! answer, which the reader thread delivers when a `Batch` (or `ChunkError`) frame with that
//! id arrives. A request that is abandoned (the query was cancelled) removes itself, and an
//! answer to a request nobody waits for any more is ignored, not a violation.
//!
//! # Ending
//!
//! EOF on the input means the app is gone: the helper cancels what runs and returns, so no
//! orphan outlives its app. A frame the helper cannot parse is a protocol violation and
//! ends it with an error. `Shutdown` is answered and then ends it cleanly.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use datafusion::arrow::record_batch::RecordBatch;
use futures::FutureExt;
use qh_analytics_proto::{
    decode_batch, decode_control, read_frame, write_control, AppMessage, ErrorKind, HelperMessage,
    Kind, ProtoError, WireColumn, WireError, PROTOCOL_VERSION,
};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};

use crate::output::{pump, Closed, Outcome, Sink};
use crate::remote_table::{ChunkSource, RemoteStoreTable};
use crate::session::{Engine, EngineConfig, Failure, Session};

/// Messages the writer thread sends, in order.
enum Outgoing {
    Control(u64, HelperMessage),
    Batch(u64, Vec<u8>),
    /// Write nothing more and let the thread end. Queued behind everything before it.
    Close,
}

type Answer = Result<RecordBatch, String>;

/// What `serve` needs from its caller.
pub struct ServerConfig {
    /// The build string sent in `Hello`; the app refuses a helper of another build.
    pub build: String,
    pub engine: EngineConfig,
    /// The largest `Batch` payload a query may send; a bigger piece fails that query.
    /// The protocol's own limit in production, lower in tests.
    pub max_chunk_bytes: usize,
    /// Called when the output cannot be written. The reader thread is blocked on input and
    /// cannot be woken, so a helper that only stopped writing would look alive and say
    /// nothing; `main` ends the process here and the app sees EOF (section 14.6).
    pub on_write_failure: fn(),
}

/// Why `serve` ended other than cleanly.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("the engine would not start: {0}")]
    Engine(Failure),
    #[error("protocol violation: {0}")]
    Protocol(ProtoError),
    #[error("could not start the writer thread: {0}")]
    Thread(std::io::Error),
}

struct Shared {
    engine: Engine,
    out: mpsc::Sender<Outgoing>,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    queries: Mutex<HashMap<u64, watch::Sender<bool>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
    next_request: AtomicU64,
    runtime: Handle,
    max_chunk_bytes: usize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    async fn say(&self, id: u64, message: HelperMessage) -> Result<(), Closed> {
        self.out
            .send(Outgoing::Control(id, message))
            .await
            .map_err(|_| Closed)
    }

    /// Say something from the reader thread, which is not inside the runtime.
    fn say_blocking(&self, id: u64, message: HelperMessage) {
        let _ = self.out.blocking_send(Outgoing::Control(id, message));
    }

    fn session(&self, session: u64) -> Result<Arc<Session>, Failure> {
        lock(&self.sessions)
            .get(&session)
            .cloned()
            .ok_or_else(|| Failure::invalid(format!("There is no open session {session}.")))
    }
}

/// Serve one app on `reader` and `writer` until EOF, `Shutdown` or a violation.
///
/// `runtime` is where queries run; it must outlive the call. The reader is the calling
/// thread, so this blocks, and it must therefore not be called from inside a runtime
/// (it sends with `blocking_send`, which panics there).
pub fn serve<R, W>(
    mut reader: R,
    writer: W,
    runtime: Handle,
    config: ServerConfig,
) -> Result<(), ServeError>
where
    R: Read,
    W: Write + Send + 'static,
{
    let engine = Engine::new(config.engine).map_err(ServeError::Engine)?;
    let (out, receiver) = mpsc::channel::<Outgoing>(32);
    let on_write_failure = config.on_write_failure;
    let writer_thread = std::thread::Builder::new()
        .name("qh-writer".to_owned())
        .spawn(move || write_loop(writer, receiver, on_write_failure))
        .map_err(ServeError::Thread)?;

    let shared = Arc::new(Shared {
        engine,
        out,
        sessions: Mutex::new(HashMap::new()),
        queries: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        next_request: AtomicU64::new(1),
        runtime,
        max_chunk_bytes: config.max_chunk_bytes,
    });
    shared.say_blocking(
        0,
        HelperMessage::Hello {
            protocol: PROTOCOL_VERSION,
            build: config.build,
        },
    );

    let result = read_loop(&mut reader, &shared);

    // Whatever ended us, nothing keeps running for an app that is not listening.
    for cancel in lock(&shared.queries).values() {
        let _ = cancel.send(true);
    }
    let _ = shared.out.blocking_send(Outgoing::Close);
    let _ = writer_thread.join();
    result
}

fn write_loop<W: Write>(
    mut writer: W,
    mut receiver: mpsc::Receiver<Outgoing>,
    on_write_failure: fn(),
) {
    while let Some(message) = receiver.blocking_recv() {
        let written = match message {
            Outgoing::Control(id, message) => write_control(&mut writer, id, &message),
            Outgoing::Batch(id, payload) => {
                qh_analytics_proto::write_frame(&mut writer, Kind::Batch, id, &payload)
            }
            Outgoing::Close => return,
        };
        // A write error means the app closed its end, or a frame could not be made.
        // Dropping the receiver makes every later `send` fail, which is how running queries
        // learn to stop; the reader may still be blocked on input, so the process is told.
        if written.is_err() || writer.flush().is_err() {
            on_write_failure();
            return;
        }
    }
}

fn read_loop<R: Read>(reader: &mut R, shared: &Arc<Shared>) -> Result<(), ServeError> {
    loop {
        let frame = match read_frame(reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(()),
            Err(error) => return Err(ServeError::Protocol(error)),
        };
        match frame.kind {
            Kind::Batch => {
                let answer = decode_batch(frame.payload)
                    .map_err(|_| "the app sent a chunk that could not be read".to_owned());
                answer_request(shared, frame.id, answer);
            }
            Kind::Control => {
                let message: AppMessage =
                    decode_control(&frame.payload).map_err(ServeError::Protocol)?;
                if !handle(shared, frame.id, message) {
                    return Ok(());
                }
            }
        }
    }
}

fn answer_request(shared: &Shared, request: u64, answer: Answer) {
    // No entry means the query was cancelled while the app was answering: not an error.
    if let Some(waiting) = lock(&shared.pending).remove(&request) {
        let _ = waiting.send(answer);
    }
}

/// Handle one control message. `false` ends the loop.
fn handle(shared: &Arc<Shared>, id: u64, message: AppMessage) -> bool {
    let reply = |result: Result<HelperMessage, Failure>| {
        let message = result.unwrap_or_else(|failure| HelperMessage::Failed { error: failure.0 });
        shared.say_blocking(id, message);
    };
    match message {
        AppMessage::OpenSession { session } => {
            lock(&shared.sessions)
                .entry(session)
                .or_insert_with(|| Arc::new(shared.engine.session()));
            reply(Ok(HelperMessage::Ack));
        }
        AppMessage::CloseSession { session } => {
            lock(&shared.sessions).remove(&session);
            reply(Ok(HelperMessage::Ack));
        }
        AppMessage::RegisterResult {
            session,
            name,
            token,
            schema,
            rows,
            chunks,
        } => reply(shared.session(session).and_then(|open| {
            let source = Arc::new(ProtocolSource {
                shared: Arc::clone(shared),
                session,
                token,
            });
            let table = RemoteStoreTable::new(Arc::new(schema), rows, chunks, source);
            open.register(&name, Arc::new(table), Some(rows))
                .map(|info| HelperMessage::TableInfo { info })
        })),
        AppMessage::RegisterFile {
            session,
            name,
            path,
            format,
        } => match shared.session(session) {
            Err(failure) => reply(Err(failure)),
            Ok(open) => {
                let shared = Arc::clone(shared);
                shared.runtime.clone().spawn(async move {
                    let result = open.register_file(&name, &path, &format).await;
                    let message = match result {
                        Ok(info) => HelperMessage::TableInfo { info },
                        Err(failure) => HelperMessage::Failed { error: failure.0 },
                    };
                    let _ = shared.say(id, message).await;
                });
            }
        },
        AppMessage::Deregister { session, name } => reply(
            shared
                .session(session)
                .and_then(|open| open.deregister(&name))
                .map(|()| HelperMessage::Ack),
        ),
        AppMessage::RunSql {
            session,
            sql,
            lease_bytes,
            row_cap,
        } => start_query(shared, id, session, sql, lease_bytes, row_cap),
        AppMessage::Cancel => {
            if let Some(cancel) = lock(&shared.queries).get(&id) {
                let _ = cancel.send(true);
            }
        }
        AppMessage::ChunkError { message } => answer_request(shared, id, Err(message)),
        AppMessage::Shutdown => {
            reply(Ok(HelperMessage::Ack));
            return false;
        }
    }
    true
}

fn start_query(
    shared: &Arc<Shared>,
    id: u64,
    session: u64,
    sql: String,
    lease_bytes: u64,
    row_cap: u64,
) {
    let refuse = |error: WireError| {
        shared.say_blocking(
            id,
            HelperMessage::Done {
                rows: 0,
                truncated: false,
                cancelled: false,
                error: Some(error),
            },
        );
    };
    let open = match shared.session(session) {
        Ok(open) => open,
        Err(failure) => return refuse(failure.0),
    };
    let (cancel, cancelled) = watch::channel(false);
    {
        let mut queries = lock(&shared.queries);
        if queries.contains_key(&id) {
            return refuse(Failure::invalid("That query id is already running.").0);
        }
        queries.insert(id, cancel);
    }
    let task = Arc::clone(shared);
    shared.runtime.spawn(async move {
        // A panic inside a query must still end the query, or the app waits for ever.
        let run = AssertUnwindSafe(run_query(
            Arc::clone(&task),
            id,
            open,
            sql,
            lease_bytes,
            row_cap,
            cancelled,
        ))
        .catch_unwind()
        .await;
        let outcome = run.unwrap_or_else(|_| Outcome {
            rows: 0,
            truncated: false,
            cancelled: false,
            error: Some(WireError {
                kind: ErrorKind::Internal,
                message: "The analytics component hit an internal error.".to_owned(),
                position: None,
            }),
        });
        lock(&task.queries).remove(&id);
        let _ = task
            .say(
                id,
                HelperMessage::Done {
                    rows: outcome.rows,
                    truncated: outcome.truncated,
                    cancelled: outcome.cancelled,
                    error: outcome.error,
                },
            )
            .await;
    });
}

async fn run_query(
    shared: Arc<Shared>,
    id: u64,
    session: Arc<Session>,
    sql: String,
    lease_bytes: u64,
    row_cap: u64,
    cancelled: watch::Receiver<bool>,
) -> Outcome {
    // The lease is declared first so it is dropped last: the stream, and with it every
    // reservation DataFusion holds, is gone before the limit comes down.
    let _lease = shared
        .engine
        .grant(usize::try_from(lease_bytes).unwrap_or(usize::MAX));
    match session.query(&sql).await {
        Err(failure) => Outcome {
            rows: 0,
            truncated: false,
            cancelled: false,
            error: Some(failure.0),
        },
        Ok(stream) => {
            let mut sink = ProtocolSink { shared, id };
            pump(stream, row_cap, cancelled, &mut sink).await
        }
    }
}

/// A query's output, as frames.
struct ProtocolSink {
    shared: Arc<Shared>,
    id: u64,
}

#[async_trait]
impl Sink for ProtocolSink {
    async fn columns(&mut self, columns: Vec<WireColumn>) -> Result<(), Closed> {
        self.shared
            .say(self.id, HelperMessage::ResultColumns { columns })
            .await
    }

    async fn chunk(&mut self, payload: Vec<u8>) -> Result<(), Closed> {
        self.shared
            .out
            .send(Outgoing::Batch(self.id, payload))
            .await
            .map_err(|_| Closed)
    }

    async fn progress(&mut self, rows: u64) -> Result<(), Closed> {
        self.shared
            .say(self.id, HelperMessage::Progress { rows })
            .await
    }

    fn max_payload(&self) -> usize {
        self.shared.max_chunk_bytes
    }
}

/// A registered result's chunks, fetched from the app over the pipe.
#[derive(Debug)]
struct ProtocolSource {
    shared: Arc<Shared>,
    session: u64,
    token: String,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared")
    }
}

/// Forgets a request that nobody waits for any more.
struct Pending<'a> {
    shared: &'a Shared,
    request: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        lock(&self.shared.pending).remove(&self.request);
    }
}

#[async_trait]
impl ChunkSource for ProtocolSource {
    async fn fetch(&self, chunk: u32, columns: &[usize]) -> Result<RecordBatch, String> {
        const GONE: &str = "The analytics component lost its connection to the app.";
        let request = self.shared.next_request.fetch_add(1, Ordering::SeqCst);
        let (deliver, answer) = oneshot::channel();
        lock(&self.shared.pending).insert(request, deliver);
        let _forget = Pending {
            shared: &self.shared,
            request,
        };
        self.shared
            .say(
                request,
                HelperMessage::ChunkRequest {
                    session: self.session,
                    token: self.token.clone(),
                    chunk,
                    columns: columns.iter().map(|&column| column as u32).collect(),
                },
            )
            .await
            .map_err(|_| GONE.to_owned())?;
        answer.await.map_err(|_| GONE.to_owned())?
    }
}
