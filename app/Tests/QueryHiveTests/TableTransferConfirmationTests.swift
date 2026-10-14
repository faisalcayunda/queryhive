import XCTest

@testable import QueryHive

/// The transfer sheet (W13-T20, B-2 and B-6, blueprint §7.3 and §7.4): every copy across
/// connections asks, by name, address, label and TLS for both ends; a prod source copying anywhere
/// that is not prod waits for a tick; and what runs is exactly what the sheet named.
@MainActor
final class TableTransferConfirmationTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        useMemorySecretStore()
    }

    private func connection(_ name: String, _ kind: ConnectionKind, safeMode: ConnectionSafeMode = .full,
                            environment: ConnectionEnvironment? = nil, sslmode: String = "",
                            verify: Bool = true) -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: kind, host: "\(name.lowercased()).internal",
                   port: kind.defaultPort, sslmode: sslmode, user: "qh", database: "app", schema: "public",
                   verify: verify, safeMode: safeMode, environment: environment)
    }

    private func fixture(source: Connection, target: Connection, engine: MockEngine = MockEngine())
        -> (AppModel, QueryTab) {
        let model = AppModel()
        model.engine = engine
        model.experiments = scratchExperiments(saveTableAcrossConnections: true)
        model.connections = [source, target]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = source.id
        tab.sql = "SELECT 1 AS id"
        tab.destination = .table
        tab.targetConnectionID = target.id
        tab.targetSchema = "public"
        tab.targetCatalog = "shop"
        tab.targetTable = "people"
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab)
    }

    // MARK: The request

    func testEveryCopyAsksWhateverTheTargetsSafeModeIs() {
        for level in [ConnectionSafeMode.full, .confirm] {
            let source = connection("Lake", .trino)
            let target = connection("Warehouse", .postgres, safeMode: level)
            let request = RunConfirmation.transferRequest(source: source, target: target, mode: .create,
                                                          table: ["public", "people"])
            XCTAssertEqual(request.source.name, "Lake")
            XCTAssertEqual(request.target.name, "Warehouse")
            XCTAssertEqual(request.confirmsTargetSafeMode, level == .confirm, "\(level)")
            XCTAssertEqual(RunConfirmation.transferApprovalSettings(request),
                           level == .confirm ? ["DEST_SAFE_MODE_CONFIRMED": "1"] : [:])
        }
    }

    func testTheSheetNamesBothConnectionsByAddressDatabaseLabelAndTLS() {
        let source = connection("Lake", .trino, environment: .prod)
        let target = connection("Warehouse", .postgres, environment: .staging, sslmode: "require")
        let request = RunConfirmation.transferRequest(source: source, target: target, mode: .create,
                                                      table: ["public", "people"])
        XCTAssertEqual(request.source.address, "lake.internal:8080")
        XCTAssertEqual(request.target.address, "warehouse.internal:5432")
        XCTAssertEqual(request.target.kind, "PostgreSQL")
        XCTAssertEqual(request.target.database, "app")
        XCTAssertEqual(request.source.environment, .prod)
        XCTAssertEqual(request.target.environment, .staging)
        XCTAssertEqual(request.target.tls, "require · unverified")
        XCTAssertTrue(request.target.tlsUnverified)
        XCTAssertEqual(request.target.safeMode, .full)
        XCTAssertTrue(request.note.contains("columns only"), request.note)
    }

    func testTheTargetNameIsQuotedTheWayTheTargetsDialectWritesIt() {
        let source = connection("Lake", .trino)
        let pg = RunConfirmation.transferRequest(source: source, target: connection("Warehouse", .postgres),
                                                 mode: .create, table: ["public", #"we"ird"#])
        XCTAssertEqual(pg.table, #"public."we""ird""#, "an embedded quote is doubled, not left to close the name")
        let my = RunConfirmation.transferRequest(source: source, target: connection("Shop", .mysql),
                                                 mode: .create, table: ["shop", "or`der"])
        XCTAssertEqual(my.table, "shop.`or``der`")
    }

    func testAProdSourceCopyingToAnythingButProdWaitsForATick() {
        let cases: [(ConnectionEnvironment?, ConnectionEnvironment?, Bool)] = [
            (.prod, nil, true), (.prod, .staging, true), (.prod, .dev, true), (.prod, .prod, false),
            (nil, nil, false), (nil, .prod, false), (.dev, nil, false), (.staging, .dev, false),
        ]
        for (from, to, asks) in cases {
            let request = RunConfirmation.transferRequest(
                source: connection("Lake", .trino, environment: from),
                target: connection("Warehouse", .postgres, environment: to), mode: .create,
                table: ["public", "people"])
            XCTAssertEqual(request.needsAcknowledgement, asks, "\(String(describing: from)) → \(String(describing: to))")
        }
    }

    func testReplaceIsTheDestructiveKind() {
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        let create = RunConfirmation.transferRequest(source: source, target: target, mode: .create, table: ["p", "t"])
        let replace = RunConfirmation.transferRequest(source: source, target: target, mode: .replace, table: ["p", "t"])
        XCTAssertFalse(create.destructive)
        XCTAssertTrue(replace.destructive)
        XCTAssertEqual(replace.confirmTitle, "Replace Table")
    }

    // MARK: Run opens the sheet, and only approval runs

    func testRunOpensTheTransferSheetEvenForAFullTargetAndRunsNothing() throws {
        let engine = MockEngine()
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres),
                                   engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer, "a full target is asked about too (B-6)")
        XCTAssertNil(model.pendingConfirmation, "the source's Safe Mode question is not this one")
        XCTAssertEqual(transfer.phase, .confirming)
        XCTAssertEqual(transfer.request.table, "public.people")
        XCTAssertTrue(engine.calls.isEmpty)
        XCTAssertEqual(tab.stage, .idle)
    }

    func testARestoredTabAsksAgain() throws {
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        let (model, original) = fixture(source: source, target: target)
        let restored = SessionTab(original).tab()
        model.tabs = [restored]
        model.selectedTabID = restored.id
        model.run(restored)
        XCTAssertNotNil(model.transfer, "a session brought back never runs a copy without the sheet")
    }

    func testAProdCopyCannotBeApprovedWithoutTheTick() throws {
        let engine = MockEngine()
        let (model, tab) = fixture(source: connection("Lake", .trino, environment: .prod),
                                   target: connection("Warehouse", .postgres), engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        XCTAssertFalse(transfer.mayApprove)
        model.approveTransfer(transfer)
        XCTAssertEqual(transfer.phase, .confirming)
        XCTAssertTrue(engine.calls.isEmpty)
        transfer.acknowledged = true
        XCTAssertTrue(transfer.mayApprove)
    }

    func testATargetDeletedWhileTheSheetIsUpRunsNothing() throws {
        let engine = MockEngine()
        let source = connection("Lake", .trino)
        let (model, tab) = fixture(source: source, target: connection("Warehouse", .postgres), engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        model.connections = [source]
        model.approveTransfer(transfer)
        XCTAssertEqual(transfer.phase, .failed)
        XCTAssertTrue(engine.calls.isEmpty)
    }

    func testATargetWhoseSafeModeChangedWhileTheSheetIsUpRunsNothing() throws {
        let engine = MockEngine()
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres),
                                   engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        model.connections[1].safeMode = .confirm
        model.approveTransfer(transfer)
        XCTAssertEqual(transfer.phase, .failed, "the sheet promised a full target; this one would need a confirmation it never asked")
        XCTAssertTrue(engine.calls.isEmpty)
    }

    // MARK: An approved copy, through a scripted engine

    private func events(_ lines: [String]) throws -> [Event] {
        try lines.map { try XCTUnwrap(EngineWire.event(in: Data($0.utf8)), $0) }
    }

    private func settle(_ condition: @escaping () -> Bool) {
        let done = expectation(description: "the scripted run ends")
        func poll() {
            if condition() { done.fulfill() } else { DispatchQueue.main.async(execute: poll) }
        }
        DispatchQueue.main.async(execute: poll)
        wait(for: [done], timeout: 5)
    }

    func testAnApprovedCopyRunsToTableWithBothConnectionsAndReportsTheVerifiedTable() throws {
        let engine = MockEngine()
        engine.answer("to_table", with: .events(try events([
            #"{"event":"step","step":"connect_target"}"#,
            #"{"event":"step","step":"connect"}"#,
            #"{"event":"step","step":"prepare"}"#,
            #"{"event":"step","step":"write"}"#,
            #"{"event":"start","columns":[{"name":"id","type":"integer"},{"name":"tags","type":"array(varchar)"}],"query_id":"q1","target_columns":[{"name":"id","type":"integer"},{"name":"tags","type":"jsonb"}],"fallbacks":[{"column":"tags","from":"array(varchar)","to":"jsonb"}]}"#,
            #"{"event":"progress","rows":2,"rows_read":3}"#,
            #"{"event":"step","step":"verify"}"#,
            #"{"event":"progress","rows":3,"rows_read":3}"#,
            #"{"event":"step","step":"swap"}"#,
            #"{"event":"done","rows":3,"rows_read":3,"table":"public.people","mode":"create","query_id":"q1","cancelled":false,"warnings":["column \"n\" (int8) is written as integer: wider"],"verified":true,"target_kind":"postgres"}"#,
        ])))
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        let (model, tab) = fixture(source: source, target: target, engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        model.approveTransfer(transfer)
        XCTAssertEqual(transfer.phase, .running)
        settle { transfer.phase != .running }

        let call = try XCTUnwrap(engine.calls.first)
        XCTAssertEqual(engine.calls.count, 1)
        XCTAssertEqual(call.command, "to_table")
        XCTAssertEqual(call.env["DB_HOST"], "lake.internal")
        XCTAssertEqual(call.env["DEST_DB_HOST"], "warehouse.internal")
        XCTAssertEqual(call.env["DEST_SAFE_MODE"], "full")
        XCTAssertNil(call.env["SAFE_MODE_CONFIRMED"])

        XCTAssertEqual(transfer.phase, .finished)
        XCTAssertEqual(transfer.rowsWritten, 3)
        XCTAssertEqual(transfer.rowsRead, 3)
        XCTAssertTrue(transfer.verified)
        XCTAssertEqual(transfer.summary, "3 rows copied from Lake to Warehouse · public.people (verified)")
        XCTAssertEqual(transfer.warnings, [#"column "n" (int8) is written as integer: wider"#])
        XCTAssertEqual(transfer.fallbacks.count, 1)
        XCTAssertEqual(tab.stage, .done)
        XCTAssertEqual(tab.writtenTable, "public.people")
        let log = tab.logLines.map(\.text)
        XCTAssertTrue(log.contains("Connecting to Warehouse…"), "\(log)")
        XCTAssertTrue(log.contains("Connecting to Lake…"), "\(log)")
        XCTAssertTrue(log.contains("3 rows copied from Lake to Warehouse · public.people (verified)"), "\(log)")
    }

    func testAFailureIsSaidAgainstTheConnectionItHappenedOn() throws {
        let engine = MockEngine()
        engine.answer("to_table", with: .failure(events: try events([
            #"{"event":"step","step":"connect_target"}"#,
            #"{"event":"error","message":"permission denied for schema public","side":"target"}"#,
        ]), status: 1, reason: "exit 1"))
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres),
                                   engine: engine)
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        model.approveTransfer(transfer)
        settle { transfer.phase != .running }
        XCTAssertEqual(transfer.phase, .failed)
        XCTAssertEqual(transfer.failure, "Warehouse: permission denied for schema public")
        XCTAssertEqual(tab.failure, "Warehouse: permission denied for schema public")
        XCTAssertNil(tab.writtenTable)
    }

    func testAConfirmTargetIsConfirmedByTheSheetsApprovalAlone() throws {
        let engine = MockEngine()
        engine.answer("to_table", with: .events([]))
        let (model, tab) = fixture(source: connection("Lake", .trino),
                                   target: connection("Warehouse", .postgres, safeMode: .confirm), engine: engine)
        model.run(tab)
        model.approveTransfer(try XCTUnwrap(model.transfer))
        let call = try XCTUnwrap(engine.calls.first)
        XCTAssertEqual(call.env["DEST_SAFE_MODE_CONFIRMED"], "1")
        XCTAssertNil(call.env["SAFE_MODE_CONFIRMED"])
    }

    // MARK: How a copy ends

    func testStopAfterTheRowsWereVerifiedStillReportsTheTableAsWritten() throws {
        let done = try XCTUnwrap(EngineWire.event(in: Data(
            #"{"event":"done","rows":3,"rows_read":3,"table":"public.people","cancelled":false,"verified":true}"#.utf8)))
        XCTAssertEqual(TransferOutcome.of(status: 0, done: done, stopRequested: true), .finished,
                       "the swap finishes once the rows are verified; saying nothing was written would be false")
        XCTAssertEqual(TransferOutcome.of(status: 0, done: done, stopRequested: false), .finished)
    }

    func testAStoppedCopyWritesNothingAndSaysSo() throws {
        let done = try XCTUnwrap(EngineWire.event(in: Data(
            #"{"event":"done","rows":0,"rows_read":40,"table":"public.people","cancelled":true,"verified":false,"warnings":["stopped; nothing was written to public.people","the scratch table qh_new_1 could not be dropped"]}"#.utf8)))
        XCTAssertEqual(TransferOutcome.of(status: 0, done: done, stopRequested: true), .stopped)
        XCTAssertEqual(TransferOutcome.of(status: 1, done: nil, stopRequested: true), .stopped)
        XCTAssertEqual(TransferOutcome.of(status: 1, done: nil, stopRequested: false), .failed)
        XCTAssertEqual(TransferOutcome.of(status: 0, done: nil, stopRequested: false), .failed,
                       "a clean exit with no table reported is not a finished copy")

        let tab = QueryTab(title: "q")
        let request = RunConfirmation.transferRequest(source: connection("Lake", .trino),
                                                      target: connection("Warehouse", .postgres),
                                                      mode: .create, table: ["public", "people"])
        let transfer = TableTransfer(tabID: tab.id, request: request, sql: "SELECT 1", sourceID: UUID(),
                                     target: connection("Warehouse", .postgres))
        AppModel.applyTransferEvent(done, to: tab, transfer: transfer, source: "Lake", target: "Warehouse")
        XCTAssertNil(tab.writtenTable, "a stopped copy has no table to report")
        XCTAssertNil(transfer.table)
        XCTAssertEqual(transfer.rowsRead, 40)
        XCTAssertEqual(transfer.warnings, ["the scratch table qh_new_1 could not be dropped"])
    }

    func testTheCopyEventsDecode() throws {
        let error = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"error","message":"m","side":"source"}"#.utf8)))
        XCTAssertEqual(error.side, "source")
        XCTAssertEqual(AppModel.attributed("m", side: error.side, source: "Lake", target: "Warehouse"), "Lake: m")
        XCTAssertEqual(AppModel.attributed("m", side: nil, source: "Lake", target: "Warehouse"), "m")
        let done = try XCTUnwrap(EngineWire.event(in: Data(
            #"{"event":"done","rows":1,"rows_read":1,"table":"t","verified":true,"target_kind":"mysql","warnings":[]}"#.utf8)))
        XCTAssertEqual(done.rowsRead, 1)
        XCTAssertEqual(done.verified, true)
        XCTAssertEqual(done.targetKind, "mysql")
    }
}
