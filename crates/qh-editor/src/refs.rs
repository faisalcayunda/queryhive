//! The tables a statement reads or writes, and the aliases it gives them: what column
//! completion needs to turn `a.` into a table.
//!
//! The scope is the statement holding the offset, from `qh_sql::walk`, never a tree. Its tree
//! (the one the colours use, parsed on demand and kept in the same cache) gives the relations;
//! a statement the grammar could not parse cleanly, one too big to parse and a document over
//! the colour ceiling fall back to a lexical scan of `FROM|JOIN|UPDATE|INTO|TABLE name [AS]
//! alias`. Nothing here runs per keystroke: the caller asks, in the background.
//!
//! The rule is that no suggestion beats a wrong one. A relation this cannot read (a function,
//! `VALUES`, a CTE with no column list) is left out, and an alias declared twice is reported
//! twice, so the caller can tell it is ambiguous.

use qh_sql::{first_significant, lex, Token, TokenKind};
use tree_sitter::Node;

use crate::keywords::is_keyword;
use crate::paint::{Analyzer, Document};
use crate::text::EditError;
use crate::TREE_CACHE_BYTES;

/// Most relations reported for one statement.
pub const MAX_RELATIONS: usize = 64;

/// Bytes around the offset the lexical fallback reads in a statement too big to parse.
const WINDOW_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSource {
    /// Read from the statement's tree.
    Tree,
    /// Read by the lexical fallback (no tree, or a tree with errors). Sees no subqueries and
    /// no CTE column lists.
    Lexical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    /// `(select …) alias`.
    Subquery,
    /// A table reference whose name is a CTE declared in the same statement.
    Cte,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelationKind,
    /// Parts of the written name, quotes removed (`"a""b"` is `a"b`). `catalog` is the first of
    /// three parts. `name` is `None` for a subquery.
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub name: Option<String>,
    /// The last part of the name was written in quotes, so it matches exactly, not by case.
    pub quoted: bool,
    pub alias: Option<String>,
    /// The name's last part (UTF-16, whole document); for a subquery, the subquery itself.
    pub name_start_utf16: u32,
    pub name_len_utf16: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cte {
    pub name: String,
    /// The declared column list; empty when there is none.
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct References {
    pub revision: u64,
    /// The statement that was read (UTF-16, whole document). Length 0 with no relations when
    /// the offset is in no statement.
    pub statement_start_utf16: u32,
    pub statement_len_utf16: u32,
    pub relations: Vec<Relation>,
    pub ctes: Vec<Cte>,
    pub source: RefSource,
}

/// A relation with byte offsets relative to the text it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Raw {
    pub kind: RelationKind,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub name: Option<String>,
    pub quoted: bool,
    pub alias: Option<String>,
    pub at: (usize, usize),
}

impl Analyzer {
    /// The relations of the statement holding `offset_utf16`. An offset in the blank after a
    /// statement belongs to that statement; one before the first significant byte of the text
    /// belongs to none and gets an empty answer.
    pub fn references(&mut self, offset_utf16: u32) -> Result<References, EditError> {
        let byte = self.mirror.byte_of(offset_utf16)?;
        let text_len = self.mirror.len_bytes();
        let mut i = self.stmts.index_of(byte);
        // The list partitions the text, so blank after a `;` is the next statement's head.
        let blank_head = |a: &Analyzer, i: usize| {
            let (s, e) = (a.stmts.list[i].start, a.stmts.end_of(i, text_len));
            let text = &a.mirror.as_str()[s..e];
            first_significant(text, a.lexer).is_none_or(|at| s + at > byte)
        };
        if blank_head(self, i) {
            if i == 0 {
                return Ok(self.no_references());
            }
            i -= 1;
        }
        let start = self.stmts.list[i].start;
        let end = self.stmts.end_of(i, text_len);
        let giant = self.stmts.is_giant(i, text_len);
        let mut out = self.no_references();
        out.statement_start_utf16 = self.mirror.utf16_of(start);
        out.statement_len_utf16 = self.mirror.utf16_of(end) - out.statement_start_utf16;

        let mut found = None;
        if !giant && !self.inactive {
            let statement = &self.mirror.as_str()[start..end];
            let stmt = &mut self.stmts.list[i];
            self.tick += 1;
            stmt.last_use = self.tick;
            let had_tree = stmt.tree.is_some();
            self.tree_bytes -= stmt.tree_cost;
            self.syntax.ensure(stmt, statement);
            self.tree_bytes += stmt.tree_cost;
            if let Some(tree) = stmt.tree.as_ref() {
                if !tree.root_node().has_error() {
                    found = Some(from_tree(tree.root_node(), statement));
                }
            }
            if !had_tree && self.tree_bytes > TREE_CACHE_BYTES {
                self.tree_bytes -= stmt.tree_cost;
                stmt.tree_cost = 0;
                stmt.tree = None;
            }
        }
        let (base, raws, ctes) = match found {
            Some((raws, ctes)) => (start, raws, ctes),
            None => {
                out.source = RefSource::Lexical;
                let (from, to) = self.window(start, end, byte, giant || self.inactive);
                let statement = &self.mirror.as_str()[from..to];
                (from, from_lex(statement, self.lexer), Vec::new())
            }
        };
        out.relations = raws
            .into_iter()
            .take(MAX_RELATIONS)
            .map(|r| Relation {
                kind: r.kind,
                catalog: r.catalog,
                schema: r.schema,
                name: r.name,
                quoted: r.quoted,
                alias: r.alias,
                name_start_utf16: self.mirror.utf16_of(base + r.at.0),
                name_len_utf16: self.mirror.utf16_of(base + r.at.1)
                    - self.mirror.utf16_of(base + r.at.0),
            })
            .collect();
        out.ctes = ctes;
        Ok(out)
    }

    fn no_references(&self) -> References {
        References {
            revision: self.revision,
            statement_start_utf16: 0,
            statement_len_utf16: 0,
            relations: Vec::new(),
            ctes: Vec::new(),
            source: RefSource::Tree,
        }
    }

    /// The bytes the lexical fallback reads: the statement, or for a giant one a stretch
    /// around `byte` on character boundaries.
    fn window(&self, start: usize, end: usize, byte: usize, cut: bool) -> (usize, usize) {
        if !cut || end - start <= 2 * WINDOW_BYTES {
            return (start, end);
        }
        let text = self.mirror.as_str();
        // The caret may sit past the statement (blank or comment after its `;`).
        let byte = byte.clamp(start, end);
        let mut from = byte.saturating_sub(WINDOW_BYTES).max(start);
        let mut to = (byte + WINDOW_BYTES).min(end);
        while !text.is_char_boundary(from) {
            from -= 1;
        }
        while !text.is_char_boundary(to) {
            to += 1;
        }
        (from, to)
    }
}

impl Document {
    /// The tables and aliases of the statement at `offset_utf16`. `revision` is the revision
    /// the caller last saw: an older one than the text's is `Stale`. Does not touch paint state.
    pub fn references(
        &mut self,
        revision: u64,
        offset_utf16: u32,
    ) -> Result<References, EditError> {
        if revision < self.revision() {
            return Err(EditError::Stale);
        }
        self.sync()?;
        self.analyzer_mut().references(offset_utf16)
    }
}

fn unquote(ident: &str) -> (String, bool) {
    let b = ident.as_bytes();
    if b.len() >= 2 {
        let q = b[0];
        if (q == b'"' || q == b'`') && b[b.len() - 1] == q {
            let inner = &ident[1..ident.len() - 1];
            let doubled = if q == b'"' { "\"\"" } else { "``" };
            return (inner.replace(doubled, &(q as char).to_string()), true);
        }
    }
    (ident.to_owned(), false)
}

fn ident_text(node: Node, text: &str) -> String {
    unquote(&text[node.byte_range()]).0
}

/// The table behind an `object_reference`.
fn table_of(node: Node, text: &str, alias: Option<String>) -> Option<Raw> {
    let name = node.child_by_field_name("name")?;
    let part = |field| node.child_by_field_name(field).map(|n| ident_text(n, text));
    Some(Raw {
        kind: RelationKind::Table,
        catalog: part("database"),
        schema: part("schema"),
        name: Some(ident_text(name, text)),
        quoted: unquote(&text[name.byte_range()]).1,
        alias,
        at: (name.start_byte(), name.end_byte()),
    })
}

/// Relations and CTEs of a statement's tree, in document order. `text` is the statement.
pub(crate) fn from_tree(root: Node, text: &str) -> (Vec<Raw>, Vec<Cte>) {
    let (mut raws, mut ctes) = (Vec::new(), Vec::new());
    let mut cursor = root.walk();
    'walk: loop {
        let node = cursor.node();
        match node.kind() {
            "relation" => {
                let alias = node
                    .child_by_field_name("alias")
                    .map(|n| ident_text(n, text));
                let mut walk = node.walk();
                let first = node
                    .named_children(&mut walk)
                    .find(|c| matches!(c.kind(), "object_reference" | "subquery"));
                match first {
                    Some(c) if c.kind() == "object_reference" => {
                        raws.extend(table_of(c, text, alias));
                    }
                    Some(c) => raws.push(Raw {
                        kind: RelationKind::Subquery,
                        catalog: None,
                        schema: None,
                        name: None,
                        quoted: false,
                        alias,
                        at: (c.start_byte(), c.end_byte()),
                    }),
                    None => {}
                }
            }
            "cte" => {
                let mut walk = node.walk();
                let name = node
                    .named_children(&mut walk)
                    .find(|c| c.kind() == "identifier");
                if let Some(name) = name {
                    let mut walk = node.walk();
                    ctes.push(Cte {
                        name: ident_text(name, text),
                        columns: node
                            .children_by_field_name("argument", &mut walk)
                            .map(|c| ident_text(c, text))
                            .collect(),
                    });
                }
            }
            "object_reference" => {
                // A table named outside a `relation`: `INSERT INTO t`, `DELETE FROM t`,
                // `TRUNCATE TABLE t`.
                let parent = node.parent();
                let lone = parent.is_some_and(|p| match p.kind() {
                    "insert" => true,
                    "from" => p.prev_named_sibling().is_some_and(|d| d.kind() == "delete"),
                    "statement" => {
                        let mut walk = p.walk();
                        let found = p.children(&mut walk).any(|c| c.kind() == "keyword_table");
                        found
                    }
                    _ => false,
                });
                if lone {
                    raws.extend(table_of(node, text, None));
                }
            }
            _ => {}
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    for raw in &mut raws {
        if raw.kind == RelationKind::Table
            && raw.catalog.is_none()
            && raw.schema.is_none()
            && ctes.iter().any(|c: &Cte| {
                raw.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&c.name))
            })
        {
            raw.kind = RelationKind::Cte;
        }
    }
    (raws, ctes)
}

/// Words that can follow a relation and are never its alias even where `is_keyword` is
/// unsure, plus the ones that end a name list.
fn is_alias(word: &str) -> bool {
    !is_keyword(word.as_bytes())
}

/// `FROM|JOIN|UPDATE|INTO|TABLE name[.name[.name]] [AS] [alias]`, and `, name [alias]` after a
/// `FROM` or `JOIN` relation, over the lexer. `text` is what is lexed; offsets are into it.
///
/// ponytail: sees neither subqueries nor CTEs, and a `FROM` inside a function call
/// (`extract(year from d)`) reads as a table. Both are harmless: an unknown name resolves to
/// nothing. Tighten if the tree path stops being the common case.
pub(crate) fn from_lex(text: &str, lexer: qh_sql::Lexer) -> Vec<Raw> {
    let mut tokens: Vec<Token> = Vec::new();
    lex(text.as_bytes(), 0..text.len(), lexer, |t| tokens.push(t));
    let src = |t: &Token| &text[t.start..t.end];
    let ident = |t: &Token| match t.kind {
        TokenKind::Word => Some((src(t).to_owned(), false)),
        TokenKind::Opaque(qh_sql::OpaqueKind::DoubleQuote | qh_sql::OpaqueKind::Backtick) => {
            let (s, q) = unquote(src(t));
            q.then_some((s, true))
        }
        _ => None,
    };
    let punct =
        |t: Option<&Token>, p: &str| t.is_some_and(|t| t.kind == TokenKind::Punct && src(t) == p);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        i += 1;
        if t.kind != TokenKind::Word {
            continue;
        }
        let lead = src(t).to_ascii_lowercase();
        let list = matches!(lead.as_str(), "from" | "join");
        if !(list || matches!(lead.as_str(), "update" | "into" | "table")) {
            continue;
        }
        loop {
            // `FROM ONLY t`, `UPDATE ONLY t`: the word is not the name.
            if tokens
                .get(i)
                .is_some_and(|t| t.kind == TokenKind::Word && src(t).eq_ignore_ascii_case("only"))
            {
                i += 1;
            }
            let mut parts: Vec<(String, bool, (usize, usize))> = Vec::new();
            while let Some((s, q)) = tokens.get(i).and_then(ident) {
                let t = &tokens[i];
                parts.push((s, q, (t.start, t.end)));
                i += 1;
                if punct(tokens.get(i), ".") {
                    i += 1;
                } else {
                    break;
                }
            }
            if parts.is_empty() || parts.len() > 3 || punct(tokens.get(i - 1), ".") {
                break;
            }
            let mut alias = None;
            let mut j = i;
            let mut with_as = false;
            if tokens
                .get(j)
                .is_some_and(|t| t.kind == TokenKind::Word && src(t).eq_ignore_ascii_case("as"))
            {
                j += 1;
                with_as = true;
            }
            if let Some(t) = tokens.get(j) {
                if let Some((s, q)) = ident(t) {
                    if with_as || q || is_alias(&s) {
                        alias = Some(s);
                        i = j + 1;
                    }
                }
            }
            let (last, quoted, at) = parts.pop().expect("one part at least");
            let mut it = parts.into_iter().map(|p| p.0);
            let (catalog, schema) = match (it.next(), it.next()) {
                (Some(a), Some(b)) => (Some(a), Some(b)),
                (a, _) => (None, a),
            };
            out.push(Raw {
                kind: RelationKind::Table,
                catalog,
                schema,
                name: Some(last),
                quoted,
                alias,
                at,
            });
            if !(list && punct(tokens.get(i), ",")) {
                break;
            }
            i += 1;
        }
    }
    out
}

/// The lexical fallback on `sql` alone, one line per relation as the tests show them.
#[doc(hidden)]
pub fn lexical_for_test(sql: &str, dialect: qh_sql::Dialect) -> Vec<String> {
    from_lex(sql, qh_sql::Lexer::from(dialect))
        .into_iter()
        .map(|x| {
            let part = |p: &Option<String>| p.clone().unwrap_or_else(|| "-".into());
            format!(
                "{:?} {}.{}.{} as {}{}",
                x.kind,
                part(&x.catalog),
                part(&x.schema),
                part(&x.name),
                part(&x.alias),
                if x.quoted { " quoted" } else { "" }
            )
        })
        .collect()
}
