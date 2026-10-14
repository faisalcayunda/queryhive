//! The type and value rules of Save to Table across connections, as data (W13-T20 §4, B-3, B-4).
//!
//! `tests/fixtures/transfer_types.json` is the table: every source type against both
//! dialects with exactly one outcome (`exact`, `widened`, `text_fallback` or `refuse`) and
//! the DDL that goes with it, then the values each pair accepts, writes as a given text, or
//! refuses by rule. A change to a mapping or a rule is a change to that file, in the same
//! commit, where a reviewer reads it as a table instead of as code.
//!
//! `tests/fixtures/ident_quoting.json` is the identifier rule, generated from the app's
//! reserved-word list (`SQLIdentifierQuoting.swift`); the test also reads the Swift source
//! itself, so the Rust list, the fixture and the app cannot drift apart unnoticed.
//!
//! No server is involved: these are pure functions.

use qh_core::{IntervalValue, Value};
use qh_driver::DriverKind;
use qh_ffi::transfer::encode::{copy_row, mysql_cell, MysqlCell};
use qh_ffi::transfer::names::{self, RESERVED_WORDS};
use qh_ffi::transfer::types::{
    map_type, plan_table, Base, Outcome, PgBuiltin, SqlType, Target, TimeZone,
};
use serde_json::Value as Json;

fn fixture(name: &str) -> Json {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn text<'a>(json: &'a Json, key: &str) -> &'a str {
    json[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {json}"))
}

fn source_of(name: &str) -> DriverKind {
    match name {
        "postgres" => DriverKind::Postgres,
        "mysql" => DriverKind::Mysql,
        "trino" => DriverKind::Trino,
        other => panic!("unknown source {other}"),
    }
}

fn target_of(name: &str) -> Target {
    match name {
        "postgres" => Target::Postgres,
        "mysql" => Target::Mysql,
        other => panic!("unknown target {other}"),
    }
}

/// Every base has a name in the fixture; the match has no wildcard, so a new base fails to
/// compile here until it is given one (and then the coverage test asks for a row).
fn base_name(base: Base) -> String {
    match base {
        Base::Bool => "bool".to_owned(),
        Base::Int8 => "int8".to_owned(),
        Base::Int16 => "int16".to_owned(),
        Base::Int24 => "int24".to_owned(),
        Base::Int32 => "int32".to_owned(),
        Base::Int64 => "int64".to_owned(),
        Base::Float32 => "float32".to_owned(),
        Base::Float64 => "float64".to_owned(),
        Base::Decimal => "decimal".to_owned(),
        Base::Text => "text".to_owned(),
        Base::Char => "char".to_owned(),
        Base::Binary => "binary".to_owned(),
        Base::Uuid => "uuid".to_owned(),
        Base::Date => "date".to_owned(),
        Base::Time => "time".to_owned(),
        Base::TimeTz => "timetz".to_owned(),
        Base::Timestamp => "timestamp".to_owned(),
        Base::TimestampTz => "timestamptz".to_owned(),
        Base::Interval => "interval".to_owned(),
        Base::Json => "json".to_owned(),
        Base::Jsonb => "jsonb".to_owned(),
        Base::Array => "array".to_owned(),
        Base::Map => "map".to_owned(),
        Base::Row => "row".to_owned(),
        Base::Enum => "enum".to_owned(),
        Base::Set => "set".to_owned(),
        Base::Builtin(kind) => format!("builtin:{}", kind.ddl_name()),
        Base::Other => "other".to_owned(),
    }
}

fn base_of(name: &str) -> Base {
    Base::all()
        .into_iter()
        .find(|base| base_name(*base) == name)
        .unwrap_or_else(|| panic!("unknown base {name}"))
}

fn sql_type(json: &Json) -> SqlType {
    let mut ty = SqlType::of(base_of(text(json, "base")));
    if let Some(precision) = json["precision"].as_u64() {
        ty.precision = Some(precision as u32);
    }
    if let Some(scale) = json["scale"].as_i64() {
        ty.scale = Some(scale as i32);
    }
    if let Some(length) = json["length"].as_u64() {
        ty.length = Some(length as u32);
    }
    ty.unsigned = json["unsigned"].as_bool().unwrap_or(false);
    ty.time_zone = match json["time_zone"].as_str() {
        None | Some("none") => TimeZone::None,
        Some("offset") => TimeZone::Offset,
        Some("session") => TimeZone::Session,
        Some(other) => panic!("unknown time_zone {other}"),
    };
    if let Some(digits) = json["fraction_digits"].as_u64() {
        ty.fraction_digits = Some(digits as u32);
    }
    if let Some(decimals) = json["decimals"].as_u64() {
        ty.decimals = Some(decimals as u32);
    }
    if json["element"].is_object() {
        ty.element = Some(Box::new(sql_type(&json["element"])));
    }
    if let Some(oid) = json["oid"].as_u64() {
        ty.oid = Some(oid as u32);
    }
    if let Some(native) = json["native"].as_str() {
        ty.native = native.to_owned();
    }
    ty
}

fn big(json: &Json) -> i128 {
    json.as_str()
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("not a number in a string: {json}"))
}

fn value(json: &Json) -> Value {
    if json.is_null() {
        return Value::Null;
    }
    let object = json
        .as_object()
        .unwrap_or_else(|| panic!("a tagged value: {json}"));
    let (tag, body) = object.iter().next().expect("one tag");
    match tag.as_str() {
        "bool" => Value::Bool(body.as_bool().unwrap()),
        "int" => Value::Int(big(body) as i64),
        "uint" => Value::UInt(big(body) as u64),
        "float" => Value::Float(match body.as_str().unwrap() {
            "NaN" => f64::NAN,
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            other => other.parse().unwrap(),
        }),
        "decimal" => Value::Decimal {
            unscaled: big(&body[0]),
            scale: body[1].as_u64().unwrap() as u8,
        },
        "text" => Value::Text(body.as_str().unwrap().into()),
        "bytes" => {
            let hex = body.as_str().unwrap();
            Value::Bytes(
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect(),
            )
        }
        "date" => Value::Date {
            days: body.as_i64().unwrap() as i32,
        },
        "time" => Value::Time {
            micros: body.as_i64().unwrap(),
        },
        "timestamp" => Value::Timestamp {
            micros: big(&body[0]) as i64,
            offset_secs: body[1].as_i64().map(|offset| offset as i32),
        },
        "interval" => Value::Interval(IntervalValue {
            months: body[0].as_i64().unwrap() as i32,
            days: body[1].as_i64().unwrap() as i32,
            micros: big(&body[2]) as i64,
        }),
        "json" => Value::Json(body.as_str().unwrap().into()),
        "array" => Value::Array(body.as_array().unwrap().iter().map(value).collect()),
        "row" => Value::Row(body.as_array().unwrap().iter().map(value).collect()),
        "map" => Value::Map(
            body.as_array()
                .unwrap()
                .iter()
                .map(|pair| (value(&pair[0]), value(&pair[1])))
                .collect(),
        ),
        "unknown" => Value::unknown(body[0].as_str().unwrap(), body[1].as_str().unwrap()),
        other => panic!("unknown value tag {other}"),
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Exact => "exact",
        Outcome::Widened => "widened",
        Outcome::TextFallback => "text_fallback",
        Outcome::Refuse => "refuse",
    }
}

fn cell_json(cell: &MysqlCell) -> Json {
    match cell {
        MysqlCell::Null => serde_json::json!({"null": true}),
        MysqlCell::Int(n) => serde_json::json!({"int": n.to_string()}),
        MysqlCell::UInt(n) => serde_json::json!({"uint": n.to_string()}),
        MysqlCell::Float(n) => serde_json::json!({"float": n.to_string()}),
        MysqlCell::Text(s) => serde_json::json!({"text": s}),
        MysqlCell::Bytes(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            serde_json::json!({"bytes": hex})
        }
    }
}

#[test]
fn every_source_type_has_exactly_one_outcome_per_dialect() {
    let table = fixture("transfer_types.json");
    let rows = table["types"].as_array().expect("types");
    assert!(rows.len() >= 100, "{} rows", rows.len());
    for row in rows {
        let id = text(row, "id");
        let source = source_of(text(row, "source"));
        let ty = sql_type(&row["type"]);
        for target in Target::ALL {
            let key = if target == Target::Postgres {
                "postgres"
            } else {
                "mysql"
            };
            let expected = &row[key];
            let mapping = map_type(&ty, source, target);
            assert_eq!(
                outcome_name(mapping.outcome()),
                text(expected, "outcome"),
                "{id} -> {key}: {mapping:?}"
            );
            match expected["ddl"].as_str() {
                Some(ddl) => assert_eq!(mapping.ddl(), Some(ddl), "{id} -> {key}"),
                None => assert_eq!(mapping.ddl(), None, "{id} -> {key}"),
            }
            // A refusal states why; a change states what changed; an exact one says nothing.
            match mapping.outcome() {
                Outcome::Exact => assert_eq!(mapping.note(), None, "{id} -> {key}"),
                _ => assert!(
                    mapping.note().is_some_and(|note| !note.is_empty()),
                    "{id} -> {key}"
                ),
            }
        }
    }
}

#[test]
fn the_table_covers_every_base_and_every_outcome_in_both_dialects() {
    let table = fixture("transfer_types.json");
    let rows = table["types"].as_array().expect("types");
    for base in Base::all() {
        let name = base_name(base);
        assert!(
            rows.iter().any(|row| text(&row["type"], "base") == name),
            "no row for base {name}"
        );
    }
    for key in ["postgres", "mysql"] {
        for outcome in ["exact", "widened", "text_fallback", "refuse"] {
            assert!(
                rows.iter().any(|row| text(&row[key], "outcome") == outcome),
                "no {outcome} row for {key}"
            );
        }
    }
    // Every source driver appears.
    for source in ["postgres", "mysql", "trino"] {
        assert!(
            rows.iter().any(|row| text(row, "source") == source),
            "{source}"
        );
    }
}

#[test]
fn the_cases_the_review_asked_for_are_in_the_table() {
    // B-4 and C-1..C-3, by id. A case dropped from the fixture fails here by name.
    let table = fixture("transfer_types.json");
    let mut ids: Vec<&str> = Vec::new();
    for key in ["types", "values"] {
        ids.extend(
            table[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| text(row, "id")),
        );
    }
    for needed in [
        "mysql-zero-date",
        "mysql-invalid-datetime",
        "pg-numeric-nan",
        "pg-numeric-infinity-to-mysql",
        "pg-date-infinity",
        "pg-timestamp-bc",
        "pg-timestamptz-bc",
        "pg-datestyle-dmy-bc",
        "trino-time-9",
        "trino-timestamp-9",
        "trino-timestamptz-9",
        "trino-time-tz",
        "text-nul-to-pg",
        "text-nul-to-mysql",
        "json-to-pg",
        "jsonb-to-pg",
        "json-duplicate-keys",
        "jsonb-nul",
        "mysql-datetime-range",
        "mysql-date-range",
        "pg-money",
        "pg-oid",
        "pg-regclass",
        "numeric-typmod",
        "numeric-unbounded-to-mysql",
        "timestamptz-to-pg",
        "timestamptz-to-mysql",
        "interval",
        "array-trino",
        "array-pg",
        "array-pg-bounds",
        "enum",
        "uuid",
        "bytea",
        "unsigned-bigint",
        "tinyint1",
        "bit-to-anything",
        "mysql-time-range",
        "map-duplicate-key",
        "malicious-native",
    ] {
        assert!(
            ids.iter().any(|id| id.starts_with(needed)),
            "the fixture has no `{needed}` case"
        );
    }
}

#[test]
fn every_value_case_writes_the_text_or_the_refusal_the_table_names() {
    let table = fixture("transfer_types.json");
    let groups = table["values"].as_array().expect("values");
    let mut checked = 0;
    for group in groups {
        let id = text(group, "id");
        let source = source_of(text(group, "source"));
        let target = target_of(text(group, "target"));
        let ty = sql_type(&group["type"]);
        let plan = plan_table(source, target, &[("c", &ty)])
            .unwrap_or_else(|refusal| panic!("{id}: the column should map: {refusal}"));
        for case in group["cases"].as_array().expect("cases") {
            let input = value(&case["value"]);
            let label = format!("{id} / {}", case["value"]);
            let outcome: Result<Json, &'static str> = match target {
                Target::Postgres => {
                    let mut out = Vec::new();
                    copy_row(&plan.columns, [&input], &mut out)
                        .map(|()| {
                            let row = String::from_utf8(out).unwrap();
                            let field = row.strip_suffix('\n').expect("a row ends in a newline");
                            serde_json::json!({ "copy": field })
                        })
                        .map_err(|violation| violation.rule.kind())
                }
                Target::Mysql => mysql_cell(&plan.columns[0], &input)
                    .map(|cell| cell_json(&cell))
                    .map_err(|rule| rule.kind()),
            };
            match (case.get("ok"), case.get("refuse")) {
                (Some(expected), None) => {
                    assert_eq!(outcome.as_ref(), Ok(expected), "{label}");
                }
                (None, Some(kind)) => {
                    assert_eq!(outcome, Err(kind.as_str().unwrap()), "{label}");
                }
                _ => panic!("{label}: a case has `ok` or `refuse`, not both and not neither"),
            }
            checked += 1;
        }
    }
    assert!(checked >= 150, "only {checked} value cases");
}

#[test]
fn a_hostile_type_text_from_the_server_is_in_the_message_and_never_in_the_statement() {
    let hostile = "int); DROP TABLE x; --";
    let mut ty = SqlType::of(Base::Int32).with_native(hostile);
    ty.oid = Some(23);
    let plan = plan_table(DriverKind::Postgres, Target::Postgres, &[("a", &ty)]).unwrap();
    let sql = plan.create_table_sql("t");
    assert_eq!(sql, "CREATE TABLE t (a integer)");
    let other = SqlType::of(Base::Other).with_native(hostile);
    let refusal = plan_table(DriverKind::Postgres, Target::Postgres, &[("a", &other)]).unwrap_err();
    assert!(refusal.to_string().contains("DROP TABLE"), "{refusal}");
}

#[test]
fn an_oid_outside_the_allowlist_is_refused() {
    for oid in [790, 26, 2205, 24, 2206, 2202] {
        assert_eq!(PgBuiltin::from_oid(oid), None, "{oid}");
        let ty = SqlType::of(Base::Other).with_oid(oid);
        for target in Target::ALL {
            assert_eq!(
                map_type(&ty, DriverKind::Postgres, target).outcome(),
                Outcome::Refuse
            );
        }
    }
}

// ---------------------------------------------------------------------------------- //
// identifiers
// ---------------------------------------------------------------------------------- //

/// The words of the app's list, read from its source.
fn swift_words() -> Vec<String> {
    let source = include_str!("../../../app/Sources/QueryHive/Models/SQLIdentifierQuoting.swift");
    let body = source
        .split("static let reserved: Set<String> = [")
        .nth(1)
        .expect("the reserved list")
        .split(']')
        .next()
        .unwrap();
    body.split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, word)| word.to_owned())
        .collect()
}

#[test]
fn the_reserved_words_are_the_apps_own_list_to_the_letter() {
    let swift = swift_words();
    assert_eq!(swift.len(), 316);
    let mut sorted = swift.clone();
    sorted.sort();
    assert_eq!(
        sorted, RESERVED_WORDS,
        "the Rust list is the Swift list, sorted"
    );

    let quoting = fixture("ident_quoting.json");
    let listed: Vec<String> = quoting["reserved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(listed, sorted, "the fixture was generated from this list");
}

#[test]
fn identifiers_are_written_as_the_fixture_generated_from_the_swift_rule_says() {
    let quoting = fixture("ident_quoting.json");
    let cases = quoting["cases"].as_array().unwrap();
    assert!(cases.len() > 316 + 20, "{} cases", cases.len());
    for case in cases {
        let name = text(case, "name");
        assert_eq!(
            names::ident(Target::Postgres, name),
            text(case, "postgres"),
            "postgres {name:?}"
        );
        assert_eq!(
            names::ident(Target::Mysql, name),
            text(case, "mysql"),
            "mysql {name:?}"
        );
    }
    // Every reserved word is in the cases, quoted, in both dialects.
    for word in RESERVED_WORDS {
        assert!(
            cases.iter().any(|case| text(case, "name") == word
                && text(case, "postgres") == format!("\"{word}\"")
                && text(case, "mysql") == format!("`{word}`")),
            "{word}"
        );
    }
}
