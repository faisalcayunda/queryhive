//! `COPY (<select>) TO STDOUT` text format: the decoder for one row.
//!
//! The server writes each row as one line. Fields are separated by a raw tab, NULL is the
//! two bytes `\N`, and the line ends at a raw newline. A tab, newline or backslash that is
//! part of a value never appears raw: the server writes `\t`, `\n` and `\\` (and `\b`, `\f`,
//! `\r`, `\v`), so splitting on the raw bytes is always right.
//!
//! The text of each value is what the type's output function produced, the same function the
//! simple query protocol uses, so after un-escaping a field is byte for byte the string the
//! normal path hands to [`crate::normalize::from_text`].

use qh_core::EngineError;

/// Split one row, without its trailing newline, into `fields` values and hand each to `cell`
/// with its index. `None` is a NULL.
///
/// A value with no backslash is lent straight from `line`; one with escapes is un-escaped into
/// `scratch`, which the caller reuses across rows, so a plain value costs no allocation.
pub fn decode_line(
    line: &[u8],
    fields: usize,
    scratch: &mut Vec<u8>,
    mut cell: impl FnMut(usize, Option<&str>),
) -> Result<(), EngineError> {
    let mut count = 0;
    let mut rest = line;
    loop {
        let (field, tail) = match memchr::memchr(b'\t', rest) {
            Some(at) => (&rest[..at], Some(&rest[at + 1..])),
            None => (rest, None),
        };
        if count < fields {
            cell(count, decode_field(field, scratch)?);
        }
        count += 1;
        match tail {
            Some(tail) => rest = tail,
            None => break,
        }
    }
    if count != fields {
        return Err(EngineError::Internal {
            message: format!(
                "a COPY row had {count} fields where the statement describes {fields}"
            ),
        });
    }
    Ok(())
}

fn decode_field<'a>(
    field: &'a [u8],
    scratch: &'a mut Vec<u8>,
) -> Result<Option<&'a str>, EngineError> {
    if field == b"\\N" {
        return Ok(None);
    }
    let bytes = if memchr::memchr(b'\\', field).is_none() {
        field
    } else {
        unescape(field, scratch);
        scratch.as_slice()
    };
    std::str::from_utf8(bytes)
        .map(Some)
        .map_err(|error| EngineError::Internal {
            message: format!("a COPY value was not valid UTF-8: {error}"),
        })
}

fn unescape(field: &[u8], out: &mut Vec<u8>) {
    out.clear();
    let mut rest = field;
    while let Some(at) = memchr::memchr(b'\\', rest) {
        out.extend_from_slice(&rest[..at]);
        match rest.get(at + 1) {
            Some(&escaped) => {
                out.push(match escaped {
                    b'b' => 0x08,
                    b'f' => 0x0c,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 0x0b,
                    // `\\`, and any other escaped byte, stands for itself (the server only
                    // emits the ones above, but this is how it reads them back too).
                    other => other,
                });
                rest = &rest[at + 2..];
            }
            // A lone trailing backslash: keep it rather than lose a byte.
            None => {
                out.push(b'\\');
                rest = &[];
            }
        }
    }
    out.extend_from_slice(rest);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(line: &[u8], fields: usize) -> Result<Vec<Option<String>>, EngineError> {
        let mut out = Vec::new();
        decode_line(line, fields, &mut Vec::new(), |_, text| {
            out.push(text.map(str::to_owned));
        })?;
        Ok(out)
    }

    fn row(line: &str, fields: usize) -> Vec<Option<String>> {
        decode(line.as_bytes(), fields).expect("decodes")
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn null_and_empty_are_different() {
        assert_eq!(row("\\N\t\t\\N", 3), vec![None, some(""), None]);
        // A one-column row holding the empty string is an empty line.
        assert_eq!(row("", 1), vec![some("")]);
        assert_eq!(row("\\N", 1), vec![None]);
    }

    #[test]
    fn a_backslash_n_inside_a_value_is_not_a_null() {
        // The text `\N` is written `\\N`.
        assert_eq!(row("\\\\N", 1), vec![some("\\N")]);
        assert_eq!(row("a\\N", 1), vec![some("aN")]);
    }

    #[test]
    fn every_escape_the_server_writes() {
        assert_eq!(
            row("\\b\\f\\n\\r\\t\\v\\\\", 1),
            vec![some("\u{8}\u{c}\n\r\t\u{b}\\")]
        );
    }

    #[test]
    fn a_value_with_a_tab_and_a_newline_stays_one_field() {
        assert_eq!(
            row("a\\tb\\nc\tnext", 2),
            vec![some("a\tb\nc"), some("next")]
        );
    }

    #[test]
    fn multibyte_text_survives_untouched() {
        assert_eq!(
            row("Jakarta \u{1F30F}\t\u{65e5}\u{672c}\\t\u{8a9e}", 2),
            vec![
                some("Jakarta \u{1F30F}"),
                some("\u{65e5}\u{672c}\t\u{8a9e}")
            ]
        );
    }

    #[test]
    fn a_bytea_value_loses_only_the_doubled_backslash() {
        assert_eq!(row("\\\\xdeadbeef", 1), vec![some("\\xdeadbeef")]);
    }

    #[test]
    fn a_field_count_mismatch_is_an_error() {
        assert!(decode(b"a\tb", 3).is_err());
        assert!(decode(b"a\tb", 1).is_err());
    }

    #[test]
    fn bytes_that_are_not_utf8_are_an_error() {
        assert!(decode(b"\xff", 1).is_err());
        assert!(decode(b"\\t\xff", 1).is_err());
    }

    #[test]
    fn the_scratch_buffer_is_reused_without_leaking_the_last_value() {
        let mut scratch = Vec::new();
        let mut seen = Vec::new();
        decode_line(b"a\\tb\tplain", 2, &mut scratch, |_, text| {
            seen.push(text.map(str::to_owned));
        })
        .expect("decodes");
        assert_eq!(seen, vec![some("a\tb"), some("plain")]);
    }
}
