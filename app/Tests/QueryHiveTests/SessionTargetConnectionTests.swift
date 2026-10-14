import XCTest

@testable import QueryHive

/// The target connection survives a relaunch by identity, an older session reads as it always
/// did, and a stored value that cannot be resolved blocks the copy instead of becoming "the tab's
/// own connection" (W13-T20, blueprint §8.2, B-6).
@MainActor
final class SessionTargetConnectionTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        useMemorySecretStore()
    }

    private func tableTab() -> QueryTab {
        let tab = QueryTab(title: "Penerima")
        tab.sql = "SELECT 1"
        tab.connectionID = UUID()
        tab.destination = .table
        tab.targetSchema = "public"
        tab.targetTable = "people"
        return tab
    }

    func testTheTargetConnectionRoundTrips() {
        let tab = tableTab()
        tab.targetConnectionID = UUID()
        let restored = SessionTab(tab).tab()
        XCTAssertEqual(restored.targetConnectionID, tab.targetConnectionID)
    }

    func testATabWithoutATargetWritesTheSameBlobAsBefore() throws {
        let tab = tableTab()
        let data = try JSONEncoder().encode(SessionTab(tab))
        let object = try XCTUnwrap(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertNil(object["targetConnection"], "no key at all, so the stored session is unchanged")
        XCTAssertNil(SessionTab(tab).tab().targetConnectionID)
    }

    func testASessionWrittenBeforeTheFieldExistedStillReads() throws {
        let old = #"{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","title":"T","sql":"SELECT 1","connection":null,"destination":"table","format":"csv","outputName":"export","outputDirectory":null,"rowLimit":100,"contextDatabase":"","contextSchema":"","targetCatalog":"","targetSchema":"public","targetTable":"people","writeMode":"create"}"#
        let decoded = try JSONDecoder().decode(SessionTab.self, from: Data(old.utf8))
        XCTAssertNil(decoded.targetConnection)
        XCTAssertNil(decoded.tab().targetConnectionID)
    }

    func testAStoredValueThatIsNotAUUIDBlocksTheCopy() throws {
        var snapshot = SessionTab(tableTab())
        snapshot.targetConnection = "not-a-uuid"
        let tab = snapshot.tab()
        XCTAssertEqual(tab.targetConnectionID, QueryTab.unresolvableConnection,
                       "read as nil it would write the table on this tab's own connection")

        let model = AppModel()
        model.engine = MockEngine()
        model.experiments = scratchExperiments(saveTableAcrossConnections: true)
        let source = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .postgres,
                                host: "localhost", port: 5432, user: "qh", database: "app",
                                schema: "public", verify: true)
        model.connections = [source]
        tab.connectionID = source.id
        XCTAssertEqual(model.tableDestination(for: tab), .missing)
        XCTAssertEqual(model.runBlockedReason(for: tab), "Choose the target connection again")
    }

    func testATargetDeletedSinceTheSessionWasSavedBlocksTheCopy() {
        let tab = tableTab()
        tab.targetConnectionID = UUID()
        let restored = SessionTab(tab).tab()
        let model = AppModel()
        model.engine = MockEngine()
        model.experiments = scratchExperiments(saveTableAcrossConnections: true)
        let source = Connection(id: restored.connectionID ?? UUID(), name: "Warehouse", color: .violet,
                                kind: .postgres, host: "localhost", port: 5432, user: "qh", database: "app",
                                schema: "public", verify: true)
        model.connections = [source]
        XCTAssertEqual(model.tableDestination(for: restored), .missing)
        XCTAssertEqual(model.runBlockedReason(for: restored), "Choose the target connection again")
    }
}
