import XCTest
import QueryHiveFFI

@testable import QueryHive

/// The parts of `RustEngine` that only make sense against the real FFI.
///
/// The protocol's own promises are checked in `EngineContractTests`; what is left here are the two
/// places this engine and the generated bindings can disagree, plus the failure path — which,
/// unlike every other path this engine has, needs no coordinator and no driver to reach.
final class RustEngineTests: XCTestCase {
    func testTheCommandsTheEngineAnswersToAreTheOnesTheFFIMakes() {
        // The drift this exists to catch: UniFFI generates an enum of commands and a function that
        // lists their names, but nothing that turns a name back into a case. So `RustEngine` keeps
        // its own word → case dictionary (the app asks by the CLI's word, `command: String`), and
        // without this test a command added on the Rust side would be one the app silently cannot
        // reach — and would refuse to start instead of failing loudly.
        XCTAssertEqual(RustEngine.commandWords, commandNames().sorted(),
                       "every command the FFI offers is one the app can ask it for")
    }

    func testTheEngineReportsTheBuildItIs() {
        // Not a version assertion — that would be a test that breaks on every release — but the
        // check that the dylib the app linked is the one answering, and that it can answer at all.
        XCTAssertFalse(engineVersion().isEmpty)
    }

    @MainActor
    func testAFailureReachesTheUIInBothPlacesTheCallSitesLook() async {
        // `credential` without its action is a usage failure, and it is the one kind of failure
        // that needs no server: the engine decides it before it touches anything. It is here
        // because the failure path is the one the FFI expresses differently from the process
        // engine — a typed error rather than an exit status and a log — and the seven call sites
        // in `AppModel` read *either* the `error` event's message *or* the last line of the stderr
        // argument. This asserts they get the same text in both, which is what makes the swap in
        // §1.6 not a change of behaviour at those call sites.
        let record = await EngineContract.run(RustEngine(), "credential")

        XCTAssertEqual(record.exitStatus, 1, "a failed run is not a clean exit")
        XCTAssertEqual(record.events.map(\.event), ["error"])
        let message = record.events.first?.message
        XCTAssertEqual(message?.contains("CREDENTIAL_ACTION"), true,
                       "the engine's own wording, not our enum's Debug rendering: \(message ?? "nil")")
        XCTAssertEqual(record.stderr, message,
                       "the same text in the argument the process engine put its log in")
        XCTAssertEqual(record.deliveriesOffMainThread, 0)
    }

    @MainActor
    func testTheTypedErrorIsNotShownToTheUserAsSwift() async {
        // UniFFI generates `EngineError`'s `localizedDescription` as `String(reflecting: self)`,
        // which prints `EngineError.Usage(message: "…")` at the user. If that ever reaches the
        // error bar this test fails, because it is exactly the kind of thing nobody notices until
        // a user sends a screenshot.
        let record = await EngineContract.run(RustEngine(), "credential")
        let message = record.events.first?.message ?? ""

        XCTAssertFalse(message.contains("EngineError"), "the enum's own name is not user-facing")
        XCTAssertFalse(message.contains("message:"), "nor are Swift's argument labels")
    }

    @MainActor
    func testStoppingARunBeforeItFinishesKeepsTheReasonOut() async {
        // The honest shape of cancellation today: `terminate()` cannot stop the FFI call (there is
        // no cancel entry point — see `RustEngine`'s note), so what it can do is stop the delivery.
        // `terminateAll()` is the app's call at termination, and this is the closest a test without
        // a server can get to it: the call runs, nothing is handed to a caller that has stopped
        // waiting, and `onExit` still arrives exactly once because the contract says so.
        let engine = RustEngine()
        let record = RunRecord()
        let handle = engine.run("credential", env: [:],
                                onEvent: { record.record(event: $0, onMainThread: Thread.isMainThread) },
                                onExit: { record.record(exit: $0, stderr: $1, onMainThread: Thread.isMainThread) })
        handle?.terminate()
        // No request was made to the engine's own store, so this cannot leave anything behind.
        engine.terminateAll()
        await record.waitForExit()

        XCTAssertEqual(record.exits, 1, "a stopped run still has to end exactly once")
        XCTAssertEqual(record.events.count, 0, "the failure the caller stopped waiting for is not delivered")
    }
}
