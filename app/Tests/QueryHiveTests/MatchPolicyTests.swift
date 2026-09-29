import XCTest

@testable import QueryHive

/// The match policy: which columns may identify a fetched row, and what happens
/// when one cannot.
final class MatchPolicyTests: XCTestCase {
    private let id = Event.Column(name: "id", type: "bigint")
    private let geom = Event.Column(name: "geom", type: "geometry")

    func testABinaryColumnIsExcludedAndNamed() {
        let columns = [id, Event.Column(name: "blob", type: "bytea")]

        let match = UpdateStatements.match(for: ["1", "\\x00"], columns: columns, kind: .postgres)

        // The comparable column still identifies the row; the binary one is left
        // out and reported rather than compared to a text the server cannot use.
        XCTAssertEqual(match.sql, "\"id\" = 1")
        XCTAssertEqual(match.excluded.count, 1)
        XCTAssertTrue(match.excluded[0].contains("\"blob\""), match.excluded[0])
        XCTAssertTrue(match.note?.contains("blob") == true, match.note ?? "")
    }

    func testASpatialColumnIsExcluded() {
        let match = UpdateStatements.match(for: ["1", "POINT(0 0)"], columns: [id, geom],
                                          kind: .postgres)
        XCTAssertEqual(match.sql, "\"id\" = 1")
        XCTAssertEqual(match.excluded.count, 1)
    }

    func testARowWhoseOnlyColumnIsExcludedHasNoPredicate() {
        // No comparable column means no predicate. That must be a refusal, not a
        // dropped `WHERE`: a bare `UPDATE t SET …` matches every row.
        let match = UpdateStatements.match(for: ["POINT(0 0)"], columns: [geom], kind: .postgres)
        XCTAssertTrue(match.isEmpty)
        XCTAssertEqual(match.excluded.count, 1)
    }

    func testAnExcludedColumnIsExcludedEvenWhenNull() {
        let match = UpdateStatements.match(for: [nil], columns: [geom], kind: .postgres)
        XCTAssertTrue(match.isEmpty, "an excluded column must not become `geom IS NULL`")
    }

    func testAFloatIsComparedThroughTheServerRendering() {
        let score = Event.Column(name: "score", type: "double")
        // The grid read the server's rendering, so the rendering is what is compared.
        XCTAssertEqual(UpdateStatements.match(for: ["1.5"], columns: [score], kind: .mysql).sql,
                       "CONCAT(`score`) = '1.5'")
        XCTAssertEqual(UpdateStatements.match(for: ["1.5"], columns: [score], kind: .postgres).sql,
                       "\"score\"::text = '1.5'")
        XCTAssertEqual(UpdateStatements.match(for: ["1.5"], columns: [score], kind: .trino).sql,
                       "CAST(\"score\" AS varchar) = '1.5'")
    }

    func testJsonIsComparedThroughTheServerRendering() {
        let doc = Event.Column(name: "doc", type: "json")
        XCTAssertEqual(UpdateStatements.match(for: ["{\"a\":1}"], columns: [doc], kind: .mysql).sql,
                       "CONCAT(`doc`) = '{\"a\":1}'")
    }

    func testTheTypePolicyReadsThroughAWidth() {
        XCTAssertEqual(MatchPolicy.forColumn(type: "double precision", kind: .postgres), .serverText)
        XCTAssertEqual(MatchPolicy.forColumn(type: "decimal(38,10)", kind: .postgres), .match)
        XCTAssertEqual(MatchPolicy.forColumn(type: "character varying(16)", kind: .postgres), .match)
    }

    func testTrinoNestedTypesAreExcluded() {
        for type in ["array(bigint)", "map(varchar, bigint)", "row(a bigint)"] {
            let column = Event.Column(name: "n", type: type)
            let match = UpdateStatements.match(for: ["[1]"], columns: [column], kind: .trino)
            XCTAssertTrue(match.isEmpty, type)
            XCTAssertEqual(match.excluded.count, 1, type)
        }
    }

    func testAnOrdinaryRowStillMatchesEveryColumn() {
        let name = Event.Column(name: "name", type: "varchar")
        let age = Event.Column(name: "age", type: "bigint")
        let match = UpdateStatements.match(for: ["Nia", "9"], columns: [name, age], kind: .postgres)
        XCTAssertEqual(match.sql, "\"name\" = 'Nia' AND \"age\" = 9")
        XCTAssertTrue(match.excluded.isEmpty)
        XCTAssertNil(match.note, "no exclusions, no note on the reviewed SQL")
    }
}
