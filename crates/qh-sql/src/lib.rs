//! Dialect-aware SQL scanning, statement wrapping and identifier quoting.
//!
//! This crate exists because two things the Python engine did are easy to get
//! subtly wrong, and both sit on a path the user can reach:
//!
//! * **Finding the end of a statement.** `;` inside a string literal, inside a
//!   comment, or inside a Postgres dollar-quoted block is *text*, not a
//!   separator. A naive `rstrip(";")` deletes a character the user typed. The
//!   Python engine got this right (`exporter/drivers.py:365`), and the behaviour
//!   is pinned by failing checks in `tests/test_engine_events.py` — this crate
//!   reproduces that behaviour and keeps those expectations as its own tests.
//! * **Quoting an identifier.** Every driver quotes differently, and a quote
//!   character inside an identifier has to be doubled, or a table named
//!   `an"alytics` produces SQL that does not parse.
//!
//! Every public function here is pure: no I/O, no globals, no dialect registry.

#![forbid(unsafe_code)]

mod ident;
mod scan;
mod wrap;

pub use ident::{quote_ident, IdentStyle};
pub use scan::{has_significant_text, scan, statement_count, Scan};
pub use wrap::{count_statement, strip_terminator, SqlError};
