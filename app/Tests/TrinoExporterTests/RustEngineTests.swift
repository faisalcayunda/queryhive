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
        // because the failure path is shared by both engines — the CLI's `error` line and the
        // last line of the stderr argument carry the same text — and the seven call sites in
        // `AppModel` read *either* one. This asserts they get the same text in both, which is what
        // makes the swap in §1.6 not a change of behaviour at those call sites.
        let (_, record) = await EngineContract.run(RustEngine(), "credential")

        XCTAssertEqual(record.exitStatus, 1, "a failed run is not a clean exit")
        XCTAssertEqual(record.events.map(\.event), ["error"])
        let message = record.events.first?.message
        XCTAssertEqual(message?.contains("CREDENTIAL_ACTION"), true,
                       "the engine's own wording, not its Debug rendering: \(message ?? "nil")")
        XCTAssertEqual(record.stderr, message,
                       "the same text in the argument the process engine put its log in")
        XCTAssertEqual(record.deliveriesOffMainThread, 0)
    }

    @MainActor
    func testTheFailureMessageIsTheEngineWordingAndNotAThrownError() async {
        // The message has to be the sentence the engine wrote, because the app's error bar shows
        // it. It must not be a language-level rendering of a failure — the old surface threw a
        // typed error whose `localizedDescription` printed the Swift case name at the user, and
        // this is the check that the failure still arrives as data rather than as a thrown value.
        let (_, record) = await EngineContract.run(RustEngine(), "credential")
        let message = record.events.first?.message ?? ""

        XCTAssertFalse(message.contains("EngineError"), "no type name is user-facing")
        XCTAssertFalse(message.contains("message:"), "nor are Swift's argument labels")
        XCTAssertTrue(message.hasPrefix("CREDENTIAL_ACTION"), "the engine's own first word: \(message)")
    }

    @MainActor
    func testStopReachesTheEngineRatherThanOnlyTheDelivery() async {
        // The half that used to be untestable. `terminate()` must reach the engine's own cancel
        // handle, because stopping only the delivery would leave the query running on the
        // coordinator — an app that believed otherwise would be wrong about its own database load.
        //
        // Asserted through the run's handle, over the real FFI: `RustRun.cancelHandle` is the
        // `RunCancel` the engine is holding, so a `terminate()` that reached it is visible as the
        // engine's own `isCancelled()`. The full effect of a stop — `done.cancelled`, the partial
        // file kept — is a Rust-side test (`golden.rs`, the cancelled-export case) because it
        // needs a server that answers with rows, which a unit test here does not have.
        let engine = RustEngine()
        let record = RunRecord()
        let handle = engine.run("credential", env: [:],
                                onEvent: { record.record(event: $0, onMainThread: Thread.isMainThread) },
                                onExit: { record.record(exit: $0, stderr: $1, onMainThread: Thread.isMainThread) })
        let rust = handle as? RustRun
        XCTAssertNotNil(rust, "the handle this engine hands back is its own")

        XCTAssertEqual(rust?.cancelHandle?.isCancelled(), false, "nothing has asked to stop yet")
        handle?.terminate()
        XCTAssertEqual(rust?.cancelHandle?.isCancelled(), true,
                       "the stop crossed into the engine, not just into this handle")
        // No request was made to the engine's own store, so this cannot leave anything behind.
        engine.terminateAll()
        await record.waitForExit()

        XCTAssertEqual(record.exits, 1, "a stopped run still has to end exactly once")
    }
}
