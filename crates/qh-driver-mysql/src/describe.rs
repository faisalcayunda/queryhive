//! A result column's type, as the structured [`SqlType`] a transfer maps (W13-T20 step 4).
//!
//! The column definition packet a statement is described with carries more than the type
//! code: the display length, the number of decimals, the flags (`UNSIGNED`, `BINARY`,
//! `ENUM`, `SET`) and the character set. `normalize::type_name` keeps only the word for the
//! grid's chip; this keeps what a faithful copy needs.
//!
//! Three facts shape the reading, each taken from the protocol rather than from memory:
//!
//! - **The length is in bytes of the character set.** `varchar(20)` in `utf8mb4` has a column
//!   length of 80, so characters are the length over the set's widest character
//!   ([`max_bytes_per_char`]). A set this table does not know is read as one byte per
//!   character, which can only make a declared length larger, never refuse a value that fits.
//! - **A decimal's length counts its sign and its point.** `decimal(10,2)` signed has length
//!   12: the precision is the length less one for the point (when there are decimals) and
//!   one for the sign (when signed).
//! - **`TEXT` and `BLOB` are one protocol type**, told apart by the character set (63 is
//!   `binary`), and `ENUM` and `SET` are `STRING` with a flag (see `normalize::type_name`).
//!
//! `tinyint(1)` stays a one-byte integer: MySQL has no boolean, and a display width is not
//! a type. All of this is pure over a `Column` built by hand in the tests; that a server
//! sends these fields for a described statement is what stays for a live check.

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::Column;
use qh_driver::{Base, SqlType, TimeZone};

use crate::normalize;

/// The descriptor of one result column.
pub fn sql_type(column: &Column) -> SqlType {
    let flags = column.flags();
    let length = column.column_length();
    let decimals = u32::from(column.decimals());
    // 31 is "not fixed" (an expression); beyond 6 is not a fraction this transfer carries.
    let fraction = (decimals <= 6).then_some(decimals);
    let unsigned = flags.contains(ColumnFlags::UNSIGNED_FLAG);
    let integer = |base| {
        let mut ty = SqlType::of(base);
        ty.unsigned = unsigned;
        ty
    };
    let timed = |base: Base, zone| {
        let mut ty = SqlType::of(base).with_zone(zone);
        ty.fraction_digits = fraction;
        ty
    };
    let binary = normalize::is_binary(column);
    let chars = || length / max_bytes_per_char(column.character_set());

    let mut described = match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY => integer(Base::Int8),
        ColumnType::MYSQL_TYPE_SHORT => integer(Base::Int16),
        ColumnType::MYSQL_TYPE_INT24 => integer(Base::Int24),
        ColumnType::MYSQL_TYPE_LONG => integer(Base::Int32),
        ColumnType::MYSQL_TYPE_LONGLONG => integer(Base::Int64),
        // `year` is a two-byte value of 1901 to 2155 (or 0), and not flagged unsigned for it.
        ColumnType::MYSQL_TYPE_YEAR => SqlType::of(Base::Int16),
        ColumnType::MYSQL_TYPE_FLOAT => SqlType::of(Base::Float32),
        ColumnType::MYSQL_TYPE_DOUBLE => SqlType::of(Base::Float64),
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => {
            let point = u32::from(decimals > 0);
            let sign = u32::from(!unsigned);
            match length.checked_sub(point + sign) {
                Some(precision) if precision > 0 && decimals <= precision => {
                    SqlType::decimal(precision, decimals as i32)
                }
                // A length that does not add up is not a decimal this can describe.
                _ => SqlType::of(Base::Other),
            }
        }
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => SqlType::of(Base::Date),
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 => {
            timed(Base::Timestamp, TimeZone::None)
        }
        // Stored as UTC and printed in the session's zone: the wall clock is the session's.
        ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => {
            timed(Base::Timestamp, TimeZone::Session)
        }
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => {
            timed(Base::Time, TimeZone::None)
        }
        ColumnType::MYSQL_TYPE_JSON => SqlType::of(Base::Json),
        ColumnType::MYSQL_TYPE_ENUM => SqlType::of(Base::Enum),
        ColumnType::MYSQL_TYPE_SET => SqlType::of(Base::Set),
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB => {
            if binary {
                SqlType::of(Base::Binary)
            } else {
                SqlType::of(Base::Text)
            }
        }
        ColumnType::MYSQL_TYPE_VARCHAR | ColumnType::MYSQL_TYPE_VAR_STRING => {
            if binary {
                SqlType::of(Base::Binary).with_length(length)
            } else {
                SqlType::of(Base::Text).with_length(chars())
            }
        }
        ColumnType::MYSQL_TYPE_STRING => {
            if flags.contains(ColumnFlags::ENUM_FLAG) {
                SqlType::of(Base::Enum)
            } else if flags.contains(ColumnFlags::SET_FLAG) {
                SqlType::of(Base::Set)
            } else if binary {
                SqlType::of(Base::Binary).with_length(length)
            } else {
                SqlType::of(Base::Char).with_length(chars())
            }
        }
        // BIT (bytes, not a number), GEOMETRY, VECTOR, NULL and anything newer.
        _ => SqlType::of(Base::Other),
    };
    described.native = normalize::type_name(column);
    described
}

/// The widest a character of the set can be, in bytes, by collation id.
///
/// Only the sets whose width is not one byte are listed; the ids are those of
/// `information_schema.COLLATIONS`.
fn max_bytes_per_char(character_set: u16) -> u32 {
    match character_set {
        // utf8mb4: the `_general`, `_bin`, `_unicode`, `_0900` families.
        45 | 46 | 224..=247 | 255..=324 => 4,
        // utf8mb3 (`utf8`) and its `_bin`, and the `_unicode_ci` family.
        33 | 76 | 83 | 192..=215 => 3,
        // ucs2, gbk, big5, gb2312, euckr, sjis, cp932.
        35 | 90 | 128..=151 | 28 | 87 | 24 | 86 | 19 | 85 | 13 | 88 | 95 | 96 | 1 | 84 => 2,
        // ujis, eucjpms.
        12 | 91 | 97 | 98 => 3,
        // utf16, utf16le, utf32, gb18030.
        54 | 55 | 56 | 60 | 61 | 62 | 101..=124 | 248..=250 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTF8MB4: u16 = 255;
    const LATIN1: u16 = 8;
    const BINARY: u16 = 63;

    fn column(kind: ColumnType, length: u32, decimals: u8, set: u16, flags: ColumnFlags) -> Column {
        Column::new(kind)
            .with_name(b"c")
            .with_column_length(length)
            .with_decimals(decimals)
            .with_character_set(set)
            .with_flags(flags)
    }

    fn plain(kind: ColumnType) -> SqlType {
        sql_type(&column(kind, 11, 0, BINARY, ColumnFlags::empty()))
    }

    #[test]
    fn integers_keep_their_width_and_their_sign() {
        for (kind, base) in [
            (ColumnType::MYSQL_TYPE_TINY, Base::Int8),
            (ColumnType::MYSQL_TYPE_SHORT, Base::Int16),
            (ColumnType::MYSQL_TYPE_INT24, Base::Int24),
            (ColumnType::MYSQL_TYPE_LONG, Base::Int32),
            (ColumnType::MYSQL_TYPE_LONGLONG, Base::Int64),
        ] {
            let signed = plain(kind);
            assert_eq!((signed.base, signed.unsigned), (base, false), "{kind:?}");
            let unsigned = sql_type(&column(kind, 10, 0, BINARY, ColumnFlags::UNSIGNED_FLAG));
            assert_eq!((unsigned.base, unsigned.unsigned), (base, true), "{kind:?}");
        }
        // `tinyint(1)` is a one-byte integer, and `year` a two-byte one.
        let tiny = sql_type(&column(
            ColumnType::MYSQL_TYPE_TINY,
            1,
            0,
            BINARY,
            ColumnFlags::empty(),
        ));
        assert_eq!(tiny.base, Base::Int8);
        assert_eq!(tiny.native, "tinyint");
        let year = sql_type(&column(
            ColumnType::MYSQL_TYPE_YEAR,
            4,
            0,
            BINARY,
            ColumnFlags::UNSIGNED_FLAG | ColumnFlags::ZEROFILL_FLAG,
        ));
        assert_eq!((year.base, year.unsigned), (Base::Int16, false));
    }

    #[test]
    fn a_decimals_precision_is_its_length_less_the_point_and_the_sign() {
        let decimal = |length, decimals, flags| {
            sql_type(&column(
                ColumnType::MYSQL_TYPE_NEWDECIMAL,
                length,
                decimals,
                BINARY,
                flags,
            ))
        };
        // decimal(10,2): 10 digits + point + sign.
        let ty = decimal(12, 2, ColumnFlags::empty());
        assert_eq!(
            (ty.base, ty.precision, ty.scale),
            (Base::Decimal, Some(10), Some(2))
        );
        // decimal(10,0): no point.
        let ty = decimal(11, 0, ColumnFlags::empty());
        assert_eq!((ty.precision, ty.scale), (Some(10), Some(0)));
        // unsigned: no sign.
        let ty = decimal(11, 2, ColumnFlags::UNSIGNED_FLAG);
        assert_eq!((ty.precision, ty.scale), (Some(10), Some(2)));
        // decimal(65,30), the widest.
        let ty = decimal(67, 30, ColumnFlags::empty());
        assert_eq!((ty.precision, ty.scale), (Some(65), Some(30)));
        // A length that does not add up is not described as a decimal.
        assert_eq!(decimal(1, 5, ColumnFlags::empty()).base, Base::Other);
        assert_eq!(decimal(0, 0, ColumnFlags::empty()).base, Base::Other);
    }

    #[test]
    fn times_carry_their_decimals_and_a_mysql_timestamp_says_it_is_in_the_session_zone() {
        let at =
            |kind, decimals| sql_type(&column(kind, 26, decimals, BINARY, ColumnFlags::empty()));
        let ty = at(ColumnType::MYSQL_TYPE_DATETIME, 3);
        assert_eq!(
            (ty.base, ty.fraction_digits, ty.time_zone),
            (Base::Timestamp, Some(3), TimeZone::None)
        );
        let ty = at(ColumnType::MYSQL_TYPE_TIMESTAMP, 6);
        assert_eq!(
            (ty.base, ty.fraction_digits, ty.time_zone),
            (Base::Timestamp, Some(6), TimeZone::Session)
        );
        assert_eq!(at(ColumnType::MYSQL_TYPE_TIME, 0).fraction_digits, Some(0));
        assert_eq!(at(ColumnType::MYSQL_TYPE_TIME, 0).base, Base::Time);
        // 31 is "not fixed" on an expression: no digits declared.
        assert_eq!(
            at(ColumnType::MYSQL_TYPE_DATETIME, 31).fraction_digits,
            None
        );
        assert_eq!(at(ColumnType::MYSQL_TYPE_DATE, 0).base, Base::Date);
    }

    #[test]
    fn text_length_is_in_characters_and_the_binary_set_makes_bytes() {
        let varchar = |length, set| {
            sql_type(&column(
                ColumnType::MYSQL_TYPE_VAR_STRING,
                length,
                0,
                set,
                ColumnFlags::empty(),
            ))
        };
        // varchar(20): 80 bytes in utf8mb4, 60 in utf8mb3, 20 in latin1.
        assert_eq!(varchar(80, UTF8MB4).length, Some(20));
        assert_eq!(varchar(60, 33).length, Some(20));
        assert_eq!(varchar(20, LATIN1).length, Some(20));
        assert_eq!(varchar(80, UTF8MB4).base, Base::Text);
        // varbinary(16): the binary set, and the length is bytes.
        let ty = varchar(16, BINARY);
        assert_eq!((ty.base, ty.length), (Base::Binary, Some(16)));
        // An unknown set is read as one byte a character: larger, never a false refusal.
        assert_eq!(varchar(80, 9999).length, Some(80));

        let string = |length, set, flags| {
            sql_type(&column(
                ColumnType::MYSQL_TYPE_STRING,
                length,
                0,
                set,
                flags,
            ))
        };
        let ty = string(40, UTF8MB4, ColumnFlags::empty());
        assert_eq!((ty.base, ty.length), (Base::Char, Some(10)));
        let ty = string(16, BINARY, ColumnFlags::BINARY_FLAG);
        assert_eq!((ty.base, ty.length), (Base::Binary, Some(16)));
        // The measured case: `enum` and `set` are STRING with a flag.
        assert_eq!(string(12, UTF8MB4, ColumnFlags::ENUM_FLAG).base, Base::Enum);
        assert_eq!(string(12, UTF8MB4, ColumnFlags::SET_FLAG).base, Base::Set);
    }

    #[test]
    fn text_and_blob_are_one_protocol_type_told_apart_by_the_character_set() {
        let blob = |set| {
            sql_type(&column(
                ColumnType::MYSQL_TYPE_BLOB,
                65_535,
                0,
                set,
                ColumnFlags::empty(),
            ))
        };
        assert_eq!(blob(BINARY).base, Base::Binary);
        assert_eq!(blob(UTF8MB4).base, Base::Text);
        assert_eq!(blob(UTF8MB4).length, None);
        for kind in [
            ColumnType::MYSQL_TYPE_TINY_BLOB,
            ColumnType::MYSQL_TYPE_MEDIUM_BLOB,
            ColumnType::MYSQL_TYPE_LONG_BLOB,
        ] {
            let ty = sql_type(&column(kind, 1, 0, UTF8MB4, ColumnFlags::empty()));
            assert_eq!(ty.base, Base::Text, "{kind:?}");
        }
    }

    #[test]
    fn floats_json_and_what_has_no_counterpart() {
        assert_eq!(plain(ColumnType::MYSQL_TYPE_FLOAT).base, Base::Float32);
        assert_eq!(plain(ColumnType::MYSQL_TYPE_DOUBLE).base, Base::Float64);
        assert_eq!(plain(ColumnType::MYSQL_TYPE_JSON).base, Base::Json);
        for kind in [
            ColumnType::MYSQL_TYPE_BIT,
            ColumnType::MYSQL_TYPE_GEOMETRY,
            ColumnType::MYSQL_TYPE_VECTOR,
            ColumnType::MYSQL_TYPE_NULL,
        ] {
            assert_eq!(plain(kind).base, Base::Other, "{kind:?}");
        }
        // The server's own word is kept for messages.
        assert_eq!(plain(ColumnType::MYSQL_TYPE_GEOMETRY).native, "geometry");
    }
}
