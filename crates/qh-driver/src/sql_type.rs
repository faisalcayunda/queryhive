//! What a driver says about one result column: the structured descriptor of W13-T20 §4.1.
//!
//! A column is described by data, never by the server's type text. [`SqlType::native`] keeps
//! that text for messages only, and nothing builds SQL from it: the writer's DDL comes from a
//! table the engine owns (`qh-ffi`'s `transfer::types`), keyed on the parsed fields here.
//!
//! The descriptor lives next to [`crate::Cursor`] because a cursor is what reports it
//! ([`crate::Cursor::column_types`]), and each driver fills it in from what its server says:
//! PostgreSQL from the type OID and modifier, MySQL from the column definition packet, Trino
//! from the type text. The mapping to a target dialect is the transfer's.

/// PostgreSQL's built-in types outside the logical model that can be written to another
/// PostgreSQL by name: the allowlist of W13-T20 §4.3.
///
/// By OID, never by name: a domain or a user type named like a built-in is not one. `oid`
/// and the `reg*` family are left out because their value means something only inside one
/// database; `money` is left out because its text depends on `lc_monetary` (condition C-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PgBuiltin {
    Inet,
    Cidr,
    Macaddr,
    Macaddr8,
    Bit,
    Varbit,
    Xml,
    Tsvector,
    Tsquery,
    Point,
    Line,
    Lseg,
    GeoBox,
    Path,
    Polygon,
    Circle,
}

impl PgBuiltin {
    pub const ALL: [PgBuiltin; 16] = [
        PgBuiltin::Inet,
        PgBuiltin::Cidr,
        PgBuiltin::Macaddr,
        PgBuiltin::Macaddr8,
        PgBuiltin::Bit,
        PgBuiltin::Varbit,
        PgBuiltin::Xml,
        PgBuiltin::Tsvector,
        PgBuiltin::Tsquery,
        PgBuiltin::Point,
        PgBuiltin::Line,
        PgBuiltin::Lseg,
        PgBuiltin::GeoBox,
        PgBuiltin::Path,
        PgBuiltin::Polygon,
        PgBuiltin::Circle,
    ];

    /// The type's OID in `pg_catalog`, fixed by the server and equal in every database.
    pub const fn oid(self) -> u32 {
        match self {
            PgBuiltin::Inet => 869,
            PgBuiltin::Cidr => 650,
            PgBuiltin::Macaddr => 829,
            PgBuiltin::Macaddr8 => 774,
            PgBuiltin::Bit => 1560,
            PgBuiltin::Varbit => 1562,
            PgBuiltin::Xml => 142,
            PgBuiltin::Tsvector => 3614,
            PgBuiltin::Tsquery => 3615,
            PgBuiltin::Point => 600,
            PgBuiltin::Line => 628,
            PgBuiltin::Lseg => 601,
            PgBuiltin::GeoBox => 603,
            PgBuiltin::Path => 602,
            PgBuiltin::Polygon => 604,
            PgBuiltin::Circle => 718,
        }
    }

    /// The allowlist lookup. `None` for everything else, including `money` (790), `oid`
    /// (26) and the `reg*` types (for example `regclass`, 2205).
    pub fn from_oid(oid: u32) -> Option<PgBuiltin> {
        PgBuiltin::ALL.into_iter().find(|kind| kind.oid() == oid)
    }

    /// The name the type is written under in DDL.
    pub const fn ddl_name(self) -> &'static str {
        match self {
            PgBuiltin::Inet => "inet",
            PgBuiltin::Cidr => "cidr",
            PgBuiltin::Macaddr => "macaddr",
            PgBuiltin::Macaddr8 => "macaddr8",
            PgBuiltin::Bit => "bit",
            PgBuiltin::Varbit => "varbit",
            PgBuiltin::Xml => "xml",
            PgBuiltin::Tsvector => "tsvector",
            PgBuiltin::Tsquery => "tsquery",
            PgBuiltin::Point => "point",
            PgBuiltin::Line => "line",
            PgBuiltin::Lseg => "lseg",
            PgBuiltin::GeoBox => "box",
            PgBuiltin::Path => "path",
            PgBuiltin::Polygon => "polygon",
            PgBuiltin::Circle => "circle",
        }
    }
}

/// The closed set of logical types a column can have (§4.2). Signedness and width of an
/// integer are `unsigned` and the variant; length, precision and scale are fields of
/// [`SqlType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Base {
    Bool,
    Int8,
    Int16,
    Int24,
    Int32,
    Int64,
    Float32,
    Float64,
    Decimal,
    Text,
    Char,
    Binary,
    Uuid,
    Date,
    Time,
    TimeTz,
    Timestamp,
    TimestampTz,
    Interval,
    Json,
    Jsonb,
    Array,
    Map,
    Row,
    Enum,
    Set,
    Builtin(PgBuiltin),
    Other,
}

impl Base {
    /// Every base, with the built-ins expanded. `all_bases_are_listed` holds it complete.
    pub fn all() -> Vec<Base> {
        let mut all = vec![
            Base::Bool,
            Base::Int8,
            Base::Int16,
            Base::Int24,
            Base::Int32,
            Base::Int64,
            Base::Float32,
            Base::Float64,
            Base::Decimal,
            Base::Text,
            Base::Char,
            Base::Binary,
            Base::Uuid,
            Base::Date,
            Base::Time,
            Base::TimeTz,
            Base::Timestamp,
            Base::TimestampTz,
            Base::Interval,
            Base::Json,
            Base::Jsonb,
            Base::Array,
            Base::Map,
            Base::Row,
            Base::Enum,
            Base::Set,
            Base::Other,
        ];
        all.extend(PgBuiltin::ALL.into_iter().map(Base::Builtin));
        all
    }

    /// The width in bytes of an integer base.
    pub const fn int_bytes(self) -> Option<u8> {
        match self {
            Base::Int8 => Some(1),
            Base::Int16 => Some(2),
            Base::Int24 => Some(3),
            Base::Int32 => Some(4),
            Base::Int64 => Some(8),
            _ => None,
        }
    }

    /// A short name for a message.
    pub fn label(self) -> &'static str {
        match self {
            Base::Bool => "boolean",
            Base::Int8 | Base::Int16 | Base::Int24 | Base::Int32 | Base::Int64 => "integer",
            Base::Float32 => "float",
            Base::Float64 => "double",
            Base::Decimal => "decimal",
            Base::Text => "text",
            Base::Char => "char",
            Base::Binary => "binary",
            Base::Uuid => "uuid",
            Base::Date => "date",
            Base::Time => "time",
            Base::TimeTz => "time with time zone",
            Base::Timestamp => "timestamp",
            Base::TimestampTz => "timestamp with time zone",
            Base::Interval => "interval",
            Base::Json => "json",
            Base::Jsonb => "jsonb",
            Base::Array => "array",
            Base::Map => "map",
            Base::Row => "row",
            Base::Enum => "enum",
            Base::Set => "set",
            Base::Builtin(kind) => kind.ddl_name(),
            Base::Other => "unsupported type",
        }
    }
}

/// How a time value relates to a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeZone {
    /// A wall clock with no zone: PostgreSQL `timestamp`, MySQL `datetime`, Trino `timestamp`.
    #[default]
    None,
    /// An instant, shown in an offset: the `with time zone` types.
    Offset,
    /// A wall clock the server printed in its **session** zone: MySQL `timestamp`. It is
    /// stored as UTC and comes back shifted, so the wall clock is the session's, and a
    /// copy of it into another server is not the same instant (a T cell, reported).
    Session,
}

/// What a driver says about one result column. The shape of W13-T20 §4.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlType {
    pub base: Base,
    /// Significant digits of a decimal. `None` is "unbounded" (PostgreSQL `numeric` without
    /// a modifier).
    pub precision: Option<u32>,
    /// Digits after the point of a decimal.
    pub scale: Option<i32>,
    /// Characters of a text or char; bytes of a binary; bits of a PostgreSQL bit string.
    /// `None` is "unbounded".
    pub length: Option<u32>,
    /// An unsigned integer (MySQL).
    pub unsigned: bool,
    pub time_zone: TimeZone,
    /// Fractional-second digits of a time or timestamp. `None` is read as 6, the widest this
    /// transfer carries, so an undescribed column is never narrowed.
    pub fraction_digits: Option<u32>,
    /// The element of an array.
    pub element: Option<Box<SqlType>>,
    /// The fields of a row.
    pub fields: Vec<SqlType>,
    /// The PostgreSQL type OID, when there is one.
    pub oid: Option<u32>,
    /// The server's own spelling of the type. **For messages only.** Nothing in this module
    /// puts it into SQL, and a test holds that.
    pub native: String,
}

impl SqlType {
    pub fn of(base: Base) -> Self {
        SqlType {
            base,
            precision: None,
            scale: None,
            length: None,
            unsigned: false,
            time_zone: TimeZone::None,
            fraction_digits: None,
            element: None,
            fields: Vec::new(),
            oid: None,
            native: String::new(),
        }
    }

    pub fn decimal(precision: u32, scale: i32) -> Self {
        SqlType {
            precision: Some(precision),
            scale: Some(scale),
            ..SqlType::of(Base::Decimal)
        }
    }

    pub fn with_length(mut self, length: u32) -> Self {
        self.length = Some(length);
        self
    }

    pub fn with_fraction(mut self, digits: u32) -> Self {
        self.fraction_digits = Some(digits);
        self
    }

    pub fn unsigned(mut self) -> Self {
        self.unsigned = true;
        self
    }

    pub fn with_zone(mut self, zone: TimeZone) -> Self {
        self.time_zone = zone;
        self
    }

    pub fn with_native(mut self, native: &str) -> Self {
        self.native = native.to_owned();
        self
    }

    pub fn with_oid(mut self, oid: u32) -> Self {
        self.oid = Some(oid);
        self
    }

    pub fn array_of(element: SqlType) -> Self {
        SqlType {
            element: Some(Box::new(element)),
            ..SqlType::of(Base::Array)
        }
    }

    /// The fractional digits a time or timestamp carries.
    pub fn fraction(&self) -> u32 {
        self.fraction_digits.unwrap_or(6)
    }
}
