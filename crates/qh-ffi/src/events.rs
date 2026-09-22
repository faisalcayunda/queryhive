//! The event stream the app decodes: one compact JSON object per line.
//!
//! This is the Rust side of `queryhive_engine.py`'s `emit()`. Two things about it
//! are contracts rather than style:
//!
//! 1. **Every line flushes as it is written.** The app paints a grid from the
//!    events as they arrive; a buffered stream would turn a progressive preview
//!    into one lump at the end, which is the defect the Python engine's
//!    `print(..., flush=True)` existed to avoid.
//! 2. **`event` is a field, not a wrapper.** The payload of an event sits beside
//!    it in the same object, so a decoder reads `{"event": "rows", "data": [...]}`
//!    in one pass. Anything else would be a protocol change.
//!
//! Keys are emitted sorted (the map is a `BTreeMap`), which is what the snapshot
//! recorder normalises to, so the engine's stdout can be compared with the frozen
//! Python lines without a normaliser in between.

use std::io::{self, Write};

use serde_json::{Map, Value as Json};

/// Where events go. A command writes to this and never to stdout directly, so the
/// same command can be driven by a test that captures the events instead.
pub trait Emitter {
    fn emit(&mut self, event: Json) -> io::Result<()>;
}

/// Events as JSON lines on a writer, flushed one at a time.
pub struct JsonLines<W: Write> {
    out: W,
}

impl<W: Write> JsonLines<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// The writer back, for a caller that wants to close or inspect it.
    pub fn into_inner(self) -> W {
        self.out
    }
}

impl<W: Write> Emitter for JsonLines<W> {
    fn emit(&mut self, event: Json) -> io::Result<()> {
        serde_json::to_writer(&mut self.out, &event)?;
        self.out.write_all(b"\n")?;
        // Flushed per event: a grid paints as the events arrive, and a buffered
        // line would hold a whole batch hostage behind the next one.
        self.out.flush()
    }
}

/// An event under construction.
///
/// A builder rather than a `json!` literal because half these events carry an
/// optional key (`query_id` a driver may not have) and a `null` is not the same
/// as an absent key to the app's decoder.
#[derive(Debug, Clone)]
pub struct Event {
    object: Map<String, Json>,
}

/// Start one event, named as the app's decoder expects it.
pub fn event(name: &str) -> Event {
    let mut object = Map::new();
    object.insert("event".to_owned(), Json::from(name));
    Event { object }
}

impl Event {
    pub fn field(mut self, key: &str, value: impl Into<Json>) -> Self {
        self.object.insert(key.to_owned(), value.into());
        self
    }

    /// A field that is left out entirely when there is nothing to say.
    pub fn maybe(mut self, key: &str, value: Option<impl Into<Json>>) -> Self {
        if let Some(value) = value {
            self.object.insert(key.to_owned(), value.into());
        }
        self
    }

    pub fn build(self) -> Json {
        Json::Object(self.object)
    }
}

/// Events kept in memory, for tests and for the golden harness.
#[derive(Debug, Default)]
pub struct Capture {
    pub lines: Vec<Json>,
}

impl Capture {
    pub fn new() -> Self {
        Self::default()
    }

    /// The captured events as the harness compares them: one JSON line each, keys
    /// sorted, exactly the shape `JsonLines` writes.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("an event is serialisable"))
            .collect()
    }
}

impl Emitter for Capture {
    fn emit(&mut self, event: Json) -> io::Result<()> {
        self.lines.push(event);
        Ok(())
    }
}
