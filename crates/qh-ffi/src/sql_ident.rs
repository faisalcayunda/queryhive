//! How a write target is named, per driver.
//!
//! From `exporter/drivers.py`'s `slots`, `qualified` and `reference`. The three
//! `TARGET_*` settings are one per name slot, and **which of them a driver needs is
//! the driver's own list**: Trino writes `catalog.schema.table`, PostgreSQL
//! `schema.table` (the database is the connection), MySQL `` `database`.`table` ``
//! (there is no schema level). A part a driver has no level for is dropped from the
//! name rather than required from the caller.
//!
//! Two spellings of the same name come out of here, and the difference is the point:
//! [`qualified`] is the one that goes into SQL, with each part quoted by the
//! driver's own dialect, and [`reference`] is the plain one a `done` event or a
//! warning shows the user. A DROP must never be able to hit a different table than
//! the CREATE that follows it because one name needed quotes and the other did not,
//! which is why both are built from the same slot list.

use qh_driver::DriverKind;
use qh_sql::{quote_ident, IdentStyle};

/// Which part of a name a slot fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Database,
    Schema,
    Table,
}

/// One `TARGET_*` setting, and the part of the name it fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub target: &'static str,
    pub part: Part,
}

/// Trino: catalog, schema, table — all three.
const TRINO_SLOTS: [Slot; 3] = [
    Slot {
        target: "TARGET_CATALOG",
        part: Part::Database,
    },
    Slot {
        target: "TARGET_SCHEMA",
        part: Part::Schema,
    },
    Slot {
        target: "TARGET_TABLE",
        part: Part::Table,
    },
];

/// PostgreSQL: schema and table. The database is the connection.
const POSTGRES_SLOTS: [Slot; 2] = [
    Slot {
        target: "TARGET_SCHEMA",
        part: Part::Schema,
    },
    Slot {
        target: "TARGET_TABLE",
        part: Part::Table,
    },
];

/// MySQL: database and table — `TARGET_CATALOG` names its database, because that is
/// the level it has.
const MYSQL_SLOTS: [Slot; 2] = [
    Slot {
        target: "TARGET_CATALOG",
        part: Part::Database,
    },
    Slot {
        target: "TARGET_TABLE",
        part: Part::Table,
    },
];

/// One driver's naming: which parts it uses, and how it quotes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotStyle {
    pub kind: DriverKind,
    pub style: IdentStyle,
}

impl SlotStyle {
    /// The naming for one driver.
    ///
    /// Trino and PostgreSQL double a `"`; MySQL doubles a backtick. That is the same
    /// rule `Driver.quote` had, and it is why quoting is not done with a single
    /// hard-coded character.
    pub fn of(kind: DriverKind) -> Self {
        let style = match kind {
            DriverKind::Trino | DriverKind::Postgres => IdentStyle::Ansi,
            DriverKind::Mysql => IdentStyle::Mysql,
        };
        Self { kind, style }
    }

    /// One part, quoted for this driver.
    pub fn quote(&self, name: &str) -> String {
        quote_ident(self.style, name)
    }
}

/// The slots a driver uses, outermost first.
pub fn slots(style: SlotStyle) -> &'static [Slot] {
    match style.kind {
        DriverKind::Trino => &TRINO_SLOTS,
        DriverKind::Postgres => &POSTGRES_SLOTS,
        DriverKind::Mysql => &MYSQL_SLOTS,
    }
}

/// The target as this driver writes it in SQL: only the levels it has, quoted.
pub fn qualified(style: SlotStyle, catalog: &str, schema: &str, table: &str) -> String {
    name(style, catalog, schema, table, |slot| {
        style.quote(slot_value(slot, catalog, schema, table))
    })
}

/// The same target written plainly, for a `done` event or a warning.
pub fn reference(style: SlotStyle, catalog: &str, schema: &str, table: &str) -> String {
    name(style, catalog, schema, table, |slot| {
        slot_value(slot, catalog, schema, table).to_owned()
    })
}

/// Join the parts a driver has, in its own order, skipping the empty ones.
fn name(
    style: SlotStyle,
    catalog: &str,
    schema: &str,
    table: &str,
    render: impl Fn(Slot) -> String,
) -> String {
    slots(style)
        .iter()
        .filter_map(|slot| {
            let value = slot_value(*slot, catalog, schema, table);
            if value.is_empty() {
                None
            } else {
                Some(render(*slot))
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The caller's value for one slot: `TARGET_CATALOG` is the database part, and the
/// rest follow from their names.
fn slot_value<'a>(slot: Slot, catalog: &'a str, schema: &'a str, table: &'a str) -> &'a str {
    match slot.part {
        Part::Database => catalog,
        Part::Schema => schema,
        Part::Table => table,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_driver_writes_the_levels_it_has() {
        let trino = SlotStyle::of(DriverKind::Trino);
        assert_eq!(
            qualified(trino, "hive", "analytics", "people_copy"),
            "\"hive\".\"analytics\".\"people_copy\""
        );
        assert_eq!(
            reference(trino, "hive", "analytics", "people_copy"),
            "hive.analytics.people_copy"
        );

        let postgres = SlotStyle::of(DriverKind::Postgres);
        assert_eq!(
            qualified(postgres, "hive", "analytics", "people_copy"),
            "\"analytics\".\"people_copy\""
        );
        assert_eq!(
            reference(postgres, "hive", "analytics", "people_copy"),
            "analytics.people_copy"
        );

        let mysql = SlotStyle::of(DriverKind::Mysql);
        assert_eq!(
            qualified(mysql, "sips", "analytics", "people_copy"),
            "`sips`.`people_copy`"
        );
        assert_eq!(
            reference(mysql, "sips", "analytics", "people_copy"),
            "sips.people_copy"
        );
    }

    #[test]
    fn an_embedded_quote_is_doubled_by_the_drivers_own_character() {
        let trino = SlotStyle::of(DriverKind::Trino);
        assert_eq!(
            qualified(trino, "hi\"ve", "an\"", "t"),
            "\"hi\"\"ve\".\"an\"\"\".\"t\""
        );
        let mysql = SlotStyle::of(DriverKind::Mysql);
        assert_eq!(qualified(mysql, "my`db", "", "t"), "`my``db`.`t`");
    }

    #[test]
    fn a_driver_needs_only_the_settings_it_has_a_level_for() {
        assert_eq!(
            slots(SlotStyle::of(DriverKind::Trino))
                .iter()
                .map(|slot| slot.target)
                .collect::<Vec<_>>(),
            vec!["TARGET_CATALOG", "TARGET_SCHEMA", "TARGET_TABLE"]
        );
        // PostgreSQL ignores TARGET_CATALOG, MySQL ignores TARGET_SCHEMA: neither is
        // left in the required list for the other driver.
        assert_eq!(
            slots(SlotStyle::of(DriverKind::Postgres))
                .iter()
                .map(|slot| slot.target)
                .collect::<Vec<_>>(),
            vec!["TARGET_SCHEMA", "TARGET_TABLE"]
        );
        assert_eq!(
            slots(SlotStyle::of(DriverKind::Mysql))
                .iter()
                .map(|slot| slot.target)
                .collect::<Vec<_>>(),
            vec!["TARGET_CATALOG", "TARGET_TABLE"]
        );
    }
}
