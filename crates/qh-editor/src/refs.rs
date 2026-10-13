//! The tables a statement reads or writes, and the aliases it gives them: what column
//! completion needs to turn `a.` into a table.
//!
//! The scope is the statement holding the offset, from `qh_sql::walk`, never a tree. Its tree
//! (the one the colours use, parsed on demand and kept in the same cache) gives the relations;
//! a statement the grammar could not parse cleanly, one too big to parse and a document over
//! the colour ceiling fall back to a scan of the lexer's tokens that reads the same shapes:
//! `FROM|JOIN|UPDATE|INTO|TABLE name [AS] alias`, subqueries and CTEs with their column lists.
//! Nothing here runs per keystroke: the caller asks, in the background.
//!
//! The rule is that no suggestion beats a wrong one. A relation this cannot read (a function
//! call, a CTE with no column list) is left out, and an alias declared twice is reported
//! twice, so the caller can tell it is ambiguous.
//!
//! Two more things live here because they read the same tokens. [`scope_at_end`] says what a
//! caret sees: the clause it is in (a table position of a query or of a write, `ON`, `HAVING`,
//! …) and the names the select list gave. It takes the text before the caret and nothing else,
//! so it is cheap enough to ask on every keystroke. [`table_alias`] makes an alias for a table
//! name that is free in the statement and is never a keyword.

use qh_sql::{first_significant, lex, Token, TokenKind};
use tree_sitter::Node;

use crate::keywords::is_keyword;
use crate::paint::{Analyzer, Document};
use crate::statements::Statements;
use crate::text::EditError;
use crate::{GIANT_STATEMENT_BYTES, TREE_CACHE_BYTES};

/// Most relations reported for one statement.
pub const MAX_RELATIONS: usize = 64;

/// Bytes around the offset the lexical fallback reads in a statement too big to parse.
const WINDOW_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSource {
    /// Read from the statement's tree.
    Tree,
    /// Read by the lexical fallback (no tree, or a tree with errors). It reads the same shapes
    /// by their spelling, so it can be wrong where the grammar would know better (a name that
    /// is a CTE in one place and a table in another).
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

/// What the position at the caret is for, at the level of the query the caret is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Clause {
    /// None of the others: no clause yet, a clause this does not tell apart (`LIMIT`, `FOR
    /// UPDATE`), the inside of a column list or a function's arguments in a table position, a
    /// blank before the first word, or a statement too big to read.
    #[default]
    Other,
    /// The select list, or `RETURNING`.
    SelectList,
    /// A table is named here in a query: after `FROM` or `JOIN`, after a comma in that list, or
    /// after `USING` of a `DELETE`. A name here may take an alias.
    QueryTable,
    /// A table is named here by a statement that writes or defines it: `UPDATE t`, `DELETE FROM
    /// t`, `INSERT INTO t`, `TRUNCATE t`, `CREATE|ALTER|DROP TABLE t`. An alias is not wanted.
    WriteTable,
    /// `ON` or `USING` of a join.
    JoinCondition,
    Where,
    GroupBy,
    Having,
    OrderBy,
    /// `UPDATE … SET`.
    SetList,
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
                let (raws, ctes) = from_lex(statement, self.lexer);
                (from, raws, ctes)
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

/// The relation a `(select …)` in a `FROM` list makes.
fn subquery_of(node: Node, alias: Option<String>) -> Raw {
    Raw {
        kind: RelationKind::Subquery,
        catalog: None,
        schema: None,
        name: None,
        quoted: false,
        alias,
        at: (node.start_byte(), node.end_byte()),
    }
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
                    Some(c) => raws.push(subquery_of(c, alias)),
                    None => {}
                }
            }
            "lateral_join" => {
                // `JOIN LATERAL (select …) alias`: the grammar puts the subquery and its alias
                // here, not in a `relation`. A function call in that place is no relation.
                let alias = node
                    .child_by_field_name("alias")
                    .map(|n| ident_text(n, text));
                let mut walk = node.walk();
                let sub = node
                    .named_children(&mut walk)
                    .find(|c| c.kind() == "subquery");
                raws.extend(sub.map(|c| subquery_of(c, alias)));
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
    mark_ctes(&mut raws, &ctes);
    (raws, ctes)
}

/// A bare table name that a CTE of the statement declares is that CTE.
fn mark_ctes(raws: &mut [Raw], ctes: &[Cte]) {
    for raw in raws {
        if raw.kind == RelationKind::Table
            && raw.catalog.is_none()
            && raw.schema.is_none()
            && ctes.iter().any(|c| {
                raw.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&c.name))
            })
        {
            raw.kind = RelationKind::Cte;
        }
    }
}

/// The lexer's tokens of `text`, with every punctuation byte a token of its own. The lexer
/// cuts a run of punctuation as one token, which would hide a `(`, `)`, `,` or `.` inside `),`
/// or `((`.
fn tokens(text: &str, lexer: qh_sql::Lexer) -> Vec<Token> {
    let mut toks = Vec::new();
    lex(text.as_bytes(), 0..text.len(), lexer, |t| {
        if t.kind == TokenKind::Punct && t.end - t.start > 1 {
            toks.extend((t.start..t.end).map(|at| Token {
                start: at,
                end: at + 1,
                kind: TokenKind::Punct,
            }));
        } else {
            toks.push(t);
        }
    });
    toks
}

/// Whether the reader takes `word` for the alias of a relation: any word that is not a keyword.
fn is_alias(word: &str) -> bool {
    !is_keyword(word.as_bytes())
}

/// Parentheses the lexical reader descends into. Deeper text is skipped, not read: no real
/// statement nests subqueries that far, and the cap is what keeps the recursion off the stack
/// limit for text that is only brackets.
const MAX_DEPTH: usize = 32;

/// Relations and CTEs by the lexer's tokens alone. `text` is what is lexed; offsets are into it.
///
/// It reads `FROM|JOIN|UPDATE|INTO|TABLE name[.name[.name]] [AS] [alias]`, the `, name [alias]`
/// after a `FROM` or `JOIN` relation, `(select …) alias`, a parenthesised join group, and
/// `WITH [RECURSIVE] name [(columns)] AS [[NOT] MATERIALIZED] (…)`. A function call in a
/// relation position is no relation, and a `FROM` inside a call (`extract(year from d)`,
/// `substring(s from 2)`, `a is distinct from b`) introduces none.
pub(crate) fn from_lex(text: &str, lexer: qh_sql::Lexer) -> (Vec<Raw>, Vec<Cte>) {
    let toks = tokens(text, lexer);
    let mut close = vec![toks.len(); toks.len()];
    let mut open = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if t.kind == TokenKind::Punct {
            match text.as_bytes()[t.start] {
                b'(' => open.push(i),
                b')' => {
                    if let Some(o) = open.pop() {
                        close[o] = i;
                    }
                }
                _ => {}
            }
        }
    }
    let mut scan = Scan {
        text,
        toks,
        close,
        raws: Vec::new(),
        ctes: Vec::new(),
    };
    let end = scan.toks.len();
    scan.scan(0, end, 0, false);
    mark_ctes(&mut scan.raws, &scan.ctes);
    (scan.raws, scan.ctes)
}

/// The state of one [`from_lex`]: tokens with their matching parentheses, and what was read.
struct Scan<'a> {
    text: &'a str,
    toks: Vec<Token>,
    /// For a `(` token, the index of the `)` that closes it, or `toks.len()` when none does.
    close: Vec<usize>,
    raws: Vec<Raw>,
    ctes: Vec<Cte>,
}

impl Scan<'_> {
    fn src(&self, i: usize) -> &str {
        &self.text[self.toks[i].start..self.toks[i].end]
    }

    fn word(&self, i: usize, word: &str) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| t.kind == TokenKind::Word && self.src(i).eq_ignore_ascii_case(word))
    }

    fn punct(&self, i: usize, p: u8) -> bool {
        self.toks
            .get(i)
            .is_some_and(|t| t.kind == TokenKind::Punct && self.text.as_bytes()[t.start] == p)
    }

    /// A name part: a bare word, or a quoted identifier (`true` for quoted).
    fn ident(&self, i: usize) -> Option<(String, bool)> {
        let t = self.toks.get(i)?;
        match t.kind {
            TokenKind::Word => Some((self.src(i).to_owned(), false)),
            TokenKind::Opaque(qh_sql::OpaqueKind::DoubleQuote | qh_sql::OpaqueKind::Backtick) => {
                let (s, q) = unquote(self.src(i));
                q.then_some((s, true))
            }
            _ => None,
        }
    }

    /// Whether the `(` at `open` starts a query: `(select`, `((select`, `(with`, `(values`.
    fn opens_query(&self, open: usize) -> bool {
        let mut i = open + 1;
        while self.punct(i, b'(') {
            i += 1;
        }
        ["select", "with", "values", "table"]
            .iter()
            .any(|w| self.word(i, w))
    }

    /// The `)` of the `(` at `open`, held to `hi`.
    fn closer(&self, open: usize, hi: usize) -> usize {
        self.close[open].min(hi)
    }

    /// Reads `toks[i..hi]`. `call` says the range is inside a parenthesis that holds no query
    /// (a function call or a list), where `FROM` introduces nothing.
    fn scan(&mut self, mut i: usize, hi: usize, depth: usize, call: bool) {
        // Inside a FROM list, where a comma after a join condition (`… on a = b, c`) goes on to
        // the next relation.
        let mut in_from = false;
        while i < hi {
            if self.punct(i, b'(') {
                let c = self.closer(i, hi);
                if depth < MAX_DEPTH {
                    let call = !self.opens_query(i);
                    self.scan(i + 1, c, depth + 1, call);
                }
                i = c + 1;
                continue;
            }
            let at = i;
            i += 1;
            if self.punct(at, b',') && in_from {
                i = self.relations(i, hi, depth, true);
                continue;
            }
            if self.punct(at, b';') {
                in_from = false;
            }
            if self.toks[at].kind != TokenKind::Word {
                continue;
            }
            let lead = self.src(at).to_ascii_lowercase();
            match lead.as_str() {
                "with" => {
                    if let Some(next) = self.ctes_at(i, hi, depth) {
                        i = next;
                    }
                }
                "from" if call || (at > 0 && self.word(at - 1, "distinct")) => {}
                "from" | "join" => {
                    i = self.relations(i, hi, depth, true);
                    in_from = true;
                }
                // Not `FOR UPDATE OF t`, `ON DUPLICATE KEY UPDATE a = 1`, `DO UPDATE SET`,
                // `ON UPDATE CASCADE`.
                "update"
                    if at > 0
                        && ["for", "key", "do", "on"]
                            .iter()
                            .any(|w| self.word(at - 1, w)) =>
                {
                    in_from = false;
                }
                "update" | "into" | "table" => {
                    i = self.relations(i, hi, depth, false);
                    in_from = false;
                }
                // What ends the FROM list, so that a later comma is not one of its commas.
                "where" | "group" | "having" | "order" | "limit" | "offset" | "fetch" | "for"
                | "window" | "values" | "union" | "intersect" | "except" | "select"
                | "returning" | "set" => in_from = false,
                _ => {}
            }
        }
    }

    /// An optional `[AS] alias` at `i`, then an optional column list. Returns the alias and the
    /// index after both.
    fn alias(&self, mut i: usize, hi: usize) -> (Option<String>, usize) {
        let with_as = self.word(i, "as");
        let j = i + usize::from(with_as);
        let mut alias = None;
        if j < hi {
            if let Some((s, q)) = self.ident(j) {
                if with_as || q || is_alias(&s) {
                    alias = Some(s);
                    i = j + 1;
                }
            }
        }
        if alias.is_some() && self.punct(i, b'(') {
            i = self.closer(i, hi) + 1;
        }
        (alias, i.min(hi))
    }

    /// The relation list that starts at `toks[i]`, after a `FROM`, `JOIN`, `UPDATE`, `INTO` or
    /// `TABLE`. A comma continues the list only after `FROM` or `JOIN` (`list`). Returns the
    /// index after what was read.
    fn relations(&mut self, mut i: usize, hi: usize, depth: usize, list: bool) -> usize {
        loop {
            // `FROM ONLY t`, `JOIN LATERAL (…)`: the word is not the name.
            while i < hi && (self.word(i, "only") || self.word(i, "lateral")) {
                i += 1;
            }
            // `CREATE TABLE IF NOT EXISTS t`, `DROP TABLE IF EXISTS t`.
            if self.word(i, "if") {
                let j = i + 1 + usize::from(self.word(i + 1, "not"));
                if self.word(j, "exists") {
                    i = j + 1;
                }
            }
            if i >= hi {
                return hi;
            }
            if self.punct(i, b'(') {
                let c = self.closer(i, hi);
                let last = if c < hi { c } else { hi - 1 };
                let query = self.opens_query(i);
                let mut at = None;
                if query {
                    at = Some(self.raws.len());
                    self.raws.push(Raw {
                        kind: RelationKind::Subquery,
                        catalog: None,
                        schema: None,
                        name: None,
                        quoted: false,
                        alias: None,
                        at: (self.toks[i].start, self.toks[last].end),
                    });
                }
                if depth < MAX_DEPTH {
                    if query {
                        self.scan(i + 1, c, depth + 1, false);
                    } else {
                        // A join group: `(a join b on …)` holds a relation list of its own.
                        let next = self.relations(i + 1, c, depth + 1, true);
                        self.scan(next, c, depth + 1, false);
                    }
                }
                let (alias, next) = self.alias(c + 1, hi);
                if let Some(at) = at {
                    self.raws[at].alias = alias;
                }
                i = next;
            } else {
                let mut parts: Vec<(String, bool, (usize, usize))> = Vec::new();
                while let Some((s, q)) = self.ident(i).filter(|_| i < hi) {
                    let t = self.toks[i];
                    parts.push((s, q, (t.start, t.end)));
                    i += 1;
                    if self.punct(i, b'.') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if parts.is_empty() || parts.len() > 3 || self.punct(i - 1, b'.') {
                    return i;
                }
                if list && self.punct(i, b'(') {
                    // A function call: `generate_series(1, 2) g`, `unnest(a) with ordinality
                    // as o(n, i)`. Not a table, but a query inside its arguments is read.
                    let c = self.closer(i, hi);
                    if depth < MAX_DEPTH {
                        self.scan(i + 1, c, depth + 1, true);
                    }
                    i = c + 1;
                    if self.word(i, "with") && self.word(i + 1, "ordinality") {
                        i += 2;
                    }
                    i = self.alias(i, hi).1;
                } else {
                    let (alias, next) = self.alias(i, hi);
                    i = next;
                    let (last, quoted, at) = parts.pop().expect("one part at least");
                    let mut it = parts.into_iter().map(|p| p.0);
                    let (catalog, schema) = match (it.next(), it.next()) {
                        (Some(a), Some(b)) => (Some(a), Some(b)),
                        (a, _) => (None, a),
                    };
                    self.raws.push(Raw {
                        kind: RelationKind::Table,
                        catalog,
                        schema,
                        name: Some(last),
                        quoted,
                        alias,
                        at,
                    });
                }
            }
            if !(list && self.punct(i, b',')) {
                return i.min(hi);
            }
            i += 1;
        }
    }

    /// `[RECURSIVE] name [(columns)] AS [[NOT] MATERIALIZED] (…)`, repeated after commas, at
    /// `toks[i]` just after a `WITH`. `None` when the `WITH` heads no CTE (`WITH ORDINALITY`,
    /// `WITH (fillfactor = 70)`, `WITH TIME ZONE`, `WITH CHECK OPTION`). Returns the index after
    /// the last definition.
    fn ctes_at(&mut self, mut i: usize, hi: usize, depth: usize) -> Option<usize> {
        if self.word(i, "recursive") {
            i += 1;
        }
        let mut found = false;
        while i < hi {
            let Some((name, _)) = self.ident(i) else {
                break;
            };
            let mut j = i + 1;
            let mut columns = Vec::new();
            if self.punct(j, b'(') {
                let c = self.close[j];
                if c >= hi {
                    break;
                }
                columns = (j + 1..c)
                    .filter_map(|k| self.ident(k).map(|p| p.0))
                    .collect();
                j = c + 1;
            }
            if !self.word(j, "as") {
                break;
            }
            j += 1;
            if self.word(j, "not") {
                j += 1;
            }
            if self.word(j, "materialized") {
                j += 1;
            }
            if !self.punct(j, b'(') {
                break;
            }
            let c = self.closer(j, hi);
            self.ctes.push(Cte { name, columns });
            found = true;
            if depth < MAX_DEPTH {
                self.scan(j + 1, c, depth + 1, false);
            }
            i = c + 1;
            if self.punct(i, b',') {
                i += 1;
            } else {
                break;
            }
        }
        found.then_some(i.min(hi))
    }
}

/// What the caret sees in its statement: the clause it is in, and the output names the select
/// list gave, which `ORDER BY` and sometimes `GROUP BY` and `HAVING` may use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub clause: Clause,
    /// Aliases written with `AS` in the select list of the query the caret is in, as spelled
    /// (a quoted one keeps its quotes), in order. The first `SELECT` of a `UNION` only. An
    /// alias written without `AS` is not read.
    pub select_aliases: Vec<String>,
}

/// The [`Scope`] at the end of `head`: the text before the caret, or before the word being
/// completed, so a half-typed `fro` does not decide its own context. The statement is the last
/// one in `head`. A caret after a `;` and blanks, in a statement too big to read, or before any
/// word has the default scope: [`Clause::Other`] and no aliases.
///
/// A function of the text alone, with no document and no lock: the editor asks it on every
/// keystroke with a stretch of text before the caret (a few KB are enough), and a stretch that
/// starts inside a statement reads that statement from where it starts.
pub fn scope_at_end(head: &str, dialect: qh_sql::Dialect) -> Scope {
    let lexer = qh_sql::Lexer::from(dialect);
    let statements = Statements::new(head, lexer);
    let Some(last) = statements.list.last() else {
        return Scope::default();
    };
    let text = &head[last.start..];
    if text.len() > GIANT_STATEMENT_BYTES || first_significant(text, lexer).is_none() {
        return Scope::default();
    }
    scope_of(text, lexer)
}

/// One level of parentheses while `scope_of` reads: the statement itself, a query in
/// parentheses, or anything else in parentheses.
#[derive(Default)]
struct Level {
    clause: Clause,
    /// An `UPDATE` or a `DELETE` was seen at this level.
    update: bool,
    delete: bool,
    /// Inside parentheses that hold no query, where a `FROM` is part of a call.
    call: bool,
    aliases: Vec<String>,
    /// A `UNION`, `INTERSECT` or `EXCEPT` ended the select list that names the output.
    closed: bool,
}

/// Parentheses `scope_of` tracks; deeper ones are counted and not entered.
const MAX_LEVELS: usize = 64;

/// Aliases kept per statement level.
const MAX_ALIASES: usize = 64;

/// The scope at the end of `text`, one statement's head. See [`Scope`].
fn scope_of(text: &str, lexer: qh_sql::Lexer) -> Scope {
    let toks = tokens(text, lexer);
    let punct = |i: usize, p: u8| {
        toks.get(i)
            .is_some_and(|t| t.kind == TokenKind::Punct && text.as_bytes()[t.start] == p)
    };
    let word = |i: usize| -> Option<String> {
        toks.get(i)
            .filter(|t| t.kind == TokenKind::Word)
            .map(|t| text[t.start..t.end].to_ascii_lowercase())
    };
    let mut levels = vec![Level::default()];
    let mut overflow = 0usize;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            TokenKind::Punct => match text.as_bytes()[t.start] {
                b';' => {
                    levels = vec![Level::default()];
                    overflow = 0;
                }
                b')' if overflow > 0 => overflow -= 1,
                b')' => {
                    if levels.len() > 1 {
                        levels.pop();
                    }
                }
                b'(' if levels.len() >= MAX_LEVELS => overflow += 1,
                b'(' => {
                    // A query in parentheses starts its own clauses, `((select` included.
                    let mut j = i + 1;
                    while punct(j, b'(') {
                        j += 1;
                    }
                    let query = word(j).is_some_and(|w| {
                        matches!(w.as_str(), "select" | "with" | "values" | "table")
                    });
                    let top = levels.last().expect("one level at least");
                    levels.push(if query {
                        Level::default()
                    } else {
                        // Other parentheses keep the clause, except after a table's name, where
                        // they hold a column list or a call: `INSERT INTO t (`, `FROM f(`. After
                        // `FROM`, `JOIN` or a comma they hold a join group, and keep it.
                        let named = i.checked_sub(1).and_then(word).is_some_and(|w| {
                            !matches!(
                                w.as_str(),
                                "from"
                                    | "join"
                                    | "into"
                                    | "update"
                                    | "table"
                                    | "only"
                                    | "lateral"
                                    | "using"
                            )
                        });
                        let table = matches!(top.clause, Clause::QueryTable | Clause::WriteTable);
                        Level {
                            clause: if table && named {
                                Clause::Other
                            } else {
                                top.clause
                            },
                            call: true,
                            ..Level::default()
                        }
                    });
                }
                _ => {}
            },
            TokenKind::Word if overflow == 0 => {
                let w = text[t.start..t.end].to_ascii_lowercase();
                let prev = i.checked_sub(1).and_then(word);
                let top = levels.last_mut().expect("one level at least");
                match w.as_str() {
                    // `a AS x` in the select list, not `cast(a as int)`.
                    "as" if top.clause == Clause::SelectList
                        && !top.call
                        && !top.closed
                        && top.aliases.len() < MAX_ALIASES =>
                    {
                        let named = toks.get(i + 1).filter(|n| {
                            matches!(
                                n.kind,
                                TokenKind::Word
                                    | TokenKind::Opaque(
                                        qh_sql::OpaqueKind::DoubleQuote
                                            | qh_sql::OpaqueKind::Backtick
                                    )
                            )
                        });
                        top.aliases
                            .extend(named.map(|n| text[n.start..n.end].to_owned()));
                    }
                    "select" | "returning" => top.clause = Clause::SelectList,
                    // `extract(year from d)`, `a is distinct from b`.
                    "from" if top.call || prev.as_deref() == Some("distinct") => {}
                    "from" if top.delete => top.clause = Clause::WriteTable,
                    "from" => top.clause = Clause::QueryTable,
                    "join" if !top.call => top.clause = Clause::QueryTable,
                    "using" if top.delete => top.clause = Clause::QueryTable,
                    "using" if top.clause == Clause::QueryTable => {
                        top.clause = Clause::JoinCondition
                    }
                    // `INSERT … ON CONFLICT`.
                    "on" if word(i + 1).as_deref() == Some("conflict") => {
                        top.clause = Clause::Other
                    }
                    "on" if top.clause == Clause::QueryTable => top.clause = Clause::JoinCondition,
                    "where" => top.clause = Clause::Where,
                    "group" => top.clause = Clause::GroupBy,
                    "having" => top.clause = Clause::Having,
                    "order" => top.clause = Clause::OrderBy,
                    "union" | "intersect" | "except" => {
                        top.clause = Clause::Other;
                        top.closed = true;
                    }
                    "limit" | "offset" | "fetch" | "for" | "window" | "values" => {
                        top.clause = Clause::Other;
                    }
                    "set" if top.update => top.clause = Clause::SetList,
                    // Not `FOR UPDATE`, `ON DUPLICATE KEY UPDATE`, `ON CONFLICT DO UPDATE`.
                    // Nor `ON UPDATE CASCADE`.
                    "update" if !matches!(prev.as_deref(), Some("for" | "key" | "do" | "on")) => {
                        top.update = true;
                        top.clause = Clause::WriteTable;
                    }
                    "delete" if prev.as_deref() != Some("on") => top.delete = true,
                    "into" | "truncate" => top.clause = Clause::WriteTable,
                    "table"
                        if matches!(
                            prev.as_deref(),
                            Some("create" | "alter" | "drop" | "truncate")
                        ) =>
                    {
                        top.clause = Clause::WriteTable;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    match levels.pop() {
        Some(level) if overflow == 0 => Scope {
            clause: level.clause,
            select_aliases: level.aliases,
        },
        _ => Scope::default(),
    }
}

/// Words the grammar has no keyword for that are reserved in practice, so `table_alias` never
/// makes one: `div`, `mod` and `xor` (MySQL), `qualify`, `top` and `sample`.
const RESERVED_IN_PRACTICE: &[&str] = &["div", "mod", "qualify", "sample", "top", "xor"];

/// An alias for `table` that is free in the statement and safe to write: made of the name's
/// initials or leading letters (`order_items` is `oi`, `users` is `u`), never a keyword, never
/// one of `taken` (aliases and table names already in the statement, compared without regard to
/// case), always ASCII letters and digits starting with a letter. Falls back to a number
/// (`u2`) when the short forms are all in use.
///
/// A keyword is refused by the same list the reader uses to refuse a keyword as an alias, plus
/// [`RESERVED_IN_PRACTICE`], so what this makes always reads back as an alias.
pub fn table_alias<S: AsRef<str>>(table: &str, taken: &[S]) -> String {
    // Words by `_`, `-`, digits-to-letters and `aB` boundaries, ASCII letters kept.
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in table.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = ch.is_ascii_lowercase();
        current.push(ch.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    // Digits lead nowhere in an identifier: a word counts by its letters.
    let words: Vec<String> = words
        .into_iter()
        .map(|w| {
            w.trim_start_matches(|c: char| c.is_ascii_digit())
                .to_owned()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let first = words.first().cloned().unwrap_or_else(|| "t".to_owned());
    let mut candidates: Vec<String> = Vec::new();
    if words.len() > 1 {
        candidates.push(words.iter().filter_map(|w| w.chars().next()).collect());
    }
    for n in 1..=3 {
        candidates.push(first.chars().take(n).collect());
    }
    let own = table.to_ascii_lowercase();
    let is_free = |a: &str| {
        !a.is_empty()
            && a != own
            && !is_keyword(a.as_bytes())
            && !RESERVED_IN_PRACTICE
                .iter()
                .any(|w| a.eq_ignore_ascii_case(w))
            && !taken.iter().any(|t| t.as_ref().eq_ignore_ascii_case(a))
    };
    if let Some(found) = candidates.iter().find(|a| is_free(a)) {
        return found.clone();
    }
    let base = &candidates[0];
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|a| is_free(a))
        .expect("an unbounded range has a free number")
}

/// The lexical fallback on `sql` alone, one line per relation as the tests show them.
#[doc(hidden)]
pub fn lexical_for_test(sql: &str, dialect: qh_sql::Dialect) -> Vec<String> {
    from_lex(sql, qh_sql::Lexer::from(dialect))
        .0
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
