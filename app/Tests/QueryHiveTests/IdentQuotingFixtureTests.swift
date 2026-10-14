import XCTest

@testable import QueryHive

/// The app and the engine write an identifier the same way (W13-T20, B-3e). The fixture was made
/// from this app's reserved-word list by a third implementation of the rule
/// (`crates/qh-ffi/tests/fixtures/generate_ident_quoting.py`), and `qh-ffi`'s transfer tests read the
/// same file: a name the transfer sheet shows is the name the engine sends.
final class IdentQuotingFixtureTests: XCTestCase {
    private struct Fixture: Decodable {
        struct Case: Decodable {
            let name: String
            let postgres: String
            let mysql: String
        }
        let reserved: [String]
        let cases: [Case]
    }

    private func fixture() throws -> Fixture {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let url = root.appendingPathComponent("crates/qh-ffi/tests/fixtures/ident_quoting.json")
        let data = try XCTUnwrap(try? Data(contentsOf: url), "the shared fixture is missing: \(url.path)")
        return try JSONDecoder().decode(Fixture.self, from: data)
    }

    func testTheReservedWordsAreTheOnesTheFixtureWasMadeFrom() throws {
        let fixture = try fixture()
        XCTAssertEqual(Set(fixture.reserved), SQLIdentifierQuoting.reserved,
                       "the list changed; regenerate the fixture so both sides read the new one")
    }

    func testEveryCaseIsQuotedAsTheEngineQuotesIt() throws {
        let fixture = try fixture()
        XCTAssertGreaterThan(fixture.cases.count, fixture.reserved.count, "the awkward names are in it too")
        for entry in fixture.cases {
            XCTAssertEqual(SQLIdentifierQuoting.quoted(entry.name, for: .postgres), entry.postgres,
                           entry.name.debugDescription)
            XCTAssertEqual(SQLIdentifierQuoting.quoted(entry.name, for: .mysql), entry.mysql,
                           entry.name.debugDescription)
        }
    }
}
