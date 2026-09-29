import XCTest

@testable import QueryHive

/// `DatabaseEngine`'s promises, held against every engine that exists.
///
/// The assertions themselves live in `EngineContract` so that this file is only the list of
/// engines: when the swap in blueprint §1.6 happens, the new engine is added here and the old one
/// is deleted, and neither event changes what "keeps the contract" means.
final class EngineContractTests: XCTestCase {
    /// A directory of its own per test, because `RustEngine`'s `connections` command opens the
    /// local store and must not be pointed at the one the app owns.
    private func temporaryDatabase() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("queryhive-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        return directory.appendingPathComponent("queryhive.sqlite3")
    }

    @MainActor
    func testMockEngineKeepsTheProtocolContract() async {
        let engine = MockEngine()
        engine.answer("connections", with: .events([Event(event: "connections")]))

        let record = await EngineContract.assertKept(engine, command: "connections",
                                                    env: ["DB_PATH": "/tmp/queryhive.sqlite3"])

        XCTAssertEqual(record.events.map(\.event), ["connections"])
        // What the caller asked for is part of the record too: an engine that changed the command
        // or dropped a setting on the way through would still deliver a plausible event.
        XCTAssertEqual(engine.calls, [.init(command: "connections",
                                            env: ["DB_PATH": "/tmp/queryhive.sqlite3"])])
    }

    @MainActor
    func testRustEngineKeepsTheProtocolContract() async throws {
        // The real engine, over the real FFI, with the one command that needs no server: the local
        // store, pointed at a database this test owns. Everything else this engine can do needs a
        // coordinator or a database, and a contract test that needed one of those would be a
        // contract test that only runs on the machine that has it.
        let database = try temporaryDatabase()
        let engine = RustEngine()

        let record = await EngineContract.assertKept(engine, command: "connections",
                                                    env: ["DB_PATH": database.path])

        XCTAssertEqual(record.events.map(\.event), ["connections"],
                       "the FFI returns the same lines the CLI writes, in order")
        XCTAssertEqual(record.stderr, "", "a clean run has no stderr, exactly like the process engine")
        XCTAssertTrue(FileManager.default.fileExists(atPath: database.path),
                      "the run really reached the store: it created and migrated its own database")
    }

    @MainActor
    func testAnEngineThatCannotStartReportsTheReasonBeforeRunReturns() async {
        // The mock with nothing scripted, and the real engine asked for a word that is not one of
        // the fourteen. Both are "this engine cannot start this", which the protocol answers with
        // no handle and an already-delivered reason.
        let mock = MockEngine()
        let mockRecord = await EngineContract.assertRefusedToStart(mock, command: "objects")
        XCTAssertTrue(mockRecord.stderr?.contains("nothing scripted") == true,
                      "the reason says what was wrong: \(mockRecord.stderr ?? "nil")")

        let rust = RustEngine()
        let rustRecord = await EngineContract.assertRefusedToStart(rust, command: "definitely_not_a_command")
        XCTAssertTrue(rustRecord.stderr?.contains("definitely_not_a_command") == true,
                      "the reason names the command that could not be answered: \(rustRecord.stderr ?? "nil")")
    }

    @MainActor
    func testTheContractTestNoticesAnEngineThatAnswersOffTheMainQueue() async {
        // The negative control. Every delivery promise above is only worth asserting if an engine
        // that broke one would be caught, and this is that engine: it runs correctly and delivers
        // on a global queue. If this ever stops failing to be zero, the tests above stopped
        // meaning anything.
        let engine = MockEngine(onMainQueue: false)
        engine.answer("connections", with: .events([Event(event: "connections")]))

        let (handle, record) = await EngineContract.run(engine, "connections")

        XCTAssertNotNil(handle, "the run still happened; the delivery is what is wrong")
        XCTAssertGreaterThan(record.deliveriesOffMainThread, 0,
                             "so `assertKept` would fail this engine, which is the point of it")
    }

    @MainActor
    func testTerminateAllLeavesTheEngineUsableAfterwards() async throws {
        // `terminateAll()` is called when the app terminates, and an engine that could never be
        // used again afterwards would be a trap for the next run in the same process. `RustEngine`
        // keeps the runs it has started in a static set, so this also checks that a finished run
        // is taken back out of it rather than accumulating.
        let database = try temporaryDatabase()
        let engine = RustEngine()
        await EngineContract.assertKept(engine, command: "connections", env: ["DB_PATH": database.path])

        engine.terminateAll()

        let second = await EngineContract.assertKept(engine, command: "connections", env: ["DB_PATH": database.path])
        XCTAssertEqual(second.events.map(\.event), ["connections"])
    }
}
