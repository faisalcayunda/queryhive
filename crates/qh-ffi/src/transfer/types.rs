//! Column types between dialects: faithfully, or not at all (W13-T20 §4.1-§4.3, B-3a).
//!
//! A source column is described by a [`SqlType`], a structured descriptor, never by the
//! server's type text. That text (`native`) is for messages only: it is **never** copied into
//! SQL, because a domain, an extension type or a hostile name could carry a statement in it.
//! The type text of every statement this module helps write comes from [`TargetType`], a
//! closed set the engine owns, rendered by [`render`].
//!
//! Every pair of (source type, target dialect) has exactly one outcome ([`Mapping`]):
//!
//! | | |
//! |---|---|
//! | **E**xact | the same type, or one that holds the same values the same way |
//! | **W**iden | a wider type that holds every value (reported when the type name changes) |
//! | **T**ext fallback | a documented, reported stand-in (`uuid` as `char(36)`, an enum as text, a map as JSON) |
//! | **R**efuse | no faithful target: the transfer stops before the first row |
//!
//! A text fallback is only ever a T cell; a cell that is R stays R (OQ-7). What can only be
//! decided per value (a scale that does not fit, a date a dialect cannot hold) is
//! [`super::encode`]'s, and it refuses the value rather than changing it.
//!
//! The descriptor is declared here so the mapping is pure and testable; the drivers fill it
//! in (W13-T20 step 4), and when they do `SqlType` moves to `qh-driver` and this module keeps
//! a `pub use`, so nothing in here changes its path.

use qh_driver::DriverKind;

use super::names;

/// The dialects a transfer can write to. Trino is a source only: lifted out of the type so
/// that "a transfer to Trino" cannot be written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Postgres,
    Mysql,
}

impl Target {
    pub const ALL: [Target; 2] = [Target::Postgres, Target::Mysql];

    /// The target a driver kind writes to, or `None` for the one that cannot (Trino).
    pub fn of(kind: DriverKind) -> Option<Target> {
        match kind {
            DriverKind::Postgres => Some(Target::Postgres),
            DriverKind::Mysql => Some(Target::Mysql),
            DriverKind::Trino => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Postgres => "PostgreSQL",
            Target::Mysql => "MySQL",
        }
    }
}

pub use qh_driver::{Base, PgBuiltin, SqlType, TimeZone};

/// How much of a text a MySQL column can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSize {
    /// PostgreSQL `text`; MySQL `text` (64 KiB).
    Text,
    /// MySQL `mediumtext` (16 MiB).
    Medium,
    /// MySQL `longtext` (4 GiB).
    Long,
}

/// The type a target column gets. A closed set: the DDL text is rendered from it and from
/// nothing the source sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetType {
    /// PostgreSQL `boolean`; MySQL `tinyint(1)`.
    Boolean,
    /// `bytes` is 1, 2, 3, 4 or 8. PostgreSQL has the signed 2, 4 and 8 only.
    Int {
        bytes: u8,
        unsigned: bool,
    },
    /// PostgreSQL `numeric(p,s)` or `numeric`; MySQL `decimal(p,s)`.
    Numeric {
        precision: Option<u32>,
        scale: u32,
    },
    Float {
        double: bool,
    },
    Varchar(u32),
    Char(u32),
    Text(TextSize),
    /// PostgreSQL `bytea`; MySQL `varbinary(n)`, or `longblob` for `None`.
    Binary(Option<u32>),
    Uuid,
    Date,
    Time {
        digits: u32,
    },
    TimeTz {
        digits: u32,
    },
    Timestamp {
        digits: u32,
    },
    TimestampTz {
        digits: u32,
    },
    /// MySQL `datetime(p)`.
    DateTime {
        digits: u32,
    },
    Interval,
    Json,
    Jsonb,
    /// PostgreSQL `T[]`.
    Array(Box<TargetType>),
    /// A PostgreSQL built-in, with the bit length for `bit(n)` and `varbit(n)`.
    Builtin {
        kind: PgBuiltin,
        length: Option<u32>,
    },
}

/// The DDL text of a target type in a dialect, or `None` when the dialect has no such type.
pub fn render(ty: &TargetType, target: Target) -> Option<String> {
    use TargetType as T;
    Some(match (target, ty) {
        (Target::Postgres, T::Boolean) => "boolean".to_owned(),
        (Target::Mysql, T::Boolean) => "tinyint(1)".to_owned(),

        (
            Target::Postgres,
            T::Int {
                bytes,
                unsigned: false,
            },
        ) => match bytes {
            2 => "smallint",
            4 => "integer",
            8 => "bigint",
            _ => return None,
        }
        .to_owned(),
        (Target::Postgres, T::Int { .. }) => return None,
        (Target::Mysql, T::Int { bytes, unsigned }) => {
            let name = match bytes {
                1 => "tinyint",
                2 => "smallint",
                3 => "mediumint",
                4 => "int",
                8 => "bigint",
                _ => return None,
            };
            if *unsigned {
                format!("{name} unsigned")
            } else {
                name.to_owned()
            }
        }

        (
            Target::Postgres,
            T::Numeric {
                precision: None, ..
            },
        ) => "numeric".to_owned(),
        (
            Target::Postgres,
            T::Numeric {
                precision: Some(p),
                scale,
            },
        ) => {
            format!("numeric({p},{scale})")
        }
        (
            Target::Mysql,
            T::Numeric {
                precision: Some(p),
                scale,
            },
        ) => {
            format!("decimal({p},{scale})")
        }
        (
            Target::Mysql,
            T::Numeric {
                precision: None, ..
            },
        ) => return None,

        (Target::Postgres, T::Float { double: false }) => "real".to_owned(),
        (Target::Postgres, T::Float { double: true }) => "double precision".to_owned(),
        (Target::Mysql, T::Float { double: false }) => "float".to_owned(),
        (Target::Mysql, T::Float { double: true }) => "double".to_owned(),

        (_, T::Varchar(n)) => format!("varchar({n})"),
        (_, T::Char(n)) => format!("char({n})"),
        (Target::Postgres, T::Text(_)) => "text".to_owned(),
        (Target::Mysql, T::Text(size)) => match size {
            TextSize::Text => "text",
            TextSize::Medium => "mediumtext",
            TextSize::Long => "longtext",
        }
        .to_owned(),

        (Target::Postgres, T::Binary(_)) => "bytea".to_owned(),
        (Target::Mysql, T::Binary(Some(n))) => format!("varbinary({n})"),
        (Target::Mysql, T::Binary(None)) => "longblob".to_owned(),

        (Target::Postgres, T::Uuid) => "uuid".to_owned(),
        (Target::Mysql, T::Uuid) => return None,

        (_, T::Date) => "date".to_owned(),
        (_, T::Time { digits }) => format!("time({digits})"),
        (Target::Postgres, T::TimeTz { digits }) => format!("timetz({digits})"),
        (Target::Postgres, T::Timestamp { digits }) => format!("timestamp({digits})"),
        (Target::Postgres, T::TimestampTz { digits }) => format!("timestamptz({digits})"),
        (Target::Mysql, T::DateTime { digits }) => format!("datetime({digits})"),
        (Target::Postgres, T::Interval) => "interval".to_owned(),

        (_, T::Json) => "json".to_owned(),
        (Target::Postgres, T::Jsonb) => "jsonb".to_owned(),

        (Target::Postgres, T::Array(inner)) => format!("{}[]", render(inner, target)?),
        (Target::Postgres, T::Builtin { kind, length }) => match length {
            Some(n) if matches!(kind, PgBuiltin::Bit | PgBuiltin::Varbit) => {
                format!("{}({n})", kind.ddl_name())
            }
            _ => kind.ddl_name().to_owned(),
        },

        _ => return None,
    })
}

/// A resolved target column type and the DDL text rendered from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub ty: TargetType,
    pub ddl: String,
}

/// The one outcome of mapping a source column to a dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapping {
    Exact(Resolved),
    /// A wider type that holds every value; the text says what changed.
    Widened(Resolved, String),
    /// A documented stand-in, reported per column; the text says what it is.
    TextFallback(Resolved, String),
    /// No faithful target; the text is the reason.
    Refuse(String),
}

/// A [`Mapping`] without its payload, for a table of expected outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Exact,
    Widened,
    TextFallback,
    Refuse,
}

impl Mapping {
    pub fn outcome(&self) -> Outcome {
        match self {
            Mapping::Exact(_) => Outcome::Exact,
            Mapping::Widened(..) => Outcome::Widened,
            Mapping::TextFallback(..) => Outcome::TextFallback,
            Mapping::Refuse(_) => Outcome::Refuse,
        }
    }

    pub fn resolved(&self) -> Option<&Resolved> {
        match self {
            Mapping::Exact(resolved)
            | Mapping::Widened(resolved, _)
            | Mapping::TextFallback(resolved, _) => Some(resolved),
            Mapping::Refuse(_) => None,
        }
    }

    pub fn ddl(&self) -> Option<&str> {
        self.resolved().map(|resolved| resolved.ddl.as_str())
    }

    /// What changed (widened, fallback) or why not (refused).
    pub fn note(&self) -> Option<&str> {
        match self {
            Mapping::Exact(_) => None,
            Mapping::Widened(_, note) | Mapping::TextFallback(_, note) | Mapping::Refuse(note) => {
                Some(note)
            }
        }
    }
}

fn resolve(ty: TargetType, target: Target) -> Result<Resolved, Mapping> {
    match render(&ty, target) {
        Some(ddl) => Ok(Resolved { ty, ddl }),
        None => Err(Mapping::Refuse(format!(
            "{} has no such column type",
            target.label()
        ))),
    }
}

fn exact(ty: TargetType, target: Target) -> Mapping {
    resolve(ty, target).map_or_else(|refusal| refusal, Mapping::Exact)
}

fn widened(ty: TargetType, target: Target, note: &str) -> Mapping {
    resolve(ty, target).map_or_else(
        |refusal| refusal,
        |resolved| Mapping::Widened(resolved, note.to_owned()),
    )
}

fn fallback(ty: TargetType, target: Target, note: &str) -> Mapping {
    resolve(ty, target).map_or_else(
        |refusal| refusal,
        |resolved| Mapping::TextFallback(resolved, note.to_owned()),
    )
}

fn refuse(reason: &str) -> Mapping {
    Mapping::Refuse(reason.to_owned())
}

/// The longest `varchar(n)` PostgreSQL takes.
const PG_VARCHAR_MAX: u32 = 10_485_760;
/// Characters of a MySQL `varchar` that fit its 65,535-byte row at 4 bytes a character, less
/// the length prefix.
const MYSQL_VARCHAR_MAX: u32 = 16_383;
/// The widest MySQL `decimal` is 65 digits with at most 30 after the point.
const MYSQL_DECIMAL_DIGITS: u32 = 65;
const MYSQL_DECIMAL_SCALE: u32 = 30;
/// PostgreSQL's `numeric(p,s)` takes up to 1000 digits.
const PG_NUMERIC_DIGITS: u32 = 1000;
/// The widest fraction either dialect is asked to carry (OQ-10).
const MAX_FRACTION: u32 = 6;

fn mysql_text_for(chars: Option<u32>) -> TargetType {
    match chars {
        None => TargetType::Text(TextSize::Long),
        Some(n) if n <= MYSQL_VARCHAR_MAX => TargetType::Varchar(n),
        Some(n) if u64::from(n) * 4 <= 16_777_215 => TargetType::Text(TextSize::Medium),
        Some(_) => TargetType::Text(TextSize::Long),
    }
}

/// The outcome of writing a column of type `ty`, read from `source`, to `target`.
///
/// Total: every base has one arm for each dialect. `source` matters in the few places the
/// driver's handling of a type decides what can be carried: the PostgreSQL driver hands a
/// type it does not model over as text, and the Trino driver cannot read a `time with time
/// zone` at all.
pub fn map_type(ty: &SqlType, source: DriverKind, target: Target) -> Mapping {
    let from_pg = source == DriverKind::Postgres;
    let digits = ty.fraction();
    match ty.base {
        Base::Bool => match target {
            Target::Postgres => exact(TargetType::Boolean, target),
            Target::Mysql => widened(
                TargetType::Boolean,
                target,
                "boolean is written as tinyint(1)",
            ),
        },

        Base::Int8 | Base::Int16 | Base::Int24 | Base::Int32 | Base::Int64 => {
            let bytes = ty.base.int_bytes().unwrap_or(8);
            let unsigned = ty.unsigned;
            match (target, bytes, unsigned) {
                (Target::Mysql, ..) => exact(TargetType::Int { bytes, unsigned }, target),
                (Target::Postgres, 2 | 4 | 8, false) => {
                    exact(TargetType::Int { bytes, unsigned }, target)
                }
                (Target::Postgres, 1, _) => widened(
                    TargetType::Int {
                        bytes: 2,
                        unsigned: false,
                    },
                    target,
                    "PostgreSQL has no one-byte integer; written as smallint",
                ),
                (Target::Postgres, 2, true) => widened(
                    TargetType::Int {
                        bytes: 4,
                        unsigned: false,
                    },
                    target,
                    "PostgreSQL has no unsigned integers; written as integer",
                ),
                (Target::Postgres, 3, _) => widened(
                    TargetType::Int {
                        bytes: 4,
                        unsigned: false,
                    },
                    target,
                    "PostgreSQL has no three-byte integer; written as integer",
                ),
                (Target::Postgres, 4, true) => widened(
                    TargetType::Int {
                        bytes: 8,
                        unsigned: false,
                    },
                    target,
                    "PostgreSQL has no unsigned integers; written as bigint",
                ),
                (Target::Postgres, _, _) => widened(
                    TargetType::Numeric {
                        precision: Some(20),
                        scale: 0,
                    },
                    target,
                    "PostgreSQL has no unsigned bigint; written as numeric(20,0)",
                ),
            }
        }

        Base::Float32 => exact(TargetType::Float { double: false }, target),
        Base::Float64 => exact(TargetType::Float { double: true }, target),

        Base::Decimal => map_decimal(ty, target),

        Base::Text => match (target, ty.length) {
            (Target::Postgres, None) => exact(TargetType::Text(TextSize::Text), target),
            (Target::Postgres, Some(n)) if n <= PG_VARCHAR_MAX => {
                exact(TargetType::Varchar(n), target)
            }
            (Target::Postgres, Some(_)) => widened(
                TargetType::Text(TextSize::Text),
                target,
                "the length is beyond PostgreSQL's varchar limit; written as text",
            ),
            (Target::Mysql, None) => widened(
                TargetType::Text(TextSize::Long),
                target,
                "text of no declared length is written as longtext",
            ),
            (Target::Mysql, Some(n)) => {
                let wanted = mysql_text_for(Some(n));
                if wanted == TargetType::Varchar(n) {
                    exact(wanted, target)
                } else {
                    widened(
                        wanted,
                        target,
                        "the length is beyond a MySQL varchar row; written as a text type",
                    )
                }
            }
        },

        Base::Char => match (target, ty.length) {
            (_, None) => refuse("a char column without a length"),
            (Target::Postgres, Some(n)) if n <= PG_VARCHAR_MAX => {
                exact(TargetType::Char(n), target)
            }
            (Target::Postgres, Some(_)) => refuse("a char column longer than PostgreSQL allows"),
            (Target::Mysql, Some(n)) if n <= 255 => {
                if source == DriverKind::Mysql {
                    exact(TargetType::Char(n), target)
                } else {
                    widened(
                        TargetType::Char(n),
                        target,
                        "MySQL does not keep the padding of a char value",
                    )
                }
            }
            (Target::Mysql, Some(n)) => widened(
                mysql_text_for(Some(n)),
                target,
                "the length is beyond MySQL's char limit; written as varchar",
            ),
        },

        Base::Binary => match (target, ty.length) {
            (Target::Postgres, _) => exact(TargetType::Binary(None), target),
            (Target::Mysql, Some(n)) if n <= 65_535 => exact(TargetType::Binary(Some(n)), target),
            (Target::Mysql, _) => widened(
                TargetType::Binary(None),
                target,
                "binary data of no fitting length is written as longblob",
            ),
        },

        Base::Uuid => match target {
            Target::Postgres => exact(TargetType::Uuid, target),
            Target::Mysql => fallback(
                TargetType::Char(36),
                target,
                "MySQL has no uuid type; written as char(36) text",
            ),
        },

        Base::Date => exact(TargetType::Date, target),

        Base::Time => {
            if digits > MAX_FRACTION {
                return refuse("fractional seconds beyond microseconds are not carried");
            }
            exact(TargetType::Time { digits }, target)
        }
        Base::TimeTz => {
            if digits > MAX_FRACTION {
                return refuse("fractional seconds beyond microseconds are not carried");
            }
            if source == DriverKind::Trino {
                return refuse("the Trino driver cannot read a time with time zone");
            }
            match target {
                Target::Postgres => exact(TargetType::TimeTz { digits }, target),
                Target::Mysql => refuse("MySQL has no time with a time zone"),
            }
        }
        Base::Timestamp => {
            if digits > MAX_FRACTION {
                return refuse("fractional seconds beyond microseconds are not carried");
            }
            let session = ty.time_zone == TimeZone::Session;
            let note = "a MySQL timestamp is the wall clock of the source session's time zone";
            match (target, session) {
                (Target::Postgres, false) => exact(TargetType::Timestamp { digits }, target),
                (Target::Postgres, true) => {
                    fallback(TargetType::Timestamp { digits }, target, note)
                }
                (Target::Mysql, false) => exact(TargetType::DateTime { digits }, target),
                (Target::Mysql, true) => fallback(TargetType::DateTime { digits }, target, note),
            }
        }
        Base::TimestampTz => {
            if digits > MAX_FRACTION {
                return refuse("fractional seconds beyond microseconds are not carried");
            }
            match target {
                Target::Postgres => exact(TargetType::TimestampTz { digits }, target),
                Target::Mysql => fallback(
                    TargetType::DateTime { digits },
                    target,
                    "MySQL has no time zone in a datetime; written as the UTC wall clock",
                ),
            }
        }
        Base::Interval => match target {
            Target::Postgres => exact(TargetType::Interval, target),
            Target::Mysql => refuse("MySQL has no interval type"),
        },

        Base::Json => match target {
            Target::Postgres => exact(TargetType::Json, target),
            Target::Mysql => widened(
                TargetType::Json,
                target,
                "MySQL json normalises key order and spacing",
            ),
        },
        Base::Jsonb => match target {
            Target::Postgres => exact(TargetType::Jsonb, target),
            Target::Mysql => widened(
                TargetType::Json,
                target,
                "MySQL json normalises key order and spacing",
            ),
        },

        Base::Array => map_array(ty, source, target),
        Base::Map | Base::Row => match target {
            Target::Postgres => fallback(
                TargetType::Jsonb,
                target,
                if ty.base == Base::Row {
                    "a row is written as a jsonb array; its field names are not kept"
                } else {
                    "a map is written as a jsonb object"
                },
            ),
            Target::Mysql => fallback(
                TargetType::Json,
                target,
                if ty.base == Base::Row {
                    "a row is written as a json array; its field names are not kept"
                } else {
                    "a map is written as a json object"
                },
            ),
        },

        Base::Enum | Base::Set => match target {
            Target::Postgres => fallback(
                TargetType::Text(TextSize::Text),
                target,
                "an enumerated value is written as text",
            ),
            Target::Mysql => fallback(
                TargetType::Varchar(255),
                target,
                "an enumerated value is written as varchar(255)",
            ),
        },

        Base::Builtin(kind) => match target {
            Target::Postgres if from_pg => {
                let length = matches!(kind, PgBuiltin::Bit | PgBuiltin::Varbit)
                    .then_some(ty.length)
                    .flatten();
                if kind == PgBuiltin::Bit && length.is_none() {
                    return refuse("a bit column without a length");
                }
                exact(TargetType::Builtin { kind, length }, target)
            }
            _ => refuse("this PostgreSQL type can only be written to PostgreSQL"),
        },

        Base::Other => refuse("this column type has no faithful counterpart"),
    }
}

fn map_decimal(ty: &SqlType, target: Target) -> Mapping {
    let scale = match ty.scale {
        None => 0,
        Some(scale) if scale >= 0 => scale as u32,
        Some(_) => return refuse("a decimal with a negative scale"),
    };
    match (target, ty.precision) {
        (Target::Postgres, None) => exact(
            TargetType::Numeric {
                precision: None,
                scale: 0,
            },
            target,
        ),
        (Target::Postgres, Some(p)) => {
            if p == 0 || p > PG_NUMERIC_DIGITS || scale > p {
                return refuse("a decimal PostgreSQL cannot declare");
            }
            exact(
                TargetType::Numeric {
                    precision: Some(p),
                    scale,
                },
                target,
            )
        }
        (Target::Mysql, None) => widened(
            TargetType::Numeric {
                precision: Some(MYSQL_DECIMAL_DIGITS),
                scale: MYSQL_DECIMAL_SCALE,
            },
            target,
            "a decimal of no declared size is written as decimal(65,30); a value with more \
             than 30 decimals or 35 whole digits is refused",
        ),
        (Target::Mysql, Some(p)) => {
            if p == 0 || p > MYSQL_DECIMAL_DIGITS || scale > MYSQL_DECIMAL_SCALE || scale > p {
                return refuse("a decimal beyond MySQL's 65 digits or 30 decimals");
            }
            exact(
                TargetType::Numeric {
                    precision: Some(p),
                    scale,
                },
                target,
            )
        }
    }
}

fn map_array(ty: &SqlType, source: DriverKind, target: Target) -> Mapping {
    let element = ty.element.as_deref();
    match target {
        Target::Postgres => {
            // An array whose element has an exact PostgreSQL type is written as that type's
            // array. Nested arrays, maps and rows are not "an element type": they go as JSON.
            let typed = element.and_then(|element| {
                if matches!(element.base, Base::Array | Base::Map | Base::Row) {
                    return None;
                }
                match map_type(element, source, Target::Postgres) {
                    Mapping::Exact(resolved) => Some(resolved.ty),
                    _ => None,
                }
            });
            match typed {
                Some(inner) => exact(TargetType::Array(Box::new(inner)), target),
                None if source == DriverKind::Postgres => refuse(
                    "the PostgreSQL driver hands an array over as text, which can only be \
                     written to an array of an element type that is written exactly",
                ),
                None => fallback(
                    TargetType::Jsonb,
                    target,
                    "an array of this element type is written as jsonb",
                ),
            }
        }
        Target::Mysql => {
            if source == DriverKind::Postgres {
                refuse(
                    "the PostgreSQL driver hands an array over as text, and turning that text \
                     into JSON is not built",
                )
            } else {
                fallback(
                    TargetType::Json,
                    target,
                    "MySQL has no array type; written as json",
                )
            }
        }
    }
}

/// A column of the table to be created, with everything the encoder needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedColumn {
    pub name: String,
    pub ty: SqlType,
    pub source: DriverKind,
    pub target: Target,
    /// What the column is written as. Present for every column of a plan: a column with no
    /// faithful target refuses the plan instead of being in it.
    pub resolved: Resolved,
    /// Never [`Outcome::Refuse`].
    pub outcome: Outcome,
    /// What changed, for a widened or fallback column.
    pub note: Option<String>,
}

impl PlannedColumn {
    /// A column from a mapping that is not a refusal.
    fn from_mapping(
        name: &str,
        ty: &SqlType,
        source: DriverKind,
        target: Target,
        mapping: Mapping,
    ) -> Result<Self, String> {
        let outcome = mapping.outcome();
        let note = mapping.note().map(str::to_owned);
        match mapping {
            Mapping::Exact(resolved)
            | Mapping::Widened(resolved, _)
            | Mapping::TextFallback(resolved, _) => Ok(PlannedColumn {
                name: name.to_owned(),
                ty: ty.clone(),
                source,
                target,
                resolved,
                outcome,
                note,
            }),
            Mapping::Refuse(reason) => Err(reason),
        }
    }
}

/// A report line for a column that did not map exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub column: String,
    /// The source type as the server spelled it (for the report; never SQL).
    pub from: String,
    pub to: String,
    pub note: String,
}

/// The table a transfer creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePlan {
    pub target: Target,
    pub columns: Vec<PlannedColumn>,
}

/// Why a table cannot be created faithfully. Raised before the first row is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The offending column, when one is.
    pub column: Option<usize>,
    pub reason: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for Refusal {}

/// A name for a message: quoted and escaped, so a newline or a control character in it
/// cannot reshape the line.
fn shown(name: &str) -> String {
    format!("{name:?}")
}

fn describe(ty: &SqlType) -> String {
    if ty.native.is_empty() {
        ty.base.label().to_owned()
    } else {
        ty.native.clone()
    }
}

/// Plan the target table for the columns of a result.
///
/// Checks every name (§4.5), maps every column (§4.3), and for MySQL fits the row into its
/// 65,535 bytes by writing the widest columns as text types, reported. The first column with
/// no faithful target refuses the whole table, naming it and why.
pub fn plan_table(
    source: DriverKind,
    target: Target,
    columns: &[(&str, &SqlType)],
) -> Result<TablePlan, Refusal> {
    let names: Vec<&str> = columns.iter().map(|(name, _)| *name).collect();
    if let Err(fault) = names::check_columns(target, &names) {
        return Err(Refusal {
            column: Some(fault.index),
            reason: format!("column {} {}", shown(names[fault.index]), fault.error),
        });
    }
    let mut planned = Vec::with_capacity(columns.len());
    for (index, (name, ty)) in columns.iter().enumerate() {
        let column =
            PlannedColumn::from_mapping(name, ty, source, target, map_type(ty, source, target))
                .map_err(|reason| Refusal {
                    column: Some(index),
                    reason: format!(
                        "column {} ({}) cannot be written to {} faithfully: {reason}",
                        shown(name),
                        describe(ty),
                        target.label()
                    ),
                })?;
        planned.push(column);
    }
    if target == Target::Mysql {
        fit_mysql_row(&mut planned)?;
    }
    Ok(TablePlan {
        target,
        columns: planned,
    })
}

/// Bytes a MySQL column can take of the 65,535-byte row, taken high so a row that fits by
/// this count fits for real. Text and blob columns count their pointer.
fn mysql_row_bytes(ty: &TargetType) -> u64 {
    fn decimal_bytes(digits: u32) -> u64 {
        const LEFTOVER: [u64; 9] = [0, 1, 1, 2, 2, 3, 3, 4, 4];
        u64::from(digits / 9) * 4 + LEFTOVER[(digits % 9) as usize]
    }
    match ty {
        TargetType::Boolean => 1,
        TargetType::Int { bytes, .. } => u64::from(*bytes),
        TargetType::Numeric { precision, scale } => {
            let p = precision.unwrap_or(MYSQL_DECIMAL_DIGITS);
            decimal_bytes(p.saturating_sub(*scale)) + decimal_bytes(*scale)
        }
        TargetType::Float { double } => {
            if *double {
                8
            } else {
                4
            }
        }
        TargetType::Varchar(n) => {
            let bytes = u64::from(*n) * 4;
            bytes + if bytes > 255 { 2 } else { 1 }
        }
        TargetType::Char(n) => u64::from(*n) * 4,
        TargetType::Binary(Some(n)) => u64::from(*n) + if *n > 255 { 2 } else { 1 },
        TargetType::Date => 3,
        TargetType::Time { digits } => 3 + u64::from(digits.div_ceil(2)),
        TargetType::DateTime { digits } => 5 + u64::from(digits.div_ceil(2)),
        _ => 12,
    }
}

const MYSQL_ROW_LIMIT: u64 = 65_535;

/// Fit the row into MySQL's limit by demoting the widest varchar, char or varbinary column
/// to a text or blob type, one at a time, reporting each.
fn fit_mysql_row(columns: &mut [PlannedColumn]) -> Result<(), Refusal> {
    loop {
        let total: u64 = columns
            .iter()
            .map(|column| mysql_row_bytes(&column.resolved.ty))
            .sum();
        if total <= MYSQL_ROW_LIMIT {
            return Ok(());
        }
        // The widest column that has a bigger sibling type to move to, with what it moves to.
        let widest = columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                let (demoted, note) = match column.resolved.ty {
                    TargetType::Binary(Some(_)) => (
                        TargetType::Binary(None),
                        "written as longblob so the row fits MySQL's 65,535 bytes",
                    ),
                    TargetType::Varchar(n) | TargetType::Char(n) => (
                        mysql_text_size_for(n),
                        "written as a text type so the row fits MySQL's 65,535 bytes",
                    ),
                    _ => return None,
                };
                Some((mysql_row_bytes(&column.resolved.ty), index, demoted, note))
            })
            .max_by_key(|(bytes, ..)| *bytes);
        let Some((_, index, demoted, note)) = widest else {
            return Err(Refusal {
                column: None,
                reason: "the row is wider than MySQL's 65,535 bytes even with every long \
                         column written as a text type"
                    .to_owned(),
            });
        };
        let Some(ddl) = render(&demoted, Target::Mysql) else {
            return Err(Refusal {
                column: Some(index),
                reason: "a column type MySQL cannot declare".to_owned(),
            });
        };
        let column = &mut columns[index];
        column.resolved = Resolved { ty: demoted, ddl };
        if column.outcome == Outcome::Exact {
            column.outcome = Outcome::Widened;
        }
        column.note = Some(match column.note.take() {
            Some(old) => format!("{old}; {note}"),
            None => note.to_owned(),
        });
    }
}

/// The text size for a demoted varchar of `n` characters.
fn mysql_text_size_for(n: u32) -> TargetType {
    if u64::from(n) * 4 <= 65_535 {
        TargetType::Text(TextSize::Text)
    } else if u64::from(n) * 4 <= 16_777_215 {
        TargetType::Text(TextSize::Medium)
    } else {
        TargetType::Text(TextSize::Long)
    }
}

impl TablePlan {
    /// The columns written as a documented stand-in, for the `start` event's `fallbacks`.
    pub fn fallbacks(&self) -> Vec<Change> {
        self.changes(Outcome::TextFallback)
    }

    /// The columns written as a wider type, for the summary's warnings.
    pub fn widenings(&self) -> Vec<Change> {
        self.changes(Outcome::Widened)
    }

    fn changes(&self, wanted: Outcome) -> Vec<Change> {
        self.columns
            .iter()
            .filter(|column| column.outcome == wanted)
            .map(|column| Change {
                column: column.name.clone(),
                from: describe(&column.ty),
                to: column.resolved.ddl.clone(),
                note: column.note.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// `CREATE TABLE <table> (<columns>)`, every name through [`names::ident`] and every type
    /// from [`TargetType`]. `table` is a name already qualified by [`names::qualified`].
    /// MySQL tables are created `DEFAULT CHARSET=utf8mb4`, so they do not inherit a database
    /// default that may be latin1.
    pub fn create_table_sql(&self, table: &str) -> String {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|column| {
                format!(
                    "{} {}",
                    names::ident(self.target, &column.name),
                    column.resolved.ddl
                )
            })
            .collect();
        let suffix = match self.target {
            Target::Postgres => "",
            Target::Mysql => " DEFAULT CHARSET=utf8mb4",
        };
        format!("CREATE TABLE {table} ({}){suffix}", columns.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_ok(ty: &SqlType, source: DriverKind, target: Target) -> String {
        map_type(ty, source, target)
            .ddl()
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn all_bases_are_listed_exactly_once() {
        // The match has no wildcard: a new base fails to compile here until it is placed
        // in `all()`, which the fixture coverage test then walks.
        fn index(base: Base) -> usize {
            match base {
                Base::Bool => 0,
                Base::Int8 => 1,
                Base::Int16 => 2,
                Base::Int24 => 3,
                Base::Int32 => 4,
                Base::Int64 => 5,
                Base::Float32 => 6,
                Base::Float64 => 7,
                Base::Decimal => 8,
                Base::Text => 9,
                Base::Char => 10,
                Base::Binary => 11,
                Base::Uuid => 12,
                Base::Date => 13,
                Base::Time => 14,
                Base::TimeTz => 15,
                Base::Timestamp => 16,
                Base::TimestampTz => 17,
                Base::Interval => 18,
                Base::Json => 19,
                Base::Jsonb => 20,
                Base::Array => 21,
                Base::Map => 22,
                Base::Row => 23,
                Base::Enum => 24,
                Base::Set => 25,
                Base::Other => 26,
                Base::Builtin(_) => 27,
            }
        }
        let all = Base::all();
        let mut seen = std::collections::HashSet::new();
        for base in &all {
            assert!(seen.insert(*base), "{base:?} listed twice");
        }
        let kinds: std::collections::HashSet<usize> = all.iter().map(|b| index(*b)).collect();
        assert_eq!(kinds.len(), 28);
        assert_eq!(all.len(), 27 + PgBuiltin::ALL.len());
    }

    #[test]
    fn the_allowlist_is_by_oid_and_leaves_out_money_oid_and_the_reg_types() {
        for kind in PgBuiltin::ALL {
            assert_eq!(PgBuiltin::from_oid(kind.oid()), Some(kind));
        }
        // money (C-1), oid, regclass, regproc, and a few that merely look harmless.
        for oid in [790, 26, 2205, 24, 2206, 1700, 25, 2950, 0, 1] {
            assert_eq!(PgBuiltin::from_oid(oid), None, "{oid}");
        }
        let mut oids: Vec<u32> = PgBuiltin::ALL.iter().map(|k| k.oid()).collect();
        oids.sort_unstable();
        oids.dedup();
        assert_eq!(oids.len(), PgBuiltin::ALL.len());
    }

    #[test]
    fn integers_map_as_the_blueprint_table_says() {
        use DriverKind::Mysql as M;
        let cases = [
            (Base::Int8, false, "smallint", "tinyint"),
            (Base::Int8, true, "smallint", "tinyint unsigned"),
            (Base::Int16, false, "smallint", "smallint"),
            (Base::Int16, true, "integer", "smallint unsigned"),
            (Base::Int24, false, "integer", "mediumint"),
            (Base::Int24, true, "integer", "mediumint unsigned"),
            (Base::Int32, false, "integer", "int"),
            (Base::Int32, true, "bigint", "int unsigned"),
            (Base::Int64, false, "bigint", "bigint"),
            (Base::Int64, true, "numeric(20,0)", "bigint unsigned"),
        ];
        for (base, unsigned, pg, my) in cases {
            let mut ty = SqlType::of(base);
            ty.unsigned = unsigned;
            assert_eq!(
                render_ok(&ty, M, Target::Postgres),
                pg,
                "{base:?} {unsigned}"
            );
            assert_eq!(render_ok(&ty, M, Target::Mysql), my, "{base:?} {unsigned}");
        }
        // Exact where the type is the same, widened where it had to grow.
        let exact_pg = map_type(&SqlType::of(Base::Int32), M, Target::Postgres);
        assert_eq!(exact_pg.outcome(), Outcome::Exact);
        let wide = map_type(&SqlType::of(Base::Int8), M, Target::Postgres);
        assert_eq!(wide.outcome(), Outcome::Widened);
    }

    #[test]
    fn a_server_supplied_type_text_never_reaches_sql() {
        let hostile = "int); DROP TABLE x; --";
        for base in Base::all() {
            let mut ty = SqlType::of(base).with_native(hostile);
            ty.element = Some(Box::new(SqlType::of(Base::Int32).with_native(hostile)));
            ty.length = Some(10);
            for target in Target::ALL {
                let mapping = map_type(&ty, DriverKind::Postgres, target);
                if let Some(ddl) = mapping.ddl() {
                    assert!(!ddl.contains("DROP"), "{base:?} -> {ddl}");
                    assert!(!ddl.contains(';'), "{base:?} -> {ddl}");
                }
            }
        }
        let ty = SqlType::of(Base::Other)
            .with_native(hostile)
            .with_oid(999_999);
        let plan = plan_table(DriverKind::Postgres, Target::Postgres, &[("a", &ty)]);
        // Refused, and the message may carry the text; SQL never does.
        assert!(plan.is_err());
        let good = SqlType::of(Base::Int32).with_native(hostile);
        let plan = plan_table(DriverKind::Postgres, Target::Postgres, &[("a", &good)]).unwrap();
        let sql = plan.create_table_sql("t");
        assert_eq!(sql, "CREATE TABLE t (a integer)");
    }

    #[test]
    fn decimals_follow_the_limits_of_each_dialect() {
        let pg = DriverKind::Postgres;
        let d = |p, s| SqlType::decimal(p, s);
        assert_eq!(render_ok(&d(10, 2), pg, Target::Postgres), "numeric(10,2)");
        assert_eq!(render_ok(&d(10, 2), pg, Target::Mysql), "decimal(10,2)");
        assert_eq!(render_ok(&d(65, 30), pg, Target::Mysql), "decimal(65,30)");
        // Beyond MySQL: a refusal, never a rounding.
        assert_eq!(
            map_type(&d(66, 2), pg, Target::Mysql).outcome(),
            Outcome::Refuse
        );
        assert_eq!(
            map_type(&d(38, 31), pg, Target::Mysql).outcome(),
            Outcome::Refuse
        );
        assert_eq!(render_ok(&d(66, 2), pg, Target::Postgres), "numeric(66,2)");
        // Unbounded PostgreSQL numeric.
        let unbounded = SqlType::of(Base::Decimal);
        assert_eq!(render_ok(&unbounded, pg, Target::Postgres), "numeric");
        let mapped = map_type(&unbounded, pg, Target::Mysql);
        assert_eq!(mapped.outcome(), Outcome::Widened);
        assert_eq!(mapped.ddl(), Some("decimal(65,30)"));
        // A negative scale is not modelled.
        assert_eq!(
            map_type(&d(5, -2), pg, Target::Postgres).outcome(),
            Outcome::Refuse
        );
    }

    #[test]
    fn times_beyond_microseconds_and_time_zone_times_are_refused() {
        let trino = DriverKind::Trino;
        for base in [Base::Time, Base::TimeTz, Base::Timestamp, Base::TimestampTz] {
            let wide = SqlType::of(base).with_fraction(9);
            for target in Target::ALL {
                assert_eq!(
                    map_type(&wide, trino, target).outcome(),
                    Outcome::Refuse,
                    "{base:?} {target:?}"
                );
            }
            let ok = SqlType::of(base).with_fraction(6);
            assert_ne!(
                map_type(&ok, DriverKind::Postgres, Target::Postgres).outcome(),
                Outcome::Refuse,
                "{base:?}"
            );
        }
        // The Trino driver cannot read the value, so the column is refused up front.
        let tz = SqlType::of(Base::TimeTz).with_fraction(3);
        assert_eq!(
            map_type(&tz, trino, Target::Postgres).outcome(),
            Outcome::Refuse
        );
        assert_eq!(
            map_type(&tz, DriverKind::Postgres, Target::Postgres).ddl(),
            Some("timetz(3)")
        );
        assert_eq!(
            map_type(&tz, DriverKind::Postgres, Target::Mysql).outcome(),
            Outcome::Refuse
        );
    }

    #[test]
    fn text_lengths_pick_the_type_that_holds_them() {
        let pg = DriverKind::Postgres;
        let text = |n| SqlType::of(Base::Text).with_length(n);
        assert_eq!(render_ok(&text(20), pg, Target::Mysql), "varchar(20)");
        assert_eq!(
            render_ok(&text(16_383), pg, Target::Mysql),
            "varchar(16383)"
        );
        assert_eq!(render_ok(&text(16_384), pg, Target::Mysql), "mediumtext");
        assert_eq!(render_ok(&text(5_000_000), pg, Target::Mysql), "longtext");
        assert_eq!(
            render_ok(&SqlType::of(Base::Text), pg, Target::Mysql),
            "longtext"
        );
        assert_eq!(
            render_ok(&SqlType::of(Base::Text), pg, Target::Postgres),
            "text"
        );
        assert_eq!(
            render_ok(&text(20), DriverKind::Trino, Target::Postgres),
            "varchar(20)"
        );
        // Trino varchar(n) beyond PostgreSQL's limit is text, reported.
        let huge = map_type(&text(2_000_000_000), DriverKind::Trino, Target::Postgres);
        assert_eq!(huge.outcome(), Outcome::Widened);
        assert_eq!(huge.ddl(), Some("text"));
    }

    #[test]
    fn a_mysql_row_that_is_too_wide_demotes_its_widest_columns_and_says_so() {
        let wide = SqlType::of(Base::Text).with_length(10_000);
        let columns: Vec<(String, SqlType)> =
            (0..3).map(|i| (format!("c{i}"), wide.clone())).collect();
        let refs: Vec<(&str, &SqlType)> = columns.iter().map(|(n, t)| (n.as_str(), t)).collect();
        let plan = plan_table(DriverKind::Mysql, Target::Mysql, &refs).unwrap();
        // 3 x (40,000 + 2) is 120,006 bytes: two of them become text, the third fits.
        let ddls: Vec<&str> = plan
            .columns
            .iter()
            .map(|c| c.resolved.ddl.as_str())
            .collect();
        assert_eq!(
            ddls.iter().filter(|d| **d == "varchar(10000)").count(),
            1,
            "{ddls:?}"
        );
        assert_eq!(ddls.iter().filter(|d| **d == "text").count(), 2, "{ddls:?}");
        assert_eq!(plan.widenings().len(), 2);
        let sql = plan.create_table_sql("`t`");
        assert!(sql.ends_with(" DEFAULT CHARSET=utf8mb4"), "{sql}");
    }

    #[test]
    fn the_create_statement_quotes_names_and_takes_types_from_the_engine() {
        let id = SqlType::of(Base::Int64);
        let note = SqlType::of(Base::Text).with_length(40);
        let columns = [("id", &id), ("Order", &note), ("we\"ird", &id)];
        let plan = plan_table(DriverKind::Postgres, Target::Postgres, &columns).unwrap();
        assert_eq!(
            plan.create_table_sql("public.t"),
            "CREATE TABLE public.t (id bigint, \"Order\" varchar(40), \"we\"\"ird\" bigint)"
        );
        let plan = plan_table(DriverKind::Postgres, Target::Mysql, &columns).unwrap();
        assert_eq!(
            plan.create_table_sql("`shop`.`t`"),
            "CREATE TABLE `shop`.`t` (id bigint, `Order` varchar(40), `we\"ird` bigint) DEFAULT CHARSET=utf8mb4"
        );
    }

    #[test]
    fn a_table_with_a_bad_name_or_no_faithful_column_is_refused_with_the_column() {
        let int = SqlType::of(Base::Int32);
        let other = SqlType::of(Base::Other).with_native("geometry");
        let refusal = plan_table(
            DriverKind::Postgres,
            Target::Postgres,
            &[("a", &int), ("shape", &other)],
        )
        .unwrap_err();
        assert_eq!(refusal.column, Some(1));
        assert!(refusal.reason.contains("\"shape\""), "{refusal}");
        assert!(refusal.reason.contains("geometry"), "{refusal}");

        let refusal = plan_table(
            DriverKind::Postgres,
            Target::Postgres,
            &[("a", &int), ("a", &int)],
        )
        .unwrap_err();
        assert_eq!(refusal.column, Some(1));
        assert!(refusal.reason.contains("more than once"), "{refusal}");
        // A name with a newline is shown escaped, so it cannot reshape the message.
        let refusal = plan_table(
            DriverKind::Mysql,
            Target::Mysql,
            &[("a\n", &int), ("a\n", &int)],
        )
        .unwrap_err();
        assert!(!refusal.reason.contains('\n'), "{refusal:?}");
    }
}
