import XCTest

@testable import QueryHive

/// Open Quickly's ranking, which is the part worth testing: the palette itself is a list.
///
/// The search is a pure function over three in-memory lists, so it can be checked without a model,
/// a window or a server. What matters is that a prefix match beats a match in the middle, that a
/// match in the title beats one in the subtitle, and that all three sources reach the list.
final class QuickSearchTests: XCTestCase {
    private func node(_ name: String) -> TreeNode {
        TreeNode.group(ConnectionGroup(name: name))
    }

    private func saved(_ name: String, sql: String = "SELECT 1") -> Event.SavedQuery {
        Event.SavedQuery(id: UUID().uuidString, name: name, sql: sql, connectionId: nil,
                         folderId: nil, favourite: false, deleted: false, version: 1)
    }

    private func history(_ sql: String, outcome: String = "ok") -> Event.HistoryEntry {
        Event.HistoryEntry(id: UUID().uuidString, sql: sql, startedAt: 0, elapsedMs: 1,
                           rowCount: 1, outcome: outcome, error: nil, connectionId: nil,
                           deleted: false, version: 1)
    }

    /// The three sources, told apart by what picking them does rather than by their names.
    private func kinds(_ results: [QuickResult]) -> [String] {
        results.map { result in
            switch result.action {
            case .revealNode: "node"
            case .loadSQL: "sql"
            }
        }
    }

    func testAllThreeSourcesReachTheList() {
        let results = QuickSearch.results(
            query: "",
            nodes: [node("Analytics")],
            savedQueries: [saved("Monthly")],
            history: [history("SELECT * FROM people")])

        XCTAssertEqual(results.count, 3)
        XCTAssertEqual(kinds(results).filter { $0 == "node" }.count, 1)
        XCTAssertEqual(kinds(results).filter { $0 == "sql" }.count, 2)
    }

    func testAPrefixMatchBeatsOneInTheMiddleAndBothBeatTheSubtitle() {
        let results = QuickSearch.results(
            query: "people",
            nodes: [],
            savedQueries: [
                saved("people report"),
                saved("all people"),
            ],
            history: [history("SELECT * FROM people")])

        XCTAssertEqual(results.map(\.title),
                       ["people report", "all people", "SELECT * FROM people"],
                       "the two rank-1 rows are ordered by how short the title is")
    }

    func testAMatchInTheSubtitleStillFindsTheRow() {
        // A saved query whose name says nothing about the statement it holds, so the only place
        // "from people" appears is the statement itself.
        let results = QuickSearch.results(
            query: "from people",
            nodes: [],
            savedQueries: [saved("Nightly", sql: "SELECT * FROM people")],
            history: [])

        XCTAssertEqual(results.count, 1)
        XCTAssertEqual(results[0].title, "Nightly")
    }

    func testTheSearchIsCaseInsensitive() {
        let results = QuickSearch.results(
            query: "ANALYTICS", nodes: [node("analytics")], savedQueries: [], history: [])
        XCTAssertEqual(results.count, 1)
    }

    func testTheLimitIsHonoured() {
        let many = (1...60).map { saved("query \($0)") }
        let results = QuickSearch.results(query: "", nodes: [], savedQueries: many, history: [],
                                          limit: 10)
        XCTAssertEqual(results.count, 10)
    }

    func testTheFirstLineSkipsBlankLinesAndNeverComesBackEmpty() {
        XCTAssertEqual(QuickSearch.firstLine("SELECT 1\nFROM people"), "SELECT 1")
        XCTAssertEqual(QuickSearch.firstLine("   \n SELECT 2"), "SELECT 2")
        XCTAssertEqual(QuickSearch.firstLine(""), "statement")
        XCTAssertEqual(QuickSearch.firstLine("   "), "statement")
    }
}
