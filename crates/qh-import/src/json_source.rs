//! The JSON source: a JSON array of objects, or JSONL / concatenated objects,
//! read one element at a time.
//!
//! The framer is this module's own: it finds where each top-level object ends by
//! counting `{[` / `}]` outside strings, then hands exactly those bytes to
//! `serde_json`. Memory is the largest element, not the file. (`serde_json`'s
//! `StreamDeserializer` is not used: it does not understand the commas of an
//! array.) Values are kept as raw text, so a 30-digit number or `1.10` reaches
//! the target untouched, and `null`, `""` and a missing key stay distinct
//! (`None`, `Some("")`, `None` for the missing key only because the column map
//! has no cell to put there; the caller's NULL rule decides the rest).

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::{ImportError, Options};

/// Largest single element, in bytes. Anything bigger is an error with its line.
pub const MAX_ELEMENT_BYTES: usize = 64 * 1024 * 1024;
/// Deepest `{[` nesting accepted inside an element.
pub const MAX_DEPTH: usize = 128;
/// Elements and bytes read up front to name the columns.
pub const SAMPLE_ROWS: usize = 1_000;
pub const SAMPLE_BYTES: usize = 16 * 1024 * 1024;
/// Unmapped key names remembered for the caller's warning.
pub const MAX_UNMAPPED: usize = 20;

#[derive(Debug, Clone, Copy)]
struct Limits {
    max_element_bytes: usize,
    sample_rows: usize,
    sample_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_element_bytes: MAX_ELEMENT_BYTES,
            sample_rows: SAMPLE_ROWS,
            sample_bytes: SAMPLE_BYTES,
        }
    }
}

/// One data row. `None` is a definite NULL (JSON `null`, or a key this object
/// lacks); a string value is `Some` even when empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonRow {
    /// 1-based line where the element starts.
    pub line: usize,
    pub cells: Vec<Option<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Unstarted,
    Stream,
    Array,
}

/// An object as its key/value pairs in file order; the values stay raw text.
struct Obj(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Obj {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Obj;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Obj, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, Box<RawValue>>()? {
                    pairs.push(pair);
                }
                Ok(Obj(pairs))
            }
        }
        deserializer.deserialize_map(V)
    }
}

struct Element {
    bytes: Vec<u8>,
    line: usize,
    offset: usize,
    index: usize,
}

/// An open JSON / JSONL file.
pub struct JsonSource {
    reader: BufReader<Box<dyn Read + Send>>,
    path: String,
    limits: Limits,
    mode: Mode,
    /// After `[`: true until the first element, and after a comma.
    expect_element: bool,
    done: bool,
    line: usize,
    offset: usize,
    count: usize,
    header: Vec<String>,
    index: HashMap<String, usize>,
    queue: VecDeque<(Element, Obj)>,
    unmapped: Vec<String>,
}

impl JsonSource {
    /// Open `path`, reading the sample that names the columns.
    pub fn open(path: &Path, _options: &Options) -> Result<Self, ImportError> {
        let file = std::fs::File::open(path).map_err(|error| ImportError::io(path, error))?;
        Self::build(
            Box::new(file),
            path.display().to_string(),
            Limits::default(),
        )
    }

    /// A source over text already in hand, for a test.
    pub fn from_text(text: String, _options: &Options) -> Result<Self, ImportError> {
        let reader = Box::new(std::io::Cursor::new(text.into_bytes()));
        Self::build(reader, String::from("<memory>"), Limits::default())
    }

    fn build(
        reader: Box<dyn Read + Send>,
        path: String,
        limits: Limits,
    ) -> Result<Self, ImportError> {
        let mut source = Self {
            reader: BufReader::new(reader),
            path,
            limits,
            mode: Mode::Unstarted,
            expect_element: true,
            done: false,
            line: 1,
            offset: 0,
            count: 0,
            header: Vec::new(),
            index: HashMap::new(),
            queue: VecDeque::new(),
            unmapped: Vec::new(),
        };
        source.sample()?;
        Ok(source)
    }

    /// Column names: first-seen key order over the sample. `None` when the file
    /// has no keys at all.
    pub fn header(&self) -> Option<&[String]> {
        Some(self.header.as_slice()).filter(|header| !header.is_empty())
    }

    /// Up to [`MAX_UNMAPPED`] keys that appeared after the sample and so were not
    /// imported. The caller reports them rather than dropping them silently.
    pub fn unmapped_keys(&self) -> &[String] {
        &self.unmapped
    }

    pub fn next_row(&mut self) -> Result<Option<JsonRow>, ImportError> {
        let (element, obj) = match self.queue.pop_front() {
            Some(entry) => entry,
            None => match self.next_object()? {
                Some(entry) => entry,
                None => return Ok(None),
            },
        };
        let mut cells = vec![None; self.header.len()];
        for (key, value) in obj.0 {
            match self.index.get(&key) {
                // A repeated key overwrites: the last one wins.
                Some(&column) => {
                    cells[column] = cell(&value).map_err(|error| {
                        self.error(
                            element.line,
                            element.offset,
                            format!(
                                "element {}, key '{key}': invalid string: {error}",
                                element.index
                            ),
                        )
                    })?
                }
                None => self.note_unmapped(key),
            }
        }
        Ok(Some(JsonRow {
            line: element.line,
            cells,
        }))
    }

    fn note_unmapped(&mut self, key: String) {
        if self.unmapped.len() < MAX_UNMAPPED && !self.unmapped.contains(&key) {
            self.unmapped.push(key);
        }
    }

    fn sample(&mut self) -> Result<(), ImportError> {
        let mut bytes = 0;
        while self.queue.len() < self.limits.sample_rows && bytes < self.limits.sample_bytes {
            let Some((element, obj)) = self.next_object()? else {
                break;
            };
            bytes += element.bytes.len();
            for (key, _) in &obj.0 {
                if !self.index.contains_key(key) {
                    self.index.insert(key.clone(), self.header.len());
                    self.header.push(key.clone());
                }
            }
            self.queue.push_back((element, obj));
        }
        Ok(())
    }

    fn next_object(&mut self) -> Result<Option<(Element, Obj)>, ImportError> {
        let Some(element) = self.next_element()? else {
            return Ok(None);
        };
        let obj = serde_json::from_slice::<Obj>(&element.bytes).map_err(|error| {
            self.error(
                element.line,
                element.offset,
                format!("element {} is not valid JSON: {error}", element.index),
            )
        })?;
        Ok(Some((element, obj)))
    }

    fn error(&self, line: usize, offset: usize, message: String) -> ImportError {
        ImportError::Json {
            path: self.path.clone(),
            line,
            offset,
            message,
        }
    }

    fn here(&self, message: &str) -> ImportError {
        self.error(self.line, self.offset, message.to_owned())
    }

    fn io(&self, error: std::io::Error) -> ImportError {
        ImportError::Io {
            path: self.path.clone(),
            source: error,
        }
    }

    fn peek(&mut self) -> Result<Option<u8>, ImportError> {
        match self.reader.fill_buf() {
            Ok(buf) => Ok(buf.first().copied()),
            Err(error) => Err(self.io(error)),
        }
    }

    fn bump(&mut self, byte: u8) {
        self.reader.consume(1);
        self.offset += 1;
        if byte == b'\n' {
            self.line += 1;
        }
    }

    fn skip_ws(&mut self) -> Result<(), ImportError> {
        while let Some(byte) = self.peek()? {
            if !matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
                break;
            }
            self.bump(byte);
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), ImportError> {
        let head = self.reader.fill_buf().map_err(|e| ImportError::Io {
            path: self.path.clone(),
            source: e,
        })?;
        if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
            return Err(self.here("the file is UTF-16; only UTF-8 is supported"));
        }
        if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.reader.consume(3);
            self.offset += 3;
        }
        self.skip_ws()?;
        self.mode = if self.peek()? == Some(b'[') {
            self.bump(b'[');
            Mode::Array
        } else {
            Mode::Stream
        };
        Ok(())
    }

    /// Frame the next top-level element, or `None` at the end.
    fn next_element(&mut self) -> Result<Option<Element>, ImportError> {
        if self.mode == Mode::Unstarted {
            self.start()?;
        }
        if self.done {
            return Ok(None);
        }
        self.skip_ws()?;
        if self.mode == Mode::Array {
            match self.peek()? {
                None => return Err(self.here("unexpected end of file: the array is not closed")),
                Some(b']') if self.count == 0 || !self.expect_element => {
                    self.bump(b']');
                    self.skip_ws()?;
                    if self.peek()?.is_some() {
                        return Err(self.here("unexpected data after the closing ']'"));
                    }
                    self.done = true;
                    return Ok(None);
                }
                Some(b']') => return Err(self.here("trailing comma before ']'")),
                Some(b',') if !self.expect_element => {
                    self.bump(b',');
                    self.expect_element = true;
                    self.skip_ws()?;
                    if self.peek()? == Some(b']') {
                        return Err(self.here("trailing comma before ']'"));
                    }
                }
                Some(byte) if !self.expect_element => {
                    return Err(self.here(&format!(
                        "expected ',' or ']' after element {}, found {:?}",
                        self.count, byte as char
                    )));
                }
                Some(_) => {}
            }
        } else if self.peek()?.is_none() {
            self.done = true;
            return Ok(None);
        }
        let (line, offset, index) = (self.line, self.offset, self.count + 1);
        match self.peek()? {
            Some(b'{') => {}
            Some(_) => {
                return Err(self.error(
                    line,
                    offset,
                    format!("element {index} (line {line}) is not an object"),
                ))
            }
            None => return Err(self.here("unexpected end of file: the array is not closed")),
        }
        let bytes = self.read_object(line, offset)?;
        self.count += 1;
        self.expect_element = false;
        Ok(Some(Element {
            bytes,
            line,
            offset,
            index,
        }))
    }

    /// Read from `{` to its matching `}`, honouring strings and escapes.
    fn read_object(&mut self, line: usize, offset: usize) -> Result<Vec<u8>, ImportError> {
        let mut out = Vec::new();
        let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
        loop {
            let chunk = self.reader.fill_buf().map_err(|e| ImportError::Io {
                path: self.path.clone(),
                source: e,
            })?;
            if chunk.is_empty() {
                return Err(self.error(
                    line,
                    offset,
                    format!("unexpected end of file inside the element that starts at line {line}"),
                ));
            }
            let mut used = chunk.len();
            let mut finished = false;
            let mut too_deep = false;
            for (i, &byte) in chunk.iter().enumerate() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else {
                    match byte {
                        b'"' => in_string = true,
                        b'{' | b'[' => {
                            depth += 1;
                            too_deep = depth > MAX_DEPTH;
                        }
                        b'}' | b']' => {
                            depth = depth.saturating_sub(1);
                            finished = depth == 0;
                        }
                        _ => {}
                    }
                }
                if too_deep || finished {
                    used = i + 1;
                    break;
                }
            }
            out.extend_from_slice(&chunk[..used]);
            self.line += chunk[..used].iter().filter(|&&b| b == b'\n').count();
            self.offset += used;
            self.reader.consume(used);
            if too_deep {
                return Err(self.error(
                    line,
                    offset,
                    format!("element nesting is deeper than {MAX_DEPTH} levels (line {line})"),
                ));
            }
            if out.len() > self.limits.max_element_bytes {
                return Err(self.error(
                    line,
                    offset,
                    format!(
                        "element at line {line} is larger than {} bytes",
                        self.limits.max_element_bytes
                    ),
                ));
            }
            if finished {
                return Ok(out);
            }
        }
    }
}

/// A raw JSON value as a cell: `null` is NULL, a string is decoded, anything
/// else (numbers, booleans, objects, arrays) keeps its original text.
fn cell(value: &RawValue) -> Result<Option<String>, serde_json::Error> {
    let raw = value.get();
    Ok(match raw.as_bytes().first() {
        Some(b'n') => None,
        Some(b'"') => Some(serde_json::from_str::<String>(raw)?),
        _ => Some(raw.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(text: &str) -> JsonSource {
        JsonSource::from_text(text.to_owned(), &Options::default()).unwrap()
    }

    fn rows(mut s: JsonSource) -> Vec<JsonRow> {
        std::iter::from_fn(|| s.next_row().transpose())
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    fn err_text(text: &str) -> String {
        let mut s = match JsonSource::from_text(text.to_owned(), &Options::default()) {
            Ok(s) => s,
            Err(e) => return e.to_string(),
        };
        loop {
            match s.next_row() {
                Err(e) => return e.to_string(),
                Ok(None) => panic!("no error for {text:?}"),
                Ok(Some(_)) => {}
            }
        }
    }

    #[test]
    fn lone_surrogate_string_is_an_error_not_null() {
        for text in [r#"{"a":"\ud800"}"#, r#"{"a":"x\ud83d"}"#] {
            let e = err_text(text);
            assert!(e.contains("key 'a'") && e.contains("line 1"), "{e}");
        }
    }

    #[test]
    fn jsonl_array_and_pretty_printed_read_the_same() {
        let jsonl = "{\"a\":1,\"b\":\"x\"}\n{\"a\":2,\"b\":\"y\"}\n";
        let array = "[{\"a\":1,\"b\":\"x\"},\n {\"a\":2,\"b\":\"y\"}]\n";
        let pretty = "{\n  \"a\": 1,\n  \"b\": \"x\"\n}\n{\n  \"a\": 2,\n  \"b\": \"y\"\n}";
        let cells = |t| {
            rows(src(t))
                .into_iter()
                .map(|r| r.cells)
                .collect::<Vec<_>>()
        };
        let want = vec![vec![some("1"), some("x")], vec![some("2"), some("y")]];
        assert_eq!(cells(jsonl), want);
        assert_eq!(cells(array), want);
        assert_eq!(cells(pretty), want);
        assert_eq!(src(jsonl).header().unwrap(), ["a", "b"]);
    }

    #[test]
    fn lines_are_where_the_element_starts() {
        let r = rows(src("{\n\"a\":1\n}\n\n{\"a\":2}"));
        assert_eq!((r[0].line, r[1].line), (1, 5));
    }

    #[test]
    fn bom_is_skipped_and_utf16_is_refused() {
        assert_eq!(rows(src("\u{feff}{\"a\":1}")).len(), 1);
        let mut s = JsonSource::build(
            Box::new(std::io::Cursor::new(vec![0xFF, 0xFE, b'{', 0])),
            "p".into(),
            Limits::default(),
        );
        assert!(s
            .as_mut()
            .err()
            .is_some_and(|e| e.to_string().contains("UTF-16")));
    }

    #[test]
    fn null_empty_and_missing_are_told_apart_and_values_stay_raw() {
        let r = rows(src(
            "{\"a\":null,\"b\":\"\",\"c\":123456789012345678901234567890,\"d\":1.10,\"e\":true,\"f\":{\"k\": [1, 2]},\"g\":\"q\\u00e9\\n\"}\n{\"b\":\"z\"}",
        ));
        assert_eq!(
            r[0].cells,
            vec![
                None,
                some(""),
                some("123456789012345678901234567890"),
                some("1.10"),
                some("true"),
                some("{\"k\": [1, 2]}"),
                some("q\u{e9}\n"),
            ]
        );
        assert_eq!(r[1].cells[0], None);
        assert_eq!(r[1].cells[1], some("z"));
    }

    #[test]
    fn duplicate_key_last_wins() {
        let r = rows(src("{\"a\":1,\"b\":2,\"a\":3}"));
        assert_eq!(r[0].cells, vec![some("3"), some("2")]);
    }

    #[test]
    fn keys_after_the_sample_are_reported_not_imported() {
        let s = JsonSource::build(
            Box::new(std::io::Cursor::new(
                b"{\"a\":1}\n{\"a\":2,\"late\":9}\n{\"late\":1,\"a\":3}".to_vec(),
            )),
            "p".into(),
            Limits {
                sample_rows: 1,
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(s.header().unwrap(), ["a"]);
        let mut s = s;
        while s.next_row().unwrap().is_some() {}
        assert_eq!(s.unmapped_keys(), ["late"]);
    }

    #[test]
    fn framing_survives_braces_and_quotes_inside_strings() {
        let r = rows(src(r#"{"a":"}{\"[","b":"]"}{"a":"x"}"#));
        assert_eq!(r[0].cells, vec![some("}{\"["), some("]")]);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn errors_carry_line_and_offset() {
        assert!(err_text("{\"a\":1}\n[1]").contains("not an object"));
        assert!(err_text("[{\"a\":1},\n 5]").contains("element 2 (line 2) is not an object"));
        assert!(err_text("{\"a\":1}\n{\"a\":}").contains("element 2"));
        assert!(err_text("{\"a\":1}\n{\"a\":}").contains("line 2"));
        assert!(err_text("[{\"a\":1},]").contains("trailing comma"));
        assert!(err_text("[{\"a\":1}").contains("not closed"));
        assert!(err_text("[{\"a\":1}] x").contains("after the closing"));
        assert!(err_text("[{\"a\":1} {\"a\":2}]").contains("expected ',' or ']'"));
        assert!(err_text("{\"a\":1").contains("unexpected end of file"));
    }

    #[test]
    fn size_and_depth_are_capped() {
        let big = JsonSource::build(
            Box::new(std::io::Cursor::new(
                b"{\"a\":1}\n{\"a\":\"xxxxxxxxxxxxxxxxxxxx\"}".to_vec(),
            )),
            "p".into(),
            Limits {
                max_element_bytes: 16,
                ..Limits::default()
            },
        );
        let e = big.err().unwrap().to_string();
        assert!(
            e.contains("larger than 16 bytes") && e.contains("line 2"),
            "{e}"
        );
        let deep = format!("{{\"a\":{}1{}}}", "[".repeat(200), "]".repeat(200));
        assert!(err_text(&deep).contains("deeper than"));
    }

    #[test]
    fn empty_inputs_have_no_header_and_no_rows() {
        for text in ["", "  \n", "[]", "[ ]"] {
            let s = src(text);
            assert!(s.header().is_none());
            assert!(rows(s).is_empty());
        }
    }
}
