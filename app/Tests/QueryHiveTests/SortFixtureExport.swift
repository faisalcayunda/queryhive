import Foundation
import XCTest

@testable import QueryHive

/// Runs the real Swift sort, column filter and cross-column search over one fixed corpus and checks
/// the answers against `crates/qh-result-store/tests/fixtures/differential.json`, the file the Rust
/// store's differential test (`tests/differential.rs`) replays.
///
/// A normal run is read-only: it recomputes the answers and fails when they differ from the
/// committed file, so a change to the Swift grid cannot drift away from the Rust port unnoticed.
/// `QH_EXPORT_FIXTURES=1 swift test --filter SortFixtureExport` rewrites the file instead.
///
/// The order of text depends on `localizedStandardCompare`, which follows the machine's locale. The
/// fixture is meant to be exported on an English or Indonesian locale; run it elsewhere and the
/// read-only check will say so.
final class SortFixtureExport: XCTestCase {
    // MARK: Corpus

    /// Column 0: names and free text in several scripts.
    private static let names: [String?] = [
        "Budi", "budi", "BUDI", "Siti Nurhaliza", "Agus Setiawan", "Dewi Lestari", "Wayan Sudarma",
        "Éric", "Eric", "Zoë", "Zoe", "José", "Jose", "Ångström", "Angstrom", "naïve", "naive",
        "Ünal", "Umar", "漢字", "日本語", "한국어", "中文 名", "😀 smile", "😀😀", "a😀b", "Äpfel", "Zebra",
        "apple", "Apple", "item 9", "item 10", "item 2", "KPM 9", "KPM 10", "",
        nil, " ", "  leading", "trailing  ",
    ]

    /// Column 1: numbers, numbers that are not plain, and text that holds digits.
    private static let codes: [String?] = [
        "10", "9", "2", "-3", "+5", "1.5", "1e3", "1E2", "0010", "007", "  7 ", "3.0", "3", "-0",
        "32.01.01.2001", "2024-01-05", "2024-01-15", "abc", "ABC", "KPM 10", "KPM 9", "x1", "x10",
        "", nil, nil, "٣", "１２", "1,5", ".5", "5.", "0x10", "NaN", "inf", "1e", "e5",
        "99999999999999999999", "0.1", "0.10", "-1.5e-3",
    ]

    /// Column 2: canonical equivalents, case folds, and spaces that are not the ASCII one.
    private static let notes: [String?] = [
        "\u{E9}",                // é composed
        "e\u{301}",              // é decomposed
        "É", "E\u{301}", "ß", "SS", "ss", "Straße", "STRASSE", "strasse", "İstanbul", "istanbul",
        "ǅ", "ǆ", "ﬁ", "fi", "Å", "A\u{30A}", "한", "\u{1112}\u{1161}\u{11AB}", "\u{3000}wide",
        "\u{A0}nbsp", "tab\there", "line\nbreak", "caf\u{E9}", "cafe\u{301}", "ÉCOLE", "école",
        "1 000", "1\u{A0}000", "😀", "👍🏽", "👍", "🇮🇩", "a", "B", "c", "", nil, "\u{0131}",
    ]

    private static let columnNames = ["name", "code", "note"]

    /// Prime-ish strides so the three columns pair up differently without a random generator.
    private static let rows: [[String?]] = (0..<names.count).map { index in
        [names[index], codes[(index * 7) % codes.count], notes[(index * 11) % notes.count]]
    }

    private static let textNeedles = [
        "", "   ", "an", "AN", "é", "e", "É", "ss", "ß", "straße", "😀", "漢", "日本", "1e3", "5",
        "KPM", "kpm 9", "  budi  ", "=budi", "= BUDI", "=é", "=e\u{301}", "=ß", "=", "= ",
        ">=10", ">10", ">5", "<10", "<=3", "<=abc", ">b", ">=B", "<a", ">= 2024-01-10", "<2024-01-10",
        ">1e2", ">=1E2", "<=-3", ">0x10", ">NaN", ">inf", "\u{3000}wide", "\u{A0}nbsp", "\nbreak",
        ">=\n3", "i", "İ", "ǆ", "fi", "ﬁ",
    ]

    private static let valueSets: [[String?]] = [
        [nil], ["Budi"], ["budi", "BUDI"], [nil, "Budi"], ["\u{E9}"], ["e\u{301}"],
        ["10", "9", "2"], ["", nil], ["3", "3.0"], [], ["no such value"],
    ]

    private static let searchTerms = [
        "", "   ", "budi", "BUDI", " budi ", "é", "e", "É", "ß", "ss", "strasse", "😀", "漢", "10",
        "kpm 9", "İ", "i", "ı", "\n", " ", "\t", "ǆ", "fi", "ﬁ", "é", "e\u{301}", "1e3", "x1",
        "no such term", "\u{A0}",
    ]

    // MARK: Export

    func testFixturesMatchTheCommittedFile() throws {
        let generated = try Self.render(Self.fixture())
        let url = Self.fixtureURL
        if ProcessInfo.processInfo.environment["QH_EXPORT_FIXTURES"] == "1" {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try generated.write(to: url, options: .atomic)
            return
        }
        let committed = try Data(contentsOf: url)
        XCTAssertEqual(
            generated, committed,
            "Swift's answers differ from \(url.lastPathComponent). If the change is intended, "
                + "re-export with QH_EXPORT_FIXTURES=1 and check the Rust differential test.")
    }

    func testTheCorpusIsWideEnoughToMeanSomething() {
        XCTAssertEqual(Self.rows.count, Self.names.count)
        XCTAssertTrue(Self.rows.contains { $0[0] == nil })
        XCTAssertTrue(Self.rows.contains { $0[0] == "" })
        XCTAssertTrue(Self.rows.contains { $0[1] == nil })
    }

    // MARK: Building

    /// `crates/qh-result-store/tests/fixtures/differential.json`, found from this file's path.
    private static var fixtureURL: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // QueryHiveTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // app
            .deletingLastPathComponent()  // repository root
            .appendingPathComponent("crates/qh-result-store/tests/fixtures/differential.json")
    }

    private static func json(_ cell: String?) -> Any { cell ?? NSNull() }

    /// Source row indices, in the order the grid would draw `rows` after `GridSort`.
    ///
    /// The sort is stable and returns rows, not indices, so each row is sent through carrying its
    /// own index in a trailing column the sort never looks at.
    private static func sortOrder(column: Int, direction: GridSort.Direction,
                                  over rows: [[String?]], ids: [Int]) -> [Int] {
        let tagged = zip(rows, ids).map { $0.0 + [String($0.1)] }
        let sorted = SwiftGridReference.order(GridSort(column: column, direction: direction), tagged)
        return sorted.map { Int($0[rows[0].count]!)! }
    }

    private static func filterIndices(_ rows: [[String?]], _ matches: ([String?]) -> Bool) -> [Int] {
        rows.indices.filter { matches(rows[$0]) }
    }

    static func fixture() -> [String: Any] {
        let rows = Self.rows
        let all = Array(rows.indices)

        let sorts: [[String: Any]] = columnNames.indices.flatMap { column in
            [GridSort.Direction.ascending, .descending].map { direction in
                [
                    "column": column,
                    "descending": direction == .descending,
                    "order": sortOrder(column: column, direction: direction, over: rows, ids: all),
                ]
            }
        }

        var filters: [[String: Any]] = []
        for column in columnNames.indices {
            for needle in textNeedles {
                filters.append([
                    "kind": "text", "column": column, "needle": needle,
                    "rows": filterIndices(rows) { SwiftGridReference.matchesText($0[column], needle) },
                ])
            }
            for set in valueSets {
                let filter = ColumnFilter.values(Set(set.map { $0 ?? ColumnFilter.nullToken }))
                filters.append([
                    "kind": "values", "column": column, "values": set.map(json),
                    "rows": filterIndices(rows) { SwiftGridReference.matches(filter, $0[column]) },
                ])
            }
        }

        let searches: [[String: Any]] = searchTerms.map { term in
            ["term": term, "rows": filterIndices(rows) { SwiftGridReference.matches($0, term: term) }]
        }

        // The pipeline the Swift grid ran (`QueryTab.displayedRows`, now the Rust view): filters, then search, then the sort.
        let pipelines: [[String: Any]] = [
            (1, ">=5", "k", 0, false), (1, "<10", "", 2, true), (0, "an", "e", 1, false),
            (2, "=ß", " ", 0, true), (1, "", "10", 1, true), (0, "zzz", "", 0, false),
        ].map { filterColumn, needle, term, sortColumn, descending in
            let kept = all.filter {
                SwiftGridReference.matchesText(rows[$0][filterColumn], needle)
                    && SwiftGridReference.matches(rows[$0], term: term)
            }
            let order = sortOrder(column: sortColumn,
                                  direction: descending ? .descending : .ascending,
                                  over: kept.map { rows[$0] }, ids: kept)
            return [
                "filter": ["column": filterColumn, "needle": needle], "search": term,
                "sort": ["column": sortColumn, "descending": descending], "rows": order,
            ]
        }

        return [
            "columns": columnNames,
            "rows": rows.map { $0.map(json) },
            "sorts": sorts, "filters": filters, "searches": searches, "pipelines": pipelines,
        ]
    }

    private static func render(_ object: [String: Any]) throws -> Data {
        var data = try JSONSerialization.data(
            withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        data.append(0x0A)
        return data
    }
}
