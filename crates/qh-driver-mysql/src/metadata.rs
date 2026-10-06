//! MySQL metadata: object kinds, columns, and the server's own `SHOW CREATE`.
//!
//! Every name goes into a statement as a back-quoted identifier (`qh_sql::quote_ident`, which
//! doubles a back-quote), and no statement here holds a string literal. That is deliberate:
//! whether a backslash inside a literal is an escape depends on the session's `sql_mode`
//! (`NO_BACKSLASH_ESCAPES`), which this crate cannot know before it connects, while a back-quoted
//! identifier reads the same in every mode. `SHOW COLUMNS` is therefore used where the
//! `information_schema.COLUMNS` form would need `TABLE_SCHEMA = '…'`; both answer from the same
//! source and with the same privileges.
//!
//! The statements are built here and run by the engine through `Session::execute`; nothing in this
//! file does I/O.

use qh_core::EngineError;
use qh_driver::{
    required_part, ColumnInfo, DdlRecipe, MetadataSql, ObjectDdl, ObjectKind, ObjectPath, Rows,
    Step, TableEntry,
};
use qh_sql::{quote_ident, IdentStyle};

/// The metadata statements of MySQL.
pub struct MysqlMetadata;

fn quote(name: &str) -> String {
    quote_ident(IdentStyle::Mysql, name)
}

/// The database and table of a target. MySQL has no schema level, so its database travels in
/// `schema`, the slot the driver's own `browse` reads it from.
fn target_of(target: &ObjectPath) -> Result<(&str, &str), EngineError> {
    let database = required_part(&target.schema, "TARGET_CATALOG", "to name the object")?;
    let table = required_part(&target.table, "TARGET_TABLE", "to name the object")?;
    Ok((database, table))
}

impl MetadataSql for MysqlMetadata {
    fn table_entries(&self, path: &ObjectPath) -> Result<String, EngineError> {
        let database = required_part(&path.schema, "DB_DATABASE", "to list tables on MySQL")?;
        Ok(format!("SHOW FULL TABLES FROM {}", quote(database)))
    }

    fn parse_table_entries(&self, rows: &Rows) -> Result<Vec<TableEntry>, EngineError> {
        (0..rows.rows.len())
            .map(|row| {
                Ok(TableEntry {
                    name: rows.required(row, 0)?.to_owned(),
                    // `SYSTEM VIEW` is what `information_schema` and `performance_schema` hold.
                    kind: match rows.cell(row, 1) {
                        Some("BASE TABLE") => Some(ObjectKind::Table),
                        Some("VIEW" | "SYSTEM VIEW") => Some(ObjectKind::View),
                        _ => None,
                    },
                })
            })
            .collect()
    }

    fn columns(&self, target: &ObjectPath) -> Result<String, EngineError> {
        let (database, table) = target_of(target)?;
        Ok(format!(
            "SHOW COLUMNS FROM {} FROM {}",
            quote(table),
            quote(database)
        ))
    }

    /// Field, Type, Null, Key, Default, Extra.
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
                    default: rows.cell(row, 4).map(str::to_owned),
                    extra: rows.cell(row, 5).unwrap_or_default().to_owned(),
                })
            })
            .collect()
    }

    fn ddl(&self, target: &ObjectPath) -> Result<Box<dyn DdlRecipe>, EngineError> {
        let (database, table) = target_of(target)?;
        Ok(Box::new(MysqlDdl {
            sql: Some(format!(
                "SHOW CREATE TABLE {}.{}",
                quote(database),
                quote(table)
            )),
            name: format!("{database}.{table}"),
        }))
    }
}

/// `SHOW CREATE TABLE`, which answers for a view too: one statement.
struct MysqlDdl {
    sql: Option<String>,
    name: String,
}

impl DdlRecipe for MysqlDdl {
    fn next(&mut self, previous: Option<Rows>) -> Result<Step, EngineError> {
        if let Some(sql) = self.sql.take() {
            return Ok(Step::Query(sql));
        }
        let rows = previous.ok_or_else(|| EngineError::Internal {
            message: "the DDL recipe was asked again with nothing to read".to_owned(),
        })?;
        if rows.rows.is_empty() {
            return Err(EngineError::Usage {
                message: format!(
                    "{} was not found, or this account has no privilege on it",
                    self.name
                ),
            });
        }
        // The first column is named for what the object is: `Table` or `View`. The text is the
        // second; a view's answer has two more columns (the client's character set and collation).
        let kind = match rows.columns.first().map(String::as_str) {
            Some("Table") => Some(ObjectKind::Table),
            Some("View") => Some(ObjectKind::View),
            _ => None,
        };
        Ok(Step::Done(ObjectDdl {
            kind,
            text: format!(
                "{};\n",
                rows.required(0, 1)?.trim_end().trim_end_matches(';')
            ),
            truncated: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(columns: &[&str], cells: &[&[Option<&str>]]) -> Rows {
        Rows {
            columns: columns.iter().map(|name| (*name).to_owned()).collect(),
            rows: cells
                .iter()
                .map(|row| row.iter().map(|cell| cell.map(str::to_owned)).collect())
                .collect(),
        }
    }

    fn some(text: &str) -> Option<&str> {
        Some(text)
    }

    fn target(database: &str, table: &str) -> ObjectPath {
        ObjectPath::new().schema(database).table(table)
    }

    #[test]
    fn the_statements_are_pinned() {
        assert_eq!(
            MysqlMetadata
                .table_entries(&ObjectPath::new().schema("sips"))
                .unwrap(),
            "SHOW FULL TABLES FROM `sips`"
        );
        assert_eq!(
            MysqlMetadata.columns(&target("sips", "people")).unwrap(),
            "SHOW COLUMNS FROM `people` FROM `sips`"
        );
        let mut recipe = MysqlMetadata.ddl(&target("sips", "people")).unwrap();
        assert_eq!(
            recipe.next(None).unwrap(),
            Step::Query("SHOW CREATE TABLE `sips`.`people`".to_owned())
        );
    }

    #[test]
    fn a_name_stays_inside_its_back_quotes_whatever_it_holds() {
        // No statement holds a string literal, so a quote, a backslash or a semicolon in a name
        // can only ever be inside back-quotes, where a back-quote is doubled and nothing else
        // has a meaning.
        for name in ["d`b", "a'b", "a\\", "a\\`", "\"x\"", "t`; DROP TABLE x; --"] {
            let quoted = format!("`{}`", name.replace('`', "``"));
            assert_eq!(
                MysqlMetadata.columns(&target(name, name)).unwrap(),
                format!("SHOW COLUMNS FROM {quoted} FROM {quoted}")
            );
            assert_eq!(
                MysqlMetadata
                    .table_entries(&ObjectPath::new().schema(name))
                    .unwrap(),
                format!("SHOW FULL TABLES FROM {quoted}")
            );
        }
    }

    #[test]
    fn the_listing_maps_the_servers_kind_words() {
        let entries = MysqlMetadata
            .parse_table_entries(&rows(
                &["Tables_in_qh", "Table_type"],
                &[
                    &[some("a"), some("BASE TABLE")],
                    &[some("b"), some("VIEW")],
                    &[some("c"), some("SYSTEM VIEW")],
                    &[some("d"), some("SOMETHING NEW")],
                ],
            ))
            .unwrap();
        assert_eq!(
            entries.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
            vec![
                Some(ObjectKind::Table),
                Some(ObjectKind::View),
                Some(ObjectKind::View),
                None
            ]
        );
    }

    #[test]
    fn columns_keep_the_servers_extra_and_a_null_default() {
        let columns = MysqlMetadata
            .parse_columns(&rows(
                &["Field", "Type", "Null", "Key", "Default", "Extra"],
                &[
                    &[
                        some("id"),
                        some("bigint unsigned"),
                        some("NO"),
                        some("PRI"),
                        None,
                        some("auto_increment"),
                    ],
                    &[
                        some("made"),
                        some("timestamp"),
                        some("YES"),
                        some(""),
                        some("CURRENT_TIMESTAMP"),
                        some("DEFAULT_GENERATED"),
                    ],
                ],
            ))
            .unwrap();
        assert_eq!(columns[0].nullable, Some(false));
        assert_eq!(columns[0].default, None);
        assert_eq!(columns[0].extra, "auto_increment");
        assert_eq!(columns[1].default.as_deref(), Some("CURRENT_TIMESTAMP"));
        assert_eq!(columns[1].extra, "DEFAULT_GENERATED");
    }

    #[test]
    fn the_ddl_kind_comes_from_the_first_column_name() {
        for (first, kind) in [("Table", ObjectKind::Table), ("View", ObjectKind::View)] {
            let mut recipe = MysqlMetadata.ddl(&target("qh", "t")).unwrap();
            recipe.next(None).unwrap();
            let step = recipe
                .next(Some(rows(
                    &[
                        first,
                        "Create",
                        "character_set_client",
                        "collation_connection",
                    ],
                    &[&[
                        some("t"),
                        some("CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB"),
                        some("utf8mb4"),
                        some("utf8mb4_0900_ai_ci"),
                    ]],
                )))
                .unwrap();
            let Step::Done(ddl) = step else {
                panic!("one statement is enough")
            };
            assert_eq!(ddl.kind, Some(kind));
            assert_eq!(
                ddl.text,
                "CREATE TABLE `t` (\n  `id` int\n) ENGINE=InnoDB;\n"
            );
        }
    }

    #[test]
    fn nothing_found_is_a_usage_error() {
        let mut recipe = MysqlMetadata.ddl(&target("qh", "t")).unwrap();
        recipe.next(None).unwrap();
        let error = recipe
            .next(Some(rows(&["Table", "Create Table"], &[])))
            .unwrap_err();
        assert!(error.message().contains("qh.t was not found"), "{error:?}");
    }

    #[test]
    fn a_missing_part_of_the_name_is_a_usage_error_naming_the_setting() {
        let error = MysqlMetadata
            .columns(&ObjectPath::new().table("t"))
            .unwrap_err();
        assert!(error.message().contains("TARGET_CATALOG"), "{error:?}");
        let error = MysqlMetadata.table_entries(&ObjectPath::new()).unwrap_err();
        assert!(error.message().contains("DB_DATABASE"), "{error:?}");
    }
}
