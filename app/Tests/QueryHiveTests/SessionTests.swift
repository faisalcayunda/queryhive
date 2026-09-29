import XCTest

@testable import QueryHive

/// The session snapshot: what a tab is written down as, and what comes back.
///
/// The engine round trip is covered on the Rust side (`crates/qh-ffi/tests/local.rs`) and the wire
/// shape in `EventDecodingTests`. What is left here is the app's own translation, which is the part
/// that can silently drop a field: a snapshot that forgot `writeMode` would restore every tab as a
/// `CREATE`, and the app would not look broken until it ran one.
final class SessionTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    func testATabRoundTripsThroughItsSnapshot() {
        let tab = QueryTab(title: "Penerima")
        tab.sql = "SELECT * FROM penerima_manfaat"
        tab.connectionID = UUID()
        tab.destination = .table
        tab.format = .xlsx
        tab.outputName = "hasil"
        tab.outputDirectory = URL(fileURLWithPath: "/tmp/out")
        tab.rowLimit = 250
        tab.contextDatabase = "hive"
        tab.contextSchema = "gold"
        tab.targetCatalog = "hive"
        tab.targetSchema = "gold"
        tab.targetTable = "penerima"
        tab.writeMode = .replace

        let restored = SessionTab(tab).tab()

        XCTAssertEqual(restored.id, tab.id,
                       "the identity survives, so active_tab_id can name the front tab")
        XCTAssertEqual(restored.title, "Penerima")
        XCTAssertEqual(restored.sql, tab.sql)
        XCTAssertEqual(restored.connectionID, tab.connectionID)
        XCTAssertEqual(restored.destination, .table)
        XCTAssertEqual(restored.format, .xlsx)
        XCTAssertEqual(restored.outputName, "hasil")
        XCTAssertEqual(restored.outputDirectory?.path, "/tmp/out")
        XCTAssertEqual(restored.rowLimit, 250)
        XCTAssertEqual(restored.contextDatabase, "hive")
        XCTAssertEqual(restored.contextSchema, "gold")
        XCTAssertEqual(restored.targetCatalog, "hive")
        XCTAssertEqual(restored.targetSchema, "gold")
        XCTAssertEqual(restored.targetTable, "penerima")
        XCTAssertEqual(restored.writeMode, .replace)
    }

    func testAValueTheBuildNoLongerKnowsFallsBackRatherThanFailing() {
        // A snapshot written by a build that had a format this one does not. One unknown word in
        // one field is not a reason to lose the tab, so each falls back to the default a new tab
        // starts with.
        var snapshot = SessionTab(QueryTab(title: "Query 1"))
        snapshot.format = "parquet"
        snapshot.writeMode = "upsert"
        snapshot.destination = "warehouse"

        let tab = snapshot.tab()

        XCTAssertEqual(tab.format, .csv)
        XCTAssertEqual(tab.writeMode, .create)
        XCTAssertEqual(tab.destination, .file)
    }

    func testASnapshotSurvivesEncodingAndDecodingAsTheEngineCarriesIt() throws {
        // The engine stores the app's JSON as text and hands it back parsed, so the only thing that
        // matters about the encoding is that it is the same shape the event decoder reads. This
        // exercises exactly that: encode the way `AppModel` does, decode the way `EngineWire` does.
        let tab = QueryTab(title: "Query 1")
        tab.sql = "SELECT 1"
        tab.format = .json
        let json = String(data: try JSONEncoder().encode([SessionTab(tab)]), encoding: .utf8)
        let line = #"{"event":"session","action":"load","saved":true,"tabs":\#(json ?? "null")}"#

        let event = try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
        let restored = try XCTUnwrap(event.tabs?.first)
        XCTAssertEqual(restored.sql, "SELECT 1")
        XCTAssertEqual(restored.format, "json")
        XCTAssertEqual(restored.id, tab.id.uuidString)
    }

    /// The whole path, through the real engine and a real file: seed a session the way the last
    /// launch would have, then build the model and watch the tabs come back.
    ///
    /// This is the app-level half of the feature. It runs against the real `RustEngine` because
    /// `Engine.current` is a `static let` — the injection seam is still missing (`DatabaseEngine`'s
    /// own note) — so the isolation comes from `isolateConnectionStore()`, which redirects the
    /// store and, with it, the `DB_PATH` `AppModel` sends for session commands.
    @MainActor
    func testASeededSessionComesBackAsTabs() async throws {
        var snapshot = SessionTab(QueryTab(title: "Penerima"))
        snapshot.sql = "SELECT * FROM penerima_manfaat"
        snapshot.rowLimit = 250
        let database = try ConnectionStore.directory().appendingPathComponent("queryhive.sqlite3")
        await seedSession([snapshot], active: snapshot.id, database: database.path)

        let model = AppModel(persistsSession: true)
        // The load is asynchronous and answers on the main queue, so the test has to let that queue
        // run. It polls for the one thing the restore changes — the placeholder has no SQL and the
        // restored tab does — rather than sleeping a fixed amount and hoping.
        let deadline = Date().addingTimeInterval(5)
        while model.tabs.first?.hasSQL != true, Date() < deadline {
            try await Task.sleep(nanoseconds: 20_000_000)
        }

        XCTAssertEqual(model.tabs.count, 1, "the placeholder was replaced, not added to")
        XCTAssertEqual(model.tabs.first?.sql, "SELECT * FROM penerima_manfaat")
        XCTAssertEqual(model.tabs.first?.id.uuidString, snapshot.id,
                       "the identity came back, which is what active_tab_id names")
        XCTAssertEqual(model.selectedTabID, model.tabs.first?.id, "and it is the tab in front")
    }

    /// Writes a session through the real engine, the way a previous launch would have.
    @MainActor
    private func seedSession(_ tabs: [SessionTab], active: String?, database: String) async {
        let data = (try? JSONEncoder().encode(tabs)) ?? Data()
        let json = String(data: data, encoding: .utf8) ?? "[]"
        await withCheckedContinuation { continuation in
            _ = Engine.current.run("session", env: [
                "SESSION_ACTION": "save",
                "TABS_JSON": json,
                "ACTIVE_TAB_ID": active ?? "",
                "DB_PATH": database,
            ], onEvent: { _ in }, onExit: { _, _ in continuation.resume() })
        }
    }
}
