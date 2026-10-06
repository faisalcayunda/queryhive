//! The SQL grammar for `tree-sitter`, vendored from DerekStride/tree-sitter-sql 0.3.11.
//!
//! Why vendored and not the crates.io crate: the 0.3.11 external scanner leaks its dollar-quote
//! tag on every re-parse of a statement holding a `$body$ … $body$` (about 74 bytes per edit),
//! which an editor that re-parses on each keystroke turns into a steady leak. The fix is
//! upstream (PR #361, merged after the last release), so this crate carries it as
//! `vendor/scanner.patch`, together with a few local scanner fixes (allocation failures,
//! non-ASCII tags). `PROVENANCE.md` records where every file came from.
//!
//! The only `unsafe` here is the declaration of the C entry point and the one call that wraps
//! it, and both are the shape `tree-sitter-language` documents for a generated grammar.

use tree_sitter_language::LanguageFn;

// The generated entry point in `vendor/parser.c`. It takes no argument and returns a pointer
// to a static `TSLanguage`.
#[allow(unsafe_code)]
extern "C" {
    fn tree_sitter_sql() -> *const ();
}

/// The `tree-sitter` language for SQL. Convert with `tree_sitter::Language::from(LANGUAGE)`.
// SAFETY: `tree_sitter_sql` is the entry point generated for this grammar and compiled by
// `build.rs`; it returns a valid `TSLanguage` pointer with static lifetime, which is the
// contract `LanguageFn::from_raw` asks for.
#[allow(unsafe_code)]
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_sql) };

/// The grammar's `node-types.json`, for tests that check node names against it.
pub const NODE_TYPES: &str = include_str!("../node-types.json");

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sql: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("the vendored grammar's ABI is one the runtime supports");
        parser
            .parse(sql, None)
            .expect("no timeout or cancellation is set")
    }

    #[test]
    fn a_select_parses_without_error() {
        let tree = parse("SELECT a, b FROM t WHERE a > 1;");
        assert!(!tree.root_node().has_error());
        assert_eq!(tree.root_node().kind(), "program");
    }

    #[test]
    fn a_dollar_quoted_body_is_one_statement_with_a_dollar_quote_node() {
        let tree =
            parse("CREATE FUNCTION f() RETURNS int AS $body$ SELECT 1; $body$ LANGUAGE sql;");
        assert!(!tree.root_node().has_error());
        assert!(tree.root_node().to_sexp().contains("dollar_quote"));
    }

    #[test]
    fn a_non_ascii_dollar_quote_tag_is_not_narrowed_onto_an_ascii_one() {
        // The scanner used to store each tag character as one byte: U+0141 became 0x41, so
        // `$Ł$` read as `$A$` and the literal ended at the inner `$A$`. Only `Ł` fails on the
        // old scanner; `€` and `😀` cover the 3- and 4-byte encodings.
        for tag in ["Ł", "€", "😀"] {
            let tree = parse(&format!("SELECT ${tag}$ a $A$ b ${tag}$;"));
            assert!(
                !tree.root_node().has_error(),
                "{tag}: {}",
                tree.root_node().to_sexp()
            );
        }
    }

    #[test]
    fn a_dollar_quote_tag_longer_than_the_scanner_buffer_still_matches() {
        // 5,000 two-byte characters: the tag buffer starts at 1,024 bytes and doubles twice.
        let tag = "é".repeat(5_000);
        let tree = parse(&format!("SELECT ${tag}$ a ${tag}$;"));
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn node_types_lists_the_kinds_the_parser_produces() {
        assert!(NODE_TYPES.contains("\"type\": \"program\""));
        assert!(NODE_TYPES.contains("\"type\": \"dollar_quote\""));
    }
}
