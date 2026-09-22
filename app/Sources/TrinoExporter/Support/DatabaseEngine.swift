import Foundation

/// The one way the UI reaches an engine (blueprint §1.6).
///
/// There is exactly one conformer today, `PythonEngine`, which launches the bundled Python process
/// and decodes its NDJSON events (ADR-0001). Fase 2 adds `RustEngine` — the only type that will
/// import the FFI module — and because every call site goes through this protocol, that swap does
/// not touch the UI.
///
/// The shape is deliberately the shape the current engine already has: a command, the environment
/// that parameterises it, an event callback and an exit callback. The typed
/// `descriptors()/browse()/objects()/run()/export()` surface §1.6 sketches is the *target*, and
/// promising it here would promise operations no implementation in this tree can perform yet.
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
/// promises: the Python engine's handle is the child `Process`, and the Rust engine's will be a
/// query handle (§1.6) — two things with nothing else in common.
protocol EngineRun: AnyObject {
    func terminate()
}

/// `Process` already stops with `terminate()`; conforming it here is what keeps the Python engine's
/// handle from leaking into the protocol's return type.
extension Process: EngineRun {}

/// The engine the app runs on, and the composition root the blueprint's §1.6 rule needs: only this
/// line names a concrete engine, so Fase 2 replaces `PythonEngine()` with `RustEngine()` and every
/// call site follows.
enum Engine {
    static let current: any DatabaseEngine = PythonEngine()
}
