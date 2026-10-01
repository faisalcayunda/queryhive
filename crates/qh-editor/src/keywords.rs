//! Which bare words are keywords: the grammar's own `keyword_*` symbols, plus a short list the
//! grammar does not know. It matters only where the tree does not classify a word: the gaps
//! around an ERROR node, a statement too big to parse, and a word the grammar reads as an
//! identifier. `qh-sql` does not know this list (it would make the engine crate depend on the
//! editor's grammar); its lexer reports words without a class.

use std::collections::HashSet;
use std::sync::OnceLock;

use tree_sitter::Language;

/// Words the editor has always coloured as keywords that the grammar has no symbol for.
pub const EXTRA_KEYWORDS: &[&str] = &[
    "apply",
    "at",
    "catalog",
    "charset",
    "describe",
    "fetch",
    "grant",
    "identity",
    "ilike",
    "regexp",
    "revoke",
    "rlike",
    "straight_join",
    "try_cast",
    "unnest",
];

/// The longest keyword; anything longer is not one, and a lookup need not copy it.
const LONGEST: usize = 40;

fn set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        let language = Language::from(qh_sql_grammar::LANGUAGE);
        let mut set: HashSet<&'static str> = EXTRA_KEYWORDS.iter().copied().collect();
        for id in 0..language.node_kind_count() as u16 {
            if let Some(word) = language
                .node_kind_for_id(id)
                .and_then(|kind| kind.strip_prefix("keyword_"))
            {
                set.insert(word);
            }
        }
        set
    })
}

/// Whether `word` is a keyword, in any letter case.
pub fn is_keyword(word: &[u8]) -> bool {
    if word.is_empty() || word.len() > LONGEST {
        return false;
    }
    let mut lower = [0u8; LONGEST];
    for (slot, byte) in lower.iter_mut().zip(word) {
        *slot = byte.to_ascii_lowercase();
    }
    std::str::from_utf8(&lower[..word.len()]).is_ok_and(|word| set().contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_and_extra_keywords_are_found_in_any_case() {
        for word in [
            "select", "SELECT", "Where", "varchar", "describe", "GRANT", "try_cast",
        ] {
            assert!(is_keyword(word.as_bytes()), "{word}");
        }
        for word in ["", "selectx", "orders", "x", "id"] {
            assert!(!is_keyword(word.as_bytes()), "{word}");
        }
    }

    /// The 131 words the regex colouring knew (`SQLSyntax.keywords`) are all still keywords.
    #[test]
    fn every_word_of_the_old_keyword_list_is_a_keyword() {
        let old = "select from where group by order having limit offset fetch insert into values update set \
            delete merge using on as join inner left right full outer cross natural lateral apply union all \
            distinct except intersect with recursive over partition window filter range rows preceding \
            following unbounded current row and or not in exists between like ilike rlike regexp is case \
            when then else end cast try_cast null true false asc desc nulls first last create replace table \
            view schema database catalog drop alter add column rename to if primary key foreign references \
            unique index grant revoke explain analyze describe show use begin commit rollback transaction \
            interval at time zone escape collate default constraint check cascade restrict unnest generated \
            always identity returning conflict do nothing engine charset auto_increment unsigned zerofill \
            straight_join force";
        let missing: Vec<&str> = old
            .split_whitespace()
            .filter(|w| !is_keyword(w.as_bytes()))
            .collect();
        assert_eq!(old.split_whitespace().count(), 131);
        assert!(missing.is_empty(), "{missing:?}");
    }
}
