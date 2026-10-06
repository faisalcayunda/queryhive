// Writes tests/fixtures/format.json: what the Swift grid's `ColumnFormat.render` makes of a corpus.
// The Rust renderer replays the same corpus in tests/format.rs (backlog B-21), so the two sides
// are checked against one file.
//
// Run from the repository root:
//
//   swiftc -swift-version 5 -parse-as-library -o /tmp/export_format \
//     app/Sources/QueryHive/Models/ColumnFormat.swift \
//     app/Sources/QueryHive/Models/GridValue.swift \
//     app/Sources/QueryHive/Support/HexDump.swift \
//     crates/qh-result-store/tests/fixtures/export_format.swift
//   /tmp/export_format > crates/qh-result-store/tests/fixtures/format.json
//
// Foundation's JSON and calendar output can move between macOS releases, so a changed answer after
// an OS update is a finding to read, not noise to re-record.

import Foundation

@main
struct ExportFormat {
    struct Case {
        var format: ColumnFormat
        var type: String
        var value: String
    }

    static func cases() -> [Case] {
        var out: [Case] = []
        func add(_ format: ColumnFormat, _ type: String, _ values: [String]) {
            out += values.map { Case(format: format, type: type, value: $0) }
        }

        add(.raw, "varchar", ["plain", " {\"a\":1} ", "", "550e8400e29b41d4a716446655440000"])

        // Text: only a binary column decodes. The values are the hex the engine sends for bytes.
        add(.text, "varchar", ["6869", "plain", "", "\\x6869"])
        add(.text, "bigint", ["12"])
        for type in ["bytea", "blob", "varbinary", "binary(16)", "longblob", "BYTEA"] {
            add(.text, type, ["6869"])
        }
        add(.text, "bytea", [
            "", "00", "ff", "c3", "ff00ff", "c328", "e697a5e69cac", "f09f9880", "f09f98", "eda080",
            "e08080", "f4908080", "c0af", "efbbbf6869", "0a", "000102", "e28028", "6162630a646566",
            "c3a9", "65cc81", "ffffffff", "f09f98e29c", "e282", "41e28242",
        ])

        // UUID: 32 hex digits in any dash layout, Swift's wider hex-digit set, and the near misses.
        let canonical = "550e8400e29b41d4a716446655440000"
        add(.uuid, "uuid", [
            canonical,
            "550E8400-E29B-41D4-A716-446655440000",
            "550e8400-e29b-41d4-a716-446655440000",
            "{550e8400-e29b-41d4-a716-446655440000}",
            "550e8400e29b41d4a71644665544000",
            "550e8400e29b41d4a7164466554400000",
            "550e84-00e29b41d4a7-16446655440000",
            "----",
            "",
            " " + canonical,
            canonical + "\n",
            "\u{FF46}50e8400e29b41d4a716446655440000",
            "\u{FF26}50e8400e29b41d4a716446655440000",
            "\u{FF15}50e8400e29b41d4a716446655440000",
            "550e8400e29b41d4a71644665544000g",
            canonical + "\u{0301}",
            "a\u{0301}50e8400e29b41d4a71644665544000",
            "\u{0660}50e8400e29b41d4a716446655440000",
            "0123456789ABCDEF0123456789abcdef",
            "550e8400-e29b-41d4-a716-4466554400",
            "-" + canonical + "-",
            "550e8400e29b41d4a716446655440000\u{200D}",
            "550e8400e29b41d4a716446655440000\u{FF5E}",
            "\u{FF21}\u{FF22}\u{FF23}\u{FF24}\u{FF25}\u{FF26}\u{FF41}\u{FF42}\u{FF43}\u{FF44}\u{FF45}\u{FF46}"
                + "\u{FF10}\u{FF11}\u{FF12}\u{FF13}\u{FF14}\u{FF15}\u{FF16}\u{FF17}\u{FF18}\u{FF19}"
                + "0123456789",
        ])
        add(.uuid, "varchar", ["not a uuid", canonical])

        // Unix timestamp: seconds, milliseconds, the edges of the unit rule, fractions, signs, text.
        add(.unixTimestamp, "bigint", [
            "0", "1", "-1", "1700000000", "1700000000123", "1.5", "-1.5", "-0.5", "-0.000001", "1e3", "1E3",
            " 1700000000 ", "\t1700000000\n", "abc", "", "NaN", "nan", "inf", "-inf", "infinity",
            "99999999999", "100000000000", "-99999999999", "-100000000000", "99999999999.9",
            "9007199254740993", "253402300799", "253402300800", "1e20", "-1e20", "1e15", "1e300",
            "0x10", "0x1p4", "1_000", "+5", ".5", "5.", "1e", "\u{0661}\u{0662}\u{0663}", "\u{FF11}\u{FF12}",
            "1,5", "86399", "86400", "-86400", "951782400", "4107542400", "2147483647", "2147483648",
            "-2147483648", "1700000000.999999", "1700000000.9999999999", "-1700000000.5",
            "12345678901234567890", "0.0000001", "-0",
            // Before the Gregorian cutover and before year 1: a calendar choice, not arithmetic.
            "-12219292800", "-12219292801", "-13000000000", "-14000000000", "-30610224000",
            "-62135596800", "-62135596801", "-62167219200", "-62167219201", "-70000000000",
            "-100000000000", "-1000000000000", "-100000000000000", "-377705116800", "-377705116801",
            // After year 9999.
            "253402300799.5", "300000000000", "1000000000000", "2000000000000", "9999999999999",
            "1000000000000000", "-9999999999999",
            // Where the formatter gives up (an empty string), a millisecond past it, and ties.
            "183882168921600000", "183882168921600320", "-184303902528000000", "-184303902528000320",
            "0.0005", "-0.0005", "-1.0005", "0.9995", "-0.9995", "1.9995", "-1.9995", "0.0004",
            "-0.0004",
        ])
        add(.unixTimestamp, "varchar", ["1700000000", "x"])

        // JSON: key order, spacing, escapes, number spelling, depth, invalid text.
        add(.json, "json", [
            #"{"b":2,"a":1}"#,
            #"{"a":{"c":[1,2,{"z":null,"y":true}],"b":"x"}}"#,
            "[]", "{}", "[[]]", "[{}]", #"{"a":[]}"#, #"{"a":{}}"#, "[1,2,3]", "[ 1 , 2 ]",
            #""str""#, "42", "true", "false", "null", "1.5", "-0", "0.1", "1e3", "1E+2", "1.0", "1.50",
            "100000000000000000000", "12345678901234567890", "9223372036854775807",
            "9223372036854775808", "-9223372036854775808", "-9223372036854775809",
            "3.14159265358979323846", "1.5e-7", "1e400", "1e-400", "0.1e1", "123456789.123456789",
            "0.30000000000000004", "1e21", "1e22", "1e-7", "5e-324", "1.7976931348623157e308",
            "4.9e-324", "100", "1e2", "2.5E3", "-1.5E-3", "0.5", "-7", "[1.0,2.50,3e0]",
            #"{"a":1,"a":2}"#, #"{"/":"/"}"#, #""\u00e9""#, #""é日本😀""#, #""\ud83d\ude00""#,
            #""\ud800""#, #""\u0000""#, #""\u001f""#, #""\u007f""#, #""\u2028""#,
            #""\n\r\t\b\f""#, #""\\""#, #""\"""#, #"{"k":"line\nbreak"}"#, #"["a\tb"]"#,
            #"{"é":1,"e":2,"z":3,"Z":4,"a":5,"aa":6,"B":7,"b":8}"#,
            "{\"\u{1F600}\":1,\"\u{FF5E}\":2}",
            #"{"a":1,"A":2,"_":3,"1":4,"10":5,"2":6,"":7," ":8}"#,
            #"{"b":{"d":1,"c":2},"a":[{"y":1,"x":2}]}"#,
            #" {"a":1} "#, "\n[1]\n", #"{"a":1} x"#, #"{'a':1}"#, "[1,]", "{not json", "NaN",
            "Infinity", "-Infinity", "[1,2", "01", "+1", ".5", "-", #""unterminated"#, "// c\n{}",
            "/* c */ {}", "\u{FEFF}{}", #"{"a":1,}"#, #"{"a" 1}"#, #"{a:1}"#, "[1 2]", #"["a" "b"]"#,
            "tru", "nul", "'x'", "", " ", "{", "}", "[", "]", ",", ":", #"{"a":}"#, "[,1]",
            "\"\u{0001}\"", "\"tab\there\"", "{\"a\":\"\u{00A0}\"}", "\"\\u00zz\"", #""\x""#,
            #"{"a":[1,2,{"b":[3,{"c":4}]}]}"#, #"[null,true,false,0,"",[],{}]"#,
            String(repeating: "[", count: 130) + String(repeating: "]", count: 130),
            String(repeating: "[", count: 600) + String(repeating: "]", count: 600),
        ])
        add(.json, "varchar", [#"{"a":1}"#, "plain text", "[1]"])

        return out
    }

    static func main() throws {
        let cases = cases().map { item -> [String: Any] in
            [
                "format": item.format.rawValue,
                "type": item.type,
                "value": item.value,
                "rendered": item.format.render(item.value, type: item.type),
            ]
        }
        let data = try JSONSerialization.data(
            withJSONObject: ["cases": cases],
            options: [.prettyPrinted, .sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
