//! Trino metadata: object kinds and columns from `information_schema`, DDL from `SHOW CREATE`.
//!
//! `SHOW CREATE TABLE` and `SHOW CREATE VIEW` each refuse the other's object (`Relation … is a
//! table, not a view`), so the kind has to be known first and the DDL is a two-statement recipe.
//! `SHOW TABLES` and `information_schema.tables` list the same objects (measured on `tpch.tiny`),
//! so asking for kinds does not change which names the tree shows.
//!
//! Names go into a statement as a double-quoted identifier (`quote`) or a string literal
//! (`literal`), the two spellings Trino has; neither is built by plain concatenation. Trino reads no
//! backslash escape inside a literal, so doubling the quote is the whole rule.
//!
//! The statements are built here and run by the engine through `Session::execute`; nothing in this
//! file does I/O.

use qh_core::EngineError;
use qh_driver::{
    required_part, ColumnInfo, DdlRecipe, MetadataSql, ObjectDdl, ObjectKind, ObjectPath, Rows,
    Step, TableEntry,
};

use crate::{literal, quote};

/// The metadata statements of Trino.
pub struct TrinoMetadata;

fn target_of(target: &ObjectPath) -> Result<(&str, &str, &str), EngineError> {
    let catalog = required_part(&target.catalog, "TARGET_CATALOG", "to name the object")?;
    let schema = required_part(&target.schema, "TARGET_SCHEMA", "to name the object")?;
    let table = required_part(&target.table, "TARGET_TABLE", "to name the object")?;
    Ok((catalog, schema, table))
}

impl MetadataSql for TrinoMetadata {
    fn table_entries(&self, path: &ObjectPath) -> Result<String, EngineError> {
        let catalog = required_part(&path.catalog, "the catalog", "to list tables")?;
        let schema = required_part(&path.schema, "the schema", "to list tables")?;
        Ok(format!(
            "SELECT table_name, table_type FROM {}.information_schema.tables \
             WHERE table_schema = {} ORDER BY table_name",
            quote(catalog),
            literal(schema)
        ))
    }

    fn parse_table_entries(&self, rows: &Rows) -> Result<Vec<TableEntry>, EngineError> {
        (0..rows.rows.len())
            .map(|row| {
                Ok(TableEntry {
                    name: rows.required(row, 0)?.to_owned(),
                    kind: match rows.cell(row, 1) {
                        Some("BASE TABLE") => Some(ObjectKind::Table),
                        Some("VIEW") => Some(ObjectKind::View),
                        _ => None,
                    },
                })
            })
            .collect()
    }

    fn columns(&self, target: &ObjectPath) -> Result<String, EngineError> {
        let (catalog, schema, table) = target_of(target)?;
        Ok(format!(
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM {}.information_schema.columns \
             WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
            quote(catalog),
            literal(schema),
            literal(table)
        ))
    }

    /// `information_schema.columns` in Trino has no `extra` column, so it is empty.
    fn parse_columns(&self, rows: &Rows) -> Result<Vec<ColumnInfo>, EngineError> {
        (0..rows.rows.len())
            .map(|row| {
                Ok(ColumnInfo {
                    name: rows.required(row, 0)?.to_owned(),
                    data_type: rows.required(row, 1)?.to_owned(),
                    nullable: match rows.cell(row, 2) {
                        Some("YES") => Some(true),
                        Some("NO") => Some(false),
                        _ => None,
                    },
                    default: rows.cell(row, 3).map(str::to_owned),
                    extra: String::new(),
                })
            })
            .collect()
    }

    fn ddl(&self, target: &ObjectPath) -> Result<Box<dyn DdlRecipe>, EngineError> {
        let (catalog, schema, table) = target_of(target)?;
        Ok(Box::new(TrinoDdl {
            kind_sql: format!(
                "SELECT table_type FROM {}.information_schema.tables \
                 WHERE table_schema = {} AND table_name = {}",
                quote(catalog),
                literal(schema),
                literal(table)
            ),
            name: format!("{}.{}.{}", quote(catalog), quote(schema), quote(table)),
            plain: format!("{catalog}.{schema}.{table}"),
            stage: Stage::Kind,
        }))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Kind,
    Show(ObjectKind),
    /// `SHOW CREATE MATERIALIZED VIEW` was already tried, so a failure is the answer.
    Last,
}

/// The kind first, then the `SHOW CREATE` that fits it.
struct TrinoDdl {
    kind_sql: String,
    /// The three-part name, quoted.
    name: String,
    /// The same name for a message.
    plain: String,
    stage: Stage,
}

impl TrinoDdl {
    fn show(&self, kind: ObjectKind) -> String {
        let word = match kind {
            ObjectKind::View => "VIEW",
            ObjectKind::MaterializedView => "MATERIALIZED VIEW",
            _ => "TABLE",
        };
        format!("SHOW CREATE {word} {}", self.name)
    }
}

impl DdlRecipe for TrinoDdl {
    fn next(&mut self, previous: Option<Rows>) -> Result<Step, EngineError> {
        let Some(rows) = previous else {
            self.stage = Stage::Kind;
            return Ok(Step::Query(self.kind_sql.clone()));
        };
        match self.stage {
            Stage::Kind => {
                let Some(table_type) = rows.cell(0, 0) else {
                    return Err(EngineError::Usage {
                        message: format!("{} was not found", self.plain),
                    });
                };
                let kind = if table_type == "VIEW" {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                };
                self.stage = Stage::Show(kind);
                Ok(Step::Query(self.show(kind)))
            }
            Stage::Show(kind) => Ok(Step::Done(ObjectDdl {
                kind: Some(kind),
                text: format!(
                    "{};\n",
                    rows.required(0, 0)?.trim_end().trim_end_matches(';')
                ),
                truncated: false,
            })),
            Stage::Last => Ok(Step::Done(ObjectDdl {
                kind: Some(ObjectKind::MaterializedView),
                text: format!(
                    "{};\n",
                    rows.required(0, 0)?.trim_end().trim_end_matches(';')
                ),
                truncated: false,
            })),
        }
    }

    /// A connector that lists a materialized view as a table refuses `SHOW CREATE TABLE` for it
    /// in words; that one refusal is answered with the one statement that fits. Not seen on a
    /// server: the dev coordinator's connectors have no materialized view.
    fn recover(&mut self, error: &EngineError) -> Option<Step> {
        if matches!(self.stage, Stage::Show(ObjectKind::Table))
            && error.message().contains("is a materialized view")
        {
            self.stage = Stage::Last;
            return Some(Step::Query(self.show(ObjectKind::MaterializedView)));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn target(catalog: &str, schema: &str, table: &str) -> ObjectPath {
        ObjectPath::new()
            .catalog(catalog)
            .schema(schema)
            .table(table)
    }

    #[test]
    fn the_statements_are_pinned() {
        assert_eq!(
            TrinoMetadata
                .table_entries(&ObjectPath::new().catalog("tpch").schema("tiny"))
                .unwrap(),
            "SELECT table_name, table_type FROM \"tpch\".information_schema.tables \
             WHERE table_schema = 'tiny' ORDER BY table_name"
        );
        assert_eq!(
            TrinoMetadata
                .columns(&target("tpch", "tiny", "nation"))
                .unwrap(),
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM \"tpch\".information_schema.columns \
             WHERE table_schema = 'tiny' AND table_name = 'nation' ORDER BY ordinal_position"
        );
        let mut recipe = TrinoMetadata
            .ddl(&target("tpch", "tiny", "nation"))
            .unwrap();
        assert_eq!(
            recipe.next(None).unwrap(),
            Step::Query(
                "SELECT table_type FROM \"tpch\".information_schema.tables \
                 WHERE table_schema = 'tiny' AND table_name = 'nation'"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_name_cannot_leave_its_quotes() {
        let sql = TrinoMetadata
            .columns(&target("c\"d", "s'; DROP TABLE x; --", "t'u"))
            .unwrap();
        assert!(
            sql.contains("FROM \"c\"\"d\".information_schema.columns"),
            "{sql}"
        );
        assert!(
            sql.contains("table_schema = 's''; DROP TABLE x; --'"),
            "{sql}"
        );
        assert!(sql.contains("table_name = 't''u'"), "{sql}");
    }

    #[test]
    fn a_table_asks_for_the_kind_and_then_for_show_create_table() {
        let mut recipe = TrinoMetadata
            .ddl(&target("tpch", "tiny", "nation"))
            .unwrap();
        recipe.next(None).unwrap();
        assert_eq!(
            recipe.next(Some(rows(&[&[some("BASE TABLE")]]))).unwrap(),
            Step::Query("SHOW CREATE TABLE \"tpch\".\"tiny\".\"nation\"".to_owned())
        );
        let step = recipe
            .next(Some(rows(&[&[some(
                "CREATE TABLE tpch.tiny.nation (\n   nationkey bigint\n)",
            )]])))
            .unwrap();
        assert_eq!(
            step,
            Step::Done(ObjectDdl {
                kind: Some(ObjectKind::Table),
                text: "CREATE TABLE tpch.tiny.nation (\n   nationkey bigint\n);\n".to_owned(),
                truncated: false,
            })
        );
    }

    #[test]
    fn a_view_asks_for_show_create_view() {
        let mut recipe = TrinoMetadata.ddl(&target("hive", "a", "v")).unwrap();
        recipe.next(None).unwrap();
        assert_eq!(
            recipe.next(Some(rows(&[&[some("VIEW")]]))).unwrap(),
            Step::Query("SHOW CREATE VIEW \"hive\".\"a\".\"v\"".to_owned())
        );
    }

    #[test]
    fn nothing_found_is_a_usage_error() {
        let mut recipe = TrinoMetadata.ddl(&target("hive", "a", "v")).unwrap();
        recipe.next(None).unwrap();
        let error = recipe.next(Some(rows(&[]))).unwrap_err();
        assert!(
            error.message().contains("hive.a.v was not found"),
            "{error:?}"
        );
    }

    #[test]
    fn a_materialized_view_listed_as_a_table_is_tried_once_more_and_no_more() {
        let refusal = EngineError::Usage {
            message: "Relation 'hive.a.m' is a materialized view, not a table".to_owned(),
        };
        let mut recipe = TrinoMetadata.ddl(&target("hive", "a", "m")).unwrap();
        recipe.next(None).unwrap();
        recipe.next(Some(rows(&[&[some("BASE TABLE")]]))).unwrap();
        assert_eq!(
            recipe.recover(&refusal),
            Some(Step::Query(
                "SHOW CREATE MATERIALIZED VIEW \"hive\".\"a\".\"m\"".to_owned()
            ))
        );
        // The second refusal is the answer.
        assert_eq!(recipe.recover(&refusal), None);
        let Step::Done(ddl) = recipe
            .next(Some(rows(&[&[some(
                "CREATE MATERIALIZED VIEW hive.a.m AS SELECT 1",
            )]])))
            .unwrap()
        else {
            panic!("the last statement finishes the recipe")
        };
        assert_eq!(ddl.kind, Some(ObjectKind::MaterializedView));
    }

    #[test]
    fn any_other_failure_is_not_recovered() {
        let mut recipe = TrinoMetadata.ddl(&target("hive", "a", "t")).unwrap();
        recipe.next(None).unwrap();
        recipe.next(Some(rows(&[&[some("BASE TABLE")]]))).unwrap();
        let other = EngineError::Usage {
            message: "Table 'hive.a.t' does not exist".to_owned(),
        };
        assert_eq!(recipe.recover(&other), None);
    }

    #[test]
    fn the_listing_and_the_columns_read_the_servers_words() {
        let entries = TrinoMetadata
            .parse_table_entries(&rows(&[
                &[some("a"), some("BASE TABLE")],
                &[some("b"), some("VIEW")],
                &[some("c"), some("OTHER")],
            ]))
            .unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
            vec![Some(ObjectKind::Table), Some(ObjectKind::View), None]
        );
        let columns = TrinoMetadata
            .parse_columns(&rows(&[
                &[some("nationkey"), some("bigint"), some("YES"), None],
                &[some("name"), some("varchar(25)"), some("NO"), some("''")],
            ]))
            .unwrap();
        assert_eq!(columns[0].nullable, Some(true));
        assert_eq!(columns[1].nullable, Some(false));
        assert_eq!(columns[1].default.as_deref(), Some("''"));
        assert!(columns.iter().all(|column| column.extra.is_empty()));
    }

    #[test]
    fn a_missing_part_of_the_name_is_a_usage_error_naming_the_setting() {
        let error = TrinoMetadata
            .columns(&ObjectPath::new().schema("s").table("t"))
            .unwrap_err();
        assert!(error.message().contains("TARGET_CATALOG"), "{error:?}");
    }
}
