import XCTest

@testable import QueryHive

/// One engine run, recorded: what the two callbacks were handed and in what order.
///
/// A class, not a struct, because the callbacks mutate it from the main queue while the test
/// awaits — and because one of the contract's promises is the *order* of those two callbacks,
/// which needs state that outlives a single call.
final class RunRecord {
    private(set) var events: [Event] = []
    private(set) var exitStatus: Int32?
    private(set) var stderr: String?

    /// How many deliveries happened anywhere other than the main thread. The protocol says zero.
    private(set) var deliveriesOffMainThread = 0
    /// How many times `onExit` was called. The protocol says exactly once.
    private(set) var exits = 0
    /// Events that arrived after the exit, which the protocol says cannot happen.
    private(set) var eventsAfterExit = 0

    private var exited = false
    private var continuation: CheckedContinuation<Void, Never>?

    func record(event: Event, onMainThread: Bool) {
        if !onMainThread { deliveriesOffMainThread += 1 }
        if exited { eventsAfterExit += 1 }
        events.append(event)
    }

    func record(exit status: Int32, stderr: String, onMainThread: Bool) {
        if !onMainThread { deliveriesOffMainThread += 1 }
        exits += 1
        exitStatus = status
        self.stderr = stderr
        exited = true
        // Resuming a continuation that was never stored is the synchronous case: an engine that
        // cannot start calls `onExit` before `run` returns.
        continuation?.resume()
        continuation = nil
    }

    /// Waits for the exit, or returns immediately when it has already happened.
    func waitForExit() async {
        guard !exited else { return }
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }
}

/// The contract `DatabaseEngine`'s doc comment promises, as something that can be run.
///
/// Written over *any* engine rather than as a test of one, because the promise is what blueprint
/// §1.6's swap is judged by: `Engine.current = RustEngine()` may change which engine answers, not
/// what its callers are allowed to assume. It runs today against `MockEngine` and against the real
/// `RustEngine`, and the day both pass it the protocol has been checked against the engine that
/// will replace the Python one rather than against a description of it.
@MainActor
enum EngineContract {
    /// Runs one command and waits for it to end, however it ends.
    static func run(_ engine: any DatabaseEngine, _ command: String,
                    env: [String: String] = [:]) async -> (handle: (any EngineRun)?, record: RunRecord) {
        let record = RunRecord()
        let handle = engine.run(
            command, env: env,
            onEvent: { record.record(event: $0, onMainThread: Thread.isMainThread) },
            onExit: { status, stderr in
                record.record(exit: status, stderr: stderr, onMainThread: Thread.isMainThread)
            })
        // An engine that could not start has already finished. `waitForExit` knows.
        await record.waitForExit()
        return (handle, record)
    }

    /// The promises, asserted. Returns the record because every caller has its own follow-up
    /// question about the run it asked for.
    @discardableResult
    static func assertKept(_ engine: any DatabaseEngine, command: String,
                           env: [String: String] = [:]) async -> RunRecord {
        let (handle, record) = await run(engine, command, env: env)

        XCTAssertNotNil(handle, "a run that started must hand back the handle that stops it")
        XCTAssertEqual(record.exits, 1, "onExit is called exactly once")
        XCTAssertEqual(record.exitStatus, 0, "this run was scripted to end cleanly")
        XCTAssertEqual(record.deliveriesOffMainThread, 0,
                       "events and the exit arrive on the main queue")
        XCTAssertEqual(record.eventsAfterExit, 0, "the exit comes after the last event")
        XCTAssertFalse(record.events.isEmpty, "a run that ended cleanly reported something")
        return record
    }

    /// The other half of the contract: a command the engine cannot start.
    @discardableResult
    static func assertRefusedToStart(_ engine: any DatabaseEngine, command: String) async -> RunRecord {
        let (handle, record) = await run(engine, command)

        XCTAssertNil(handle, "an engine that could not start hands back no handle")
        XCTAssertEqual(record.exits, 1,
                       "the reason is already delivered when run returns, so callers keep one failure path")
        XCTAssertNotEqual(record.exitStatus, 0, "a refused start is not a success")
        XCTAssertEqual(record.events.count, 0, "nothing ran, so there is nothing to report as an event")
        XCTAssertEqual(record.deliveriesOffMainThread, 0, "the reason arrives on the main queue too")
        return record
    }
}
