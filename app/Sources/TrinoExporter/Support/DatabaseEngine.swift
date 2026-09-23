import Foundation

/// The one way the UI reaches an engine (blueprint §1.6).
///
/// Three conformers exist, and they are at three different stages:
///
/// - `PythonEngine` launches the bundled Python process and decodes its NDJSON events (ADR-0001).
///   It is what `Engine.current` names, so it is what the app runs on today.
/// - `RustEngine` calls into the Rust engine over UniFFI, and is the only type that imports the
///   FFI module. It compiles, it keeps this contract, and it is not wired in: the legacy engine is
///   removed in the same change that flips `Engine.current` (PROGRESS.md, "Keputusan sapu
///   bersih"), and that change is not this one. Its own doc comment lists the three things the FFI
///   cannot answer yet — no streamed events, no cancellation, no process to report on.
/// - `MockEngine` (in the test target) answers from a script, so this protocol's promises and the
///   app's decoding can be checked without an engine at all. Nothing in the app can be *handed* an
///   engine yet, so it cannot be reached from the app's own code — that is a missing seam, not a
///   missing mock.
///
/// The shape is deliberately the shape the current engine already has: a command, the environment
/// that parameterises it, an event callback and an exit callback. The typed
/// `descriptors()/browse()/objects()/run()/export()` surface §1.6 sketches is the *target*, and
/// promising it here would promise operations no implementation in this tree can perform yet. The
/// gap is now measured rather than assumed, and it is wider than "typed events": over the FFI today
/// there is no streamed event delivery (one blocking call returns every line at once), no cancel
/// entry point at all (`CancelFlag` exists in the crate and is not exported), no typed event or
/// page types, no typed `ConnectionTest`/`DriverDescriptor`, no result-set handle or window (ADR-0004
/// keeps the data plane off this surface deliberately), and no command for query history or saved
/// queries at all. Every one of those is an FFI addition, not an app-side one.
protocol DatabaseEngine: Sendable {
    /// Runs one operation. Events and the exit handler arrive on the main queue.
    ///
    /// Returns the handle that stops it, or `nil` when the engine could not be started at all — in
    /// which case `onExit` has already been called with the reason, so every caller keeps its one
    /// failure path.
    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)?

    /// Stops everything this engine still has running, used when the app terminates: an engine
    /// child left behind would keep writing after the window is gone.
    func terminateAll()
}

/// A handle to one running operation. Callers only ever stop it, so stopping it is all this
/// promises: the Python engine's handle is the child `Process`, and the Rust engine's is `RustRun`
/// — two things with nothing else in common. They stop *differently* as well, and the difference is
/// real rather than cosmetic: `Process.terminate()` ends the query, and `RustRun.terminate()` ends
/// only the delivery because the FFI has no cancel entry point yet.
protocol EngineRun: AnyObject {
    func terminate()
}

/// `Process` already stops with `terminate()`; conforming it here is what keeps the Python engine's
/// handle from leaking into the protocol's return type.
extension Process: EngineRun {}

/// The engine the app runs on, and the composition root the blueprint's §1.6 rule needs: only this
/// line names a concrete engine, so Fase 2 replaces `PythonEngine()` with `RustEngine()` and every
/// call site follows. It stays `PythonEngine()` until then — `RustEngine` exists and keeps this
/// protocol's contract (`EngineContractTests`), and it is not finished: the FFI cannot stream an
/// export's events or cancel one, so naming it now would make a long export look frozen and its
/// Stop button a lie.
enum Engine {
    static let current: any DatabaseEngine = PythonEngine()
}
