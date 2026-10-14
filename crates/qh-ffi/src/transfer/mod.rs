//! Save to Table across connections (W13-T20): read a query on one connection and write the
//! rows as a new table on another.
//!
//! This module holds the pure half. It runs nothing and opens nothing; the transfer that
//! uses it (`transfer::run`, the writers, the statement plan) lands with the later steps of
//! the build order (blueprint `w13-save-to-table-across-connections.md` §13).
//!
//! - [`types`]: what a source column is ([`types::SqlType`]) and what it is written as
//!   ([`types::map_type`], [`types::plan_table`]): exact, wider, a reported text stand-in, or
//!   refused. Server-supplied type text never reaches SQL.
//! - [`encode`]: one value as a `COPY` text field or a MySQL parameter, or a refusal that
//!   names the rule and never the value.
//! - [`names`]: the one way an identifier is written into SQL, and the names refused.
//!
//! The settings of the second connection are `env`'s (`DEST_*`), and the engine seam that
//! opens it is `Engine::for_settings`.

pub mod encode;
pub mod names;
pub mod types;
