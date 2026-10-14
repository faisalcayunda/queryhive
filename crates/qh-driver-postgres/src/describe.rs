//! A result column's type, as the structured [`SqlType`] a transfer maps (W13-T20 step 4).
//!
//! The server describes every column of a prepared statement with a type OID and a type
//! modifier, and nothing about it needs a round trip beyond the describe the driver already
//! makes. The modifier is where the precision lives, and it is an `i32` with a different
//! meaning per type (`atttypmod`):
//!
//! | Type | Modifier |
//! |---|---|
//! | `numeric(p,s)` | `-1`, or `((p << 16) | (s & 0x7ff)) + 4` (the scale is a signed 11-bit field since PostgreSQL 15, so `numeric(5,-2)` exists) |
//! | `varchar(n)`, `char(n)` | `-1`, or `n + 4` |
//! | `time`, `timetz`, `timestamp`, `timestamptz` | `-1`, or the fractional digits |
//! | `interval` | `-1`, or a field mask in the high bits and the fractional digits in the low 16 (`0xFFFF` is "none") |
//! | `bit(n)`, `varbit(n)` | `-1`, or `n` |
//!
//! Everything here is a pure function of the OID, the kind and the modifier, so it is proven
//! offline; what stays for a live server is that the server really sends these modifiers for
//! the columns of a described statement (including an expression's, which carries `-1`).
//!
//! A type that is not one of the logical set is `Other`, with its OID and name kept for the
//! message. A domain is `Other` too: the transfer must not write it as its base type behind
//! the user's back, and a PostgreSQL built-in is only ever picked by OID from
//! [`PgBuiltin::from_oid`], never by name.

use qh_driver::{Base, PgBuiltin, SqlType, TimeZone};
use tokio_postgres::types::{Kind, Type};

/// What PostgreSQL adds to a `varchar` or `numeric` modifier.
const VARHDRSZ: i32 = 4;

/// The descriptor of one column of type `ty` with modifier `typmod`.
pub fn sql_type(ty: &Type, typmod: i32) -> SqlType {
    let mut described = match ty.kind() {
        Kind::Array(element) => SqlType::array_of(sql_type(element, typmod)),
        Kind::Enum(_) => SqlType::of(Base::Enum),
        Kind::Simple => simple(ty.oid(), typmod),
        // A domain, composite, range, pseudo-type: not written as something it is not.
        _ => SqlType::of(Base::Other),
    };
    described.oid = Some(ty.oid());
    ty.name().clone_into(&mut described.native);
    described
}

fn simple(oid: u32, typmod: i32) -> SqlType {
    let sized = |base| {
        let mut ty = SqlType::of(base);
        if typmod >= VARHDRSZ {
            ty.length = Some((typmod - VARHDRSZ) as u32);
        }
        ty
    };
    let timed = |base, zone| {
        let mut ty = SqlType::of(base).with_zone(zone);
        if typmod >= 0 {
            ty.fraction_digits = Some(typmod as u32);
        }
        ty
    };
    match oid {
        16 => SqlType::of(Base::Bool),
        17 => SqlType::of(Base::Binary),
        20 => SqlType::of(Base::Int64),
        21 => SqlType::of(Base::Int16),
        23 => SqlType::of(Base::Int32),
        700 => SqlType::of(Base::Float32),
        701 => SqlType::of(Base::Float64),
        1700 => numeric(typmod),
        25 => SqlType::of(Base::Text),
        1043 => sized(Base::Text),
        1042 => sized(Base::Char),
        2950 => SqlType::of(Base::Uuid),
        1082 => SqlType::of(Base::Date),
        1083 => timed(Base::Time, TimeZone::None),
        1266 => timed(Base::TimeTz, TimeZone::Offset),
        1114 => timed(Base::Timestamp, TimeZone::None),
        1184 => timed(Base::TimestampTz, TimeZone::Offset),
        1186 => {
            let mut ty = SqlType::of(Base::Interval);
            // The low 16 bits are the fractional digits, `0xFFFF` meaning none were declared.
            if typmod >= 0 && (typmod & 0xFFFF) != 0xFFFF {
                ty.fraction_digits = Some((typmod & 0xFFFF) as u32);
            }
            ty
        }
        114 => SqlType::of(Base::Json),
        3802 => SqlType::of(Base::Jsonb),
        other => match PgBuiltin::from_oid(other) {
            Some(kind) => {
                let mut ty = SqlType::of(Base::Builtin(kind));
                if matches!(kind, PgBuiltin::Bit | PgBuiltin::Varbit) && typmod > 0 {
                    ty.length = Some(typmod as u32);
                }
                ty
            }
            None => SqlType::of(Base::Other),
        },
    }
}

fn numeric(typmod: i32) -> SqlType {
    if typmod < VARHDRSZ {
        return SqlType::of(Base::Decimal);
    }
    let packed = typmod - VARHDRSZ;
    let precision = ((packed >> 16) & 0xFFFF) as u32;
    // Eleven bits, signed: 0x400 is the sign bit.
    let scale = ((packed & 0x7FF) ^ 0x400) - 0x400;
    SqlType::decimal(precision, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifier(precision: i32, scale: i32) -> i32 {
        ((precision << 16) | (scale & 0x7FF)) + VARHDRSZ
    }

    #[test]
    fn numeric_reads_precision_and_a_signed_scale_out_of_the_modifier() {
        let ty = sql_type(&Type::NUMERIC, modifier(10, 2));
        assert_eq!(
            (ty.base, ty.precision, ty.scale),
            (Base::Decimal, Some(10), Some(2))
        );
        let ty = sql_type(&Type::NUMERIC, modifier(38, 0));
        assert_eq!((ty.precision, ty.scale), (Some(38), Some(0)));
        // The widest the server declares, and a negative scale (PostgreSQL 15).
        let ty = sql_type(&Type::NUMERIC, modifier(1000, 1000));
        assert_eq!((ty.precision, ty.scale), (Some(1000), Some(1000)));
        let ty = sql_type(&Type::NUMERIC, modifier(5, -2));
        assert_eq!((ty.precision, ty.scale), (Some(5), Some(-2)));
        // No modifier is an unbounded numeric, and 0..3 are not a modifier at all.
        for typmod in [-1, 0, 3] {
            let ty = sql_type(&Type::NUMERIC, typmod);
            assert_eq!(
                (ty.base, ty.precision, ty.scale),
                (Base::Decimal, None, None)
            );
        }
    }

    #[test]
    fn text_lengths_come_from_the_modifier_less_four() {
        let ty = sql_type(&Type::VARCHAR, 24);
        assert_eq!((ty.base, ty.length), (Base::Text, Some(20)));
        assert_eq!(sql_type(&Type::VARCHAR, -1).length, None);
        let ty = sql_type(&Type::BPCHAR, 9);
        assert_eq!((ty.base, ty.length), (Base::Char, Some(5)));
        assert_eq!(sql_type(&Type::BPCHAR, -1).length, None);
        let ty = sql_type(&Type::TEXT, -1);
        assert_eq!((ty.base, ty.length), (Base::Text, None));
        // `char(0)` does not exist, and a modifier of exactly four would be `n = 0`.
        assert_eq!(sql_type(&Type::VARCHAR, 4).length, Some(0));
    }

    #[test]
    fn times_carry_their_fractional_digits_and_their_zone() {
        for (oid_type, base, zone) in [
            (Type::TIME, Base::Time, TimeZone::None),
            (Type::TIMETZ, Base::TimeTz, TimeZone::Offset),
            (Type::TIMESTAMP, Base::Timestamp, TimeZone::None),
            (Type::TIMESTAMPTZ, Base::TimestampTz, TimeZone::Offset),
        ] {
            let ty = sql_type(&oid_type, 3);
            assert_eq!(
                (ty.base, ty.fraction_digits, ty.time_zone),
                (base, Some(3), zone)
            );
            let ty = sql_type(&oid_type, 0);
            assert_eq!(ty.fraction_digits, Some(0));
            assert_eq!(sql_type(&oid_type, -1).fraction_digits, None);
        }
        // An interval's fractional digits are the low bits; `0xFFFF` is "none declared".
        assert_eq!(sql_type(&Type::INTERVAL, -1).fraction_digits, None);
        assert_eq!(
            sql_type(&Type::INTERVAL, 0x7FFF_0000 | 0xFFFF).fraction_digits,
            None
        );
        assert_eq!(
            sql_type(&Type::INTERVAL, 0x0400_0003).fraction_digits,
            Some(3)
        );
    }

    #[test]
    fn the_plain_types_map_to_their_logical_type() {
        for (ty, base) in [
            (Type::BOOL, Base::Bool),
            (Type::BYTEA, Base::Binary),
            (Type::INT8, Base::Int64),
            (Type::INT2, Base::Int16),
            (Type::INT4, Base::Int32),
            (Type::FLOAT4, Base::Float32),
            (Type::FLOAT8, Base::Float64),
            (Type::UUID, Base::Uuid),
            (Type::DATE, Base::Date),
            (Type::JSON, Base::Json),
            (Type::JSONB, Base::Jsonb),
        ] {
            let described = sql_type(&ty, -1);
            assert_eq!(described.base, base, "{ty}");
            assert_eq!(described.oid, Some(ty.oid()));
            assert_eq!(described.native, ty.name());
        }
    }

    #[test]
    fn built_ins_are_picked_by_oid_and_money_oid_and_reg_types_are_not() {
        for kind in PgBuiltin::ALL {
            let ty = Type::from_oid(kind.oid()).expect("a built-in oid");
            assert_eq!(sql_type(&ty, -1).base, Base::Builtin(kind), "{ty}");
        }
        for ty in [
            Type::MONEY,
            Type::OID,
            Type::REGCLASS,
            Type::REGPROC,
            Type::NAME,
            Type::CHAR,
            Type::INT4_RANGE,
            Type::RECORD,
        ] {
            assert_eq!(sql_type(&ty, -1).base, Base::Other, "{ty}");
        }
        // A bit string keeps its length.
        assert_eq!(sql_type(&Type::BIT, 8).length, Some(8));
        assert_eq!(sql_type(&Type::VARBIT, -1).length, None);
        assert_eq!(sql_type(&Type::INET, -1).length, None);
    }

    #[test]
    fn an_array_takes_its_element_and_the_same_modifier() {
        let ty = sql_type(&Type::NUMERIC_ARRAY, modifier(10, 2));
        assert_eq!(ty.base, Base::Array);
        assert_eq!(ty.oid, Some(Type::NUMERIC_ARRAY.oid()));
        let element = ty.element.expect("an element");
        assert_eq!(
            (element.base, element.precision, element.scale, element.oid),
            (Base::Decimal, Some(10), Some(2), Some(Type::NUMERIC.oid()))
        );
        let ty = sql_type(&Type::INT4_ARRAY, -1);
        assert_eq!(ty.element.expect("an element").base, Base::Int32);
        let ty = sql_type(&Type::VARCHAR_ARRAY, 14);
        assert_eq!(ty.element.expect("an element").length, Some(10));
    }

    #[test]
    fn an_enum_is_an_enum_and_a_domain_or_extension_type_is_other() {
        let mood = Type::new(
            "mood".to_owned(),
            99_001,
            Kind::Enum(vec!["sad".to_owned(), "ok".to_owned()]),
            "public".to_owned(),
        );
        let ty = sql_type(&mood, -1);
        assert_eq!(
            (ty.base, ty.oid, ty.native.as_str()),
            (Base::Enum, Some(99_001), "mood")
        );

        // A domain named like a built-in is not that built-in.
        let domain = Type::new(
            "inet".to_owned(),
            99_002,
            Kind::Domain(Type::TEXT),
            "public".to_owned(),
        );
        assert_eq!(sql_type(&domain, -1).base, Base::Other);
        let extension = Type::new(
            "hstore".to_owned(),
            99_003,
            Kind::Simple,
            "public".to_owned(),
        );
        assert_eq!(sql_type(&extension, -1).base, Base::Other);
        // An array of an enum is an array of enum; an array of a domain keeps the domain `Other`.
        let moods = Type::new(
            "_mood".to_owned(),
            99_004,
            Kind::Array(mood),
            "public".to_owned(),
        );
        let ty = sql_type(&moods, -1);
        assert_eq!(ty.element.expect("an element").base, Base::Enum);
    }
}
