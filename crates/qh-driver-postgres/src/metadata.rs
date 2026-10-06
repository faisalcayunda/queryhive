//! PostgreSQL metadata: object kinds, columns, and a DDL rebuilt from the catalog.
//!
//! PostgreSQL has no `SHOW CREATE`, so the DDL is **assembled** here from what the server prints
//! for each part (`format_type`, `pg_get_expr`, `pg_get_constraintdef`, `pg_get_indexdef`,
//! `quote_ident`), which keeps the text the server's own rather than a rewrite of it. The result is
//! a reconstruction, not `pg_dump`, and its first line says so.
//!
//! These statements read `pg_class` and not `information_schema`, because
//! `information_schema.tables` leaves materialized views out entirely. The predicate that stands
//! in for its visibility rule is [`VISIBLE`], one constant shared by every statement, so the tree
//! never lists an object the role may not touch.
//!
//! The statements are built here and run by the engine through `Session::execute`; nothing in this
//! file does I/O, so each one is pinned by a test without a server.

use qh_core::EngineError;
use qh_driver::{
    required_part, ColumnInfo, DdlRecipe, MetadataSql, ObjectDdl, ObjectKind, ObjectPath, Rows,
    Step, TableEntry,
};

use crate::quote_literal;

/// The metadata statements of PostgreSQL.
pub struct PostgresMetadata;

/// Which relations a role can see, written so the set equals `information_schema.tables`' own
/// (5,213 rows against 5,213 on the dev server). Without it the tree would list objects the role
/// has no privilege on.
const VISIBLE: &str = "(pg_has_role(c.relowner, 'USAGE') \
     OR has_table_privilege(c.oid, 'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER') \
     OR has_any_column_privilege(c.oid, 'SELECT, INSERT, UPDATE, REFERENCES'))";

/// Tables, partitioned tables, views, materialized views and foreign tables.
const RELKINDS: &str = "('r','p','v','m','f')";

/// The first line of every DDL this file writes: what the text is, and what it leaves out.
const NOTE: &str = "-- Reconstructed from the catalog by QueryHive. Storage options, tablespace, \
     ownership, grants, comments, triggers and rules are not included.";

fn target_of(target: &ObjectPath) -> Result<(&str, &str), EngineError> {
    let schema = required_part(&target.schema, "TARGET_SCHEMA", "to name the object")?;
    let table = required_part(&target.table, "TARGET_TABLE", "to name the object")?;
    Ok((schema, table))
}

/// The column statement. `name` is what the first cell selects (`attname` as text for the grid,
/// `quote_ident` of it for DDL), and `flags` are the extra cells DDL needs.
fn column_sql(schema: &str, table: &str, name: &str, last: &str) -> String {
    format!(
        "SELECT {name}, format_type(a.atttypid, a.atttypmod), \
         CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, \
         pg_get_expr(d.adbin, d.adrelid), {last} \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE n.nspname = {} AND c.relname = {} AND c.relkind IN {RELKINDS} \
         AND {VISIBLE} AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum",
        quote_literal(schema),
        quote_literal(table),
    )
}

impl MetadataSql for PostgresMetadata {
    fn table_entries(&self, path: &ObjectPath) -> Result<String, EngineError> {
        let schema = required_part(&path.schema, "DB_SCHEMA", "to list tables on PostgreSQL")?;
        Ok(format!(
            "SELECT c.relname::text, \
             CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'table' WHEN 'v' THEN 'view' \
             WHEN 'm' THEN 'materialized_view' WHEN 'f' THEN 'foreign_table' END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {} AND c.relkind IN {RELKINDS} AND {VISIBLE} \
             ORDER BY c.relname",
            quote_literal(schema)
        ))
    }

    fn parse_table_entries(&self, rows: &Rows) -> Result<Vec<TableEntry>, EngineError> {
        (0..rows.rows.len())
            .map(|row| {
                Ok(TableEntry {
                    name: rows.required(row, 0)?.to_owned(),
                    kind: rows.cell(row, 1).and_then(ObjectKind::parse),
                })
            })
            .collect()
    }

    fn columns(&self, target: &ObjectPath) -> Result<String, EngineError> {
        let (schema, table) = target_of(target)?;
        Ok(column_sql(
            schema,
            table,
            "a.attname::text",
            "CASE WHEN a.attidentity <> '' THEN 'identity' \
             WHEN a.attgenerated <> '' THEN 'generated' ELSE '' END",
        ))
    }

    fn parse_columns(&self, rows: &Rows) -> Result<Vec<ColumnInfo>, EngineError> {
        (0..rows.rows.len())
            .map(|row| {
                Ok(ColumnInfo {
                    name: rows.required(row, 0)?.to_owned(),
                    data_type: rows.required(row, 1)?.to_owned(),
                    nullable: yes_no(rows.cell(row, 2)),
                    default: rows.cell(row, 3).map(str::to_owned),
                    extra: rows.cell(row, 4).unwrap_or_default().to_owned(),
                })
            })
            .collect()
    }

    fn ddl(&self, target: &ObjectPath) -> Result<Box<dyn DdlRecipe>, EngineError> {
        let (schema, table) = target_of(target)?;
        Ok(Box::new(PostgresDdl::new(schema, table)))
    }
}

fn yes_no(cell: Option<&str>) -> Option<bool> {
    match cell {
        Some("YES") => Some(true),
        Some("NO") => Some(false),
        _ => None,
    }
}

/// What the head statement said about the relation.
struct Head {
    relkind: String,
    /// `quote_ident` of the schema and of the name, joined with a dot, as the server wrote them.
    name: String,
    view_definition: Option<String>,
    partition_key: Option<String>,
    partition_bound: Option<String>,
    parent: Option<String>,
}

/// One column as the DDL needs it: the server's own spelling of every part.
struct DdlColumn {
    name: String,
    data_type: String,
    not_null: bool,
    expression: Option<String>,
    identity: String,
    generated: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Head,
    Columns,
    Constraints,
    Indexes,
}

/// The DDL of one relation, in up to four statements.
pub struct PostgresDdl {
    schema: String,
    table: String,
    stage: Stage,
    head: Option<Head>,
    columns: Vec<DdlColumn>,
    constraints: Vec<String>,
}

impl PostgresDdl {
    fn new(schema: &str, table: &str) -> Self {
        Self {
            schema: schema.to_owned(),
            table: table.to_owned(),
            stage: Stage::Head,
            head: None,
            columns: Vec::new(),
            constraints: Vec::new(),
        }
    }

    fn head_sql(&self) -> String {
        format!(
            "SELECT c.relkind::text, quote_ident(n.nspname), quote_ident(c.relname), \
             CASE WHEN c.relkind IN ('v','m') THEN pg_get_viewdef(c.oid, true) END, \
             CASE WHEN c.relkind = 'p' THEN pg_get_partkeydef(c.oid) END, \
             CASE WHEN c.relispartition THEN pg_get_expr(c.relpartbound, c.oid) END, \
             (SELECT quote_ident(pn.nspname) || '.' || quote_ident(pc.relname) \
              FROM pg_inherits i JOIN pg_class pc ON pc.oid = i.inhparent \
              JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
              WHERE i.inhrelid = c.oid AND c.relispartition) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {} AND c.relname = {} AND c.relkind IN {RELKINDS} AND {VISIBLE}",
            quote_literal(&self.schema),
            quote_literal(&self.table),
        )
    }

    fn columns_sql(&self) -> String {
        column_sql(
            &self.schema,
            &self.table,
            "quote_ident(a.attname)",
            "a.attidentity::text, a.attgenerated::text",
        )
    }

    fn constraints_sql(&self) -> String {
        format!(
            "SELECT quote_ident(co.conname), co.contype::text, pg_get_constraintdef(co.oid, true) \
             FROM pg_constraint co JOIN pg_class c ON c.oid = co.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = {} AND c.relname = {} AND co.contype IN ('p','u','f','c','x') \
             ORDER BY CASE co.contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'x' THEN 2 \
             WHEN 'c' THEN 3 ELSE 4 END, co.conname",
            quote_literal(&self.schema),
            quote_literal(&self.table),
        )
    }

    /// Indexes no constraint owns. An index that is a partition of a parent's index is left out:
    /// creating the partition creates it.
    fn indexes_sql(&self) -> String {
        format!(
            "SELECT pg_get_indexdef(x.indexrelid) \
             FROM pg_index x JOIN pg_class t ON t.oid = x.indrelid \
             JOIN pg_class i ON i.oid = x.indexrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE n.nspname = {} AND t.relname = {} AND NOT i.relispartition \
             AND NOT EXISTS (SELECT 1 FROM pg_constraint co WHERE co.conindid = x.indexrelid) \
             ORDER BY i.relname",
            quote_literal(&self.schema),
            quote_literal(&self.table),
        )
    }

    fn read_head(&mut self, rows: &Rows) -> Result<Step, EngineError> {
        if rows.rows.is_empty() {
            return Err(EngineError::Usage {
                message: format!(
                    "{}.{} was not found, or this role has no privilege on it",
                    self.schema, self.table
                ),
            });
        }
        let head = Head {
            relkind: rows.required(0, 0)?.to_owned(),
            name: format!("{}.{}", rows.required(0, 1)?, rows.required(0, 2)?),
            view_definition: rows.cell(0, 3).map(str::to_owned),
            partition_key: rows.cell(0, 4).map(str::to_owned),
            partition_bound: rows.cell(0, 5).map(str::to_owned),
            parent: rows.cell(0, 6).map(str::to_owned),
        };
        match head.relkind.as_str() {
            "v" | "m" => {
                let materialized = head.relkind == "m";
                let definition =
                    head.view_definition
                        .as_deref()
                        .ok_or_else(|| EngineError::Internal {
                            message: "the catalog gave a view no definition".to_owned(),
                        })?;
                Ok(Step::Done(ObjectDdl {
                    kind: Some(if materialized {
                        ObjectKind::MaterializedView
                    } else {
                        ObjectKind::View
                    }),
                    text: view_ddl(&head.name, materialized, definition),
                    truncated: false,
                }))
            }
            "f" => Err(EngineError::Usage {
                message: "DDL for foreign tables is not reconstructed".to_owned(),
            }),
            _ => {
                // A partition is written as `PARTITION OF`, which replaces the column list, so its
                // columns and constraints are not asked for.
                let next = if head.parent.is_some() {
                    self.stage = Stage::Indexes;
                    self.indexes_sql()
                } else {
                    self.stage = Stage::Columns;
                    self.columns_sql()
                };
                self.head = Some(head);
                Ok(Step::Query(next))
            }
        }
    }

    fn read_columns(&mut self, rows: &Rows) -> Result<(), EngineError> {
        self.columns = (0..rows.rows.len())
            .map(|row| {
                Ok(DdlColumn {
                    name: rows.required(row, 0)?.to_owned(),
                    data_type: rows.required(row, 1)?.to_owned(),
                    not_null: rows.cell(row, 2) == Some("NO"),
                    expression: rows.cell(row, 3).map(str::to_owned),
                    identity: rows.cell(row, 4).unwrap_or_default().to_owned(),
                    generated: rows.cell(row, 5).unwrap_or_default().to_owned(),
                })
            })
            .collect::<Result<_, EngineError>>()?;
        Ok(())
    }

    fn read_constraints(&mut self, rows: &Rows) -> Result<(), EngineError> {
        self.constraints = (0..rows.rows.len())
            .map(|row| {
                Ok(format!(
                    "CONSTRAINT {} {}",
                    rows.required(row, 0)?,
                    rows.required(row, 2)?
                ))
            })
            .collect::<Result<_, EngineError>>()?;
        Ok(())
    }

    fn finish(&self, indexes: &Rows) -> Result<Step, EngineError> {
        let head = self.head.as_ref().ok_or_else(|| EngineError::Internal {
            message: "the DDL recipe finished before it had read the relation".to_owned(),
        })?;
        let mut text = format!("{NOTE}\n");
        match (&head.parent, &head.partition_bound) {
            (Some(parent), Some(bound)) => {
                text.push_str(&format!(
                    "CREATE TABLE {} PARTITION OF {parent} {bound}",
                    head.name
                ));
                if let Some(key) = &head.partition_key {
                    text.push_str(&format!(" PARTITION BY {key}"));
                }
            }
            _ => {
                let mut lines: Vec<String> = self.columns.iter().map(column_line).collect();
                lines.extend(self.constraints.iter().map(|line| format!("    {line}")));
                if lines.is_empty() {
                    text.push_str(&format!("CREATE TABLE {} ()", head.name));
                } else {
                    text.push_str(&format!(
                        "CREATE TABLE {} (\n{}\n)",
                        head.name,
                        lines.join(",\n")
                    ));
                }
                if let Some(key) = &head.partition_key {
                    text.push_str(&format!(" PARTITION BY {key}"));
                }
            }
        }
        text.push_str(";\n");
        for row in 0..indexes.rows.len() {
            text.push_str(indexes.required(row, 0)?);
            text.push_str(";\n");
        }
        Ok(Step::Done(ObjectDdl {
            kind: Some(ObjectKind::Table),
            text,
            truncated: false,
        }))
    }
}

impl DdlRecipe for PostgresDdl {
    fn next(&mut self, previous: Option<Rows>) -> Result<Step, EngineError> {
        let Some(rows) = previous else {
            self.stage = Stage::Head;
            return Ok(Step::Query(self.head_sql()));
        };
        match self.stage {
            Stage::Head => self.read_head(&rows),
            Stage::Columns => {
                self.read_columns(&rows)?;
                self.stage = Stage::Constraints;
                Ok(Step::Query(self.constraints_sql()))
            }
            Stage::Constraints => {
                self.read_constraints(&rows)?;
                self.stage = Stage::Indexes;
                Ok(Step::Query(self.indexes_sql()))
            }
            Stage::Indexes => self.finish(&rows),
        }
    }
}

/// `CREATE [MATERIALIZED] VIEW name AS` and the definition, without the leading space and the
/// closing `;` `pg_get_viewdef` puts on it.
fn view_ddl(name: &str, materialized: bool, definition: &str) -> String {
    let body = definition.trim().trim_end_matches(';').trim_end();
    format!(
        "{NOTE}\nCREATE {}VIEW {name} AS\n{body};\n",
        if materialized { "MATERIALIZED " } else { "" }
    )
}

/// One column line: the name, the type, then identity or generated, the default, `NOT NULL`.
///
/// A generated column's expression is in `pg_attrdef` beside real defaults, and is written as the
/// `GENERATED` clause it belongs to, never as a `DEFAULT`.
fn column_line(column: &DdlColumn) -> String {
    let mut line = format!("    {} {}", column.name, column.data_type);
    match column.identity.as_str() {
        "a" => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
        "d" => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
        _ => {}
    }
    match (&column.expression, column.generated.as_str()) {
        (Some(expression), "s") => line.push_str(&format!(
            " GENERATED ALWAYS AS {} STORED",
            wrapped(expression)
        )),
        (Some(expression), "v") => line.push_str(&format!(
            " GENERATED ALWAYS AS {} VIRTUAL",
            wrapped(expression)
        )),
        (Some(expression), _) if column.identity.is_empty() => {
            line.push_str(&format!(" DEFAULT {expression}"))
        }
        _ => {}
    }
    if column.not_null {
        line.push_str(" NOT NULL");
    }
    line
}

/// The expression in parentheses, once: `pg_get_expr` already wraps most of them.
fn wrapped(expression: &str) -> String {
    if expression.starts_with('(') && expression.ends_with(')') {
        expression.to_owned()
    } else {
        format!("({expression})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(schema: &str, table: &str) -> ObjectPath {
        ObjectPath::new().schema(schema).table(table)
    }

    fn rows(cells: &[&[Option<&str>]]) -> Rows {
        Rows {
            columns: Vec::new(),
            rows: cells
                .iter()
                .map(|row| row.iter().map(|cell| cell.map(str::to_owned)).collect())
                .collect(),
        }
    }

    fn some(text: &str) -> Option<&str> {
        Some(text)
    }

    #[test]
    fn the_object_list_statement_is_pinned() {
        assert_eq!(
            PostgresMetadata
                .table_entries(&ObjectPath::new().schema("analytics"))
                .unwrap(),
            format!(
                "SELECT c.relname::text, \
                 CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'table' WHEN 'v' THEN 'view' \
                 WHEN 'm' THEN 'materialized_view' WHEN 'f' THEN 'foreign_table' END \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = 'analytics' AND c.relkind IN ('r','p','v','m','f') AND {VISIBLE} \
                 ORDER BY c.relname"
            )
        );
    }

    #[test]
    fn the_column_statement_is_pinned() {
        assert_eq!(
            PostgresMetadata
                .columns(&path("analytics", "people"))
                .unwrap(),
            format!(
                "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
                 CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, \
                 pg_get_expr(d.adbin, d.adrelid), \
                 CASE WHEN a.attidentity <> '' THEN 'identity' \
                 WHEN a.attgenerated <> '' THEN 'generated' ELSE '' END \
                 FROM pg_attribute a \
                 JOIN pg_class c ON c.oid = a.attrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
                 WHERE n.nspname = 'analytics' AND c.relname = 'people' \
                 AND c.relkind IN ('r','p','v','m','f') AND {VISIBLE} \
                 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum"
            )
        );
    }

    #[test]
    fn the_visibility_predicate_is_the_one_the_blueprint_measured() {
        assert_eq!(
            VISIBLE,
            "(pg_has_role(c.relowner, 'USAGE') OR has_table_privilege(c.oid, 'SELECT, INSERT, \
             UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER') OR has_any_column_privilege(c.oid, \
             'SELECT, INSERT, UPDATE, REFERENCES'))"
        );
    }

    #[test]
    fn a_dangerous_name_stays_inside_its_literal_in_every_statement() {
        // Every statement, for a name that tries each way out of a literal and a comment.
        for name in [
            "a'b",
            "'; DROP TABLE t; --",
            "a\\",
            "a\\'; DROP TABLE t; --",
            "\"x\"",
            "`y`",
        ] {
            let target = path(name, name);
            let mut statements = vec![
                PostgresMetadata
                    .table_entries(&ObjectPath::new().schema(name))
                    .unwrap(),
                PostgresMetadata.columns(&target).unwrap(),
            ];
            let mut recipe = PostgresMetadata.ddl(&target).unwrap();
            let Step::Query(head) = recipe.next(None).unwrap() else {
                panic!("the recipe starts with a statement")
            };
            statements.push(head);
            let literal = quote_literal(name);
            for sql in statements {
                assert!(sql.contains(&literal), "{name:?}: {sql}");
            }
        }
    }

    #[test]
    fn a_backslash_is_escaped_for_either_string_setting() {
        // `E'…'` reads a backslash as an escape whatever `standard_conforming_strings` says,
        // so a name ending in one cannot swallow the closing quote.
        assert_eq!(quote_literal("a\\b"), "E'a\\\\b'");
        assert_eq!(quote_literal("a\\'"), "E'a\\\\'''");
        assert_eq!(quote_literal("plain"), "'plain'");
    }

    #[test]
    fn a_missing_part_of_the_name_is_a_usage_error_naming_the_setting() {
        let error = PostgresMetadata
            .columns(&ObjectPath::new().schema("s"))
            .unwrap_err();
        assert!(error.message().contains("TARGET_TABLE"), "{error:?}");
        let error = PostgresMetadata
            .table_entries(&ObjectPath::new())
            .unwrap_err();
        assert!(error.message().contains("DB_SCHEMA"), "{error:?}");
    }

    #[test]
    fn the_listing_parses_a_kind_and_keeps_an_unknown_one_as_none() {
        let entries = PostgresMetadata
            .parse_table_entries(&rows(&[
                &[some("a"), some("table")],
                &[some("b"), some("materialized_view")],
                &[some("c"), None],
            ]))
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.kind))
                .collect::<Vec<_>>(),
            vec![
                ("a", Some(ObjectKind::Table)),
                ("b", Some(ObjectKind::MaterializedView)),
                ("c", None),
            ]
        );
    }

    #[test]
    fn a_generated_column_reports_its_expression_as_extra_not_as_a_default() {
        // The expression of a generated column sits in `pg_attrdef`, so the same cell that
        // carries a real default carries it. `extra` is what tells them apart.
        let columns = PostgresMetadata
            .parse_columns(&rows(&[
                &[
                    some("id"),
                    some("bigint"),
                    some("NO"),
                    None,
                    some("identity"),
                ],
                &[
                    some("doubled"),
                    some("bigint"),
                    some("YES"),
                    some("(id * 2)"),
                    some("generated"),
                ],
                &[
                    some("note"),
                    some("text"),
                    some("YES"),
                    Some("'x'::text"),
                    some(""),
                ],
            ]))
            .unwrap();
        assert_eq!(columns[0].nullable, Some(false));
        assert_eq!(columns[0].extra, "identity");
        assert_eq!(columns[1].extra, "generated");
        assert_eq!(columns[1].default.as_deref(), Some("(id * 2)"));
        assert_eq!(columns[2].extra, "");
        assert_eq!(columns[2].default.as_deref(), Some("'x'::text"));
    }

    /// Run a recipe to its end over the answers it asks for, in order.
    fn assemble(recipe: &mut dyn DdlRecipe, answers: Vec<Rows>) -> ObjectDdl {
        let mut step = recipe.next(None).unwrap();
        for answer in answers {
            assert!(matches!(step, Step::Query(_)), "{step:?}");
            step = recipe.next(Some(answer)).unwrap();
        }
        match step {
            Step::Done(ddl) => ddl,
            Step::Query(sql) => panic!("the recipe wants more: {sql}"),
        }
    }

    #[test]
    fn a_table_is_rebuilt_from_what_the_server_printed() {
        let mut recipe = PostgresMetadata.ddl(&path("w11probe", "child")).unwrap();
        let ddl = assemble(
            &mut *recipe,
            vec![
                rows(&[&[some("r"), some("w11probe"), some("child"), None, None, None, None]]),
                rows(&[
                    &[some("id"), some("bigint"), some("NO"), None, some("a"), some("")],
                    &[some("parent_id"), some("bigint"), some("YES"), None, some(""), some("")],
                    &[some("doubled"), some("bigint"), some("YES"), some("(id * 2)"), some(""), some("s")],
                    &[some("label"), some("text"), some("NO"), some("'x'::text"), some(""), some("")],
                ]),
                rows(&[
                    &[some("child_pkey"), some("p"), some("PRIMARY KEY (id)")],
                    &[
                        some("child_parent_id_fkey"),
                        some("f"),
                        some("FOREIGN KEY (parent_id) REFERENCES w11probe.parent(id) ON DELETE CASCADE"),
                    ],
                ]),
                rows(&[&[some(
                    "CREATE INDEX child_parent_ix ON w11probe.child USING btree (parent_id) WHERE (id > 5)",
                )]]),
            ],
        );
        assert_eq!(ddl.kind, Some(ObjectKind::Table));
        assert_eq!(ddl.text, include_str!("../tests/fixtures/ddl_table.sql"));
    }

    #[test]
    fn a_partition_is_written_as_partition_of_and_skips_the_column_list() {
        let mut recipe = PostgresMetadata
            .ddl(&path("w11probe", "events_2026"))
            .unwrap();
        let ddl = assemble(
            &mut *recipe,
            vec![
                rows(&[&[
                    some("r"),
                    some("w11probe"),
                    some("events_2026"),
                    None,
                    None,
                    some("FOR VALUES FROM ('2026-01-01') TO ('2027-01-01')"),
                    some("w11probe.events"),
                ]]),
                rows(&[]),
            ],
        );
        assert_eq!(
            ddl.text,
            include_str!("../tests/fixtures/ddl_partition.sql")
        );
    }

    #[test]
    fn a_partitioned_table_carries_its_partition_key() {
        let mut recipe = PostgresMetadata.ddl(&path("w11probe", "events")).unwrap();
        let ddl = assemble(
            &mut *recipe,
            vec![
                rows(&[&[
                    some("p"),
                    some("w11probe"),
                    some("events"),
                    None,
                    some("RANGE (d)"),
                    None,
                    None,
                ]]),
                rows(&[&[
                    some("d"),
                    some("date"),
                    some("NO"),
                    None,
                    some(""),
                    some(""),
                ]]),
                rows(&[]),
                rows(&[]),
            ],
        );
        assert_eq!(
            ddl.text,
            include_str!("../tests/fixtures/ddl_partitioned.sql")
        );
    }

    #[test]
    fn a_view_keeps_its_definition_without_the_servers_padding() {
        for (relkind, kind, fixture) in [
            (
                "v",
                ObjectKind::View,
                include_str!("../tests/fixtures/ddl_view.sql"),
            ),
            (
                "m",
                ObjectKind::MaterializedView,
                include_str!("../tests/fixtures/ddl_matview.sql"),
            ),
        ] {
            let mut recipe = PostgresMetadata.ddl(&path("w11probe", "My View")).unwrap();
            let ddl = assemble(
                &mut *recipe,
                vec![rows(&[&[
                    some(relkind),
                    some("w11probe"),
                    some("\"My View\""),
                    // `pg_get_viewdef` starts with a space and ends with `;`.
                    some(" SELECT id,\n    label\n   FROM w11probe.child\n  WHERE (id > 0);"),
                    None,
                    None,
                    None,
                ]])],
            );
            assert_eq!(ddl.kind, Some(kind));
            assert_eq!(ddl.text, fixture);
        }
    }

    #[test]
    fn nothing_found_is_a_usage_error_and_a_foreign_table_is_refused_by_name() {
        let mut recipe = PostgresMetadata.ddl(&path("s", "t")).unwrap();
        recipe.next(None).unwrap();
        let error = recipe.next(Some(rows(&[]))).unwrap_err();
        assert!(error.message().contains("not found"), "{error:?}");

        let mut recipe = PostgresMetadata.ddl(&path("s", "t")).unwrap();
        recipe.next(None).unwrap();
        let error = recipe
            .next(Some(rows(&[&[
                some("f"),
                some("s"),
                some("t"),
                None,
                None,
                None,
                None,
            ]])))
            .unwrap_err();
        assert!(error.message().contains("foreign tables"), "{error:?}");
    }

    #[test]
    fn a_generated_expression_is_wrapped_once() {
        assert_eq!(wrapped("(id * 2)"), "(id * 2)");
        assert_eq!(wrapped("id * 2"), "(id * 2)");
    }

    #[test]
    fn no_statement_the_rebuilt_text_holds_ends_early() {
        // The only `;` in the text is the one that closes a statement.
        let mut recipe = PostgresMetadata.ddl(&path("s", "t")).unwrap();
        let ddl = assemble(
            &mut *recipe,
            vec![
                rows(&[&[some("r"), some("s"), some("t"), None, None, None, None]]),
                rows(&[&[
                    some("a"),
                    some("text"),
                    some("YES"),
                    None,
                    some(""),
                    some(""),
                ]]),
                rows(&[]),
                rows(&[]),
            ],
        );
        assert_eq!(ddl.text.matches(';').count(), 1, "{}", ddl.text);
    }
}
