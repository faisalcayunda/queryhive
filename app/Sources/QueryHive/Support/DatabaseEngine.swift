import Foundation

/// The one way the UI reaches an engine (blueprint §1.6).
///
/// Two conformers exist:
///
/// - `RustEngine` calls into the Rust engine over UniFFI, and is the only type that imports the
///   FFI module. It is what `Engine.current` names, so it is what the app runs on. Events arrive
///   as the engine emits them and a Stop button really does cancel the query.
/// - `MockEngine` (in the test target) answers from a script, so this protocol's promises and the
///   app's decoding can be checked without an engine at all. Nothing in the app can be *handed* an
///   engine yet, so it cannot be reached from the app's own code — that is a missing seam, not a
///   missing mock.
///
/// The shape is deliberately the shape the engine has: a command, the environment that
/// parameterises it, an event callback and an exit callback. The typed
/// `descriptors()/browse()/objects()/run()/export()` surface §1.6 sketches is the *target*, and
/// promising it here would promise operations no implementation in this tree can perform yet. The
/// gap is now measured rather than assumed, and it is wider than "typed events": there are no typed
/// event or page types, no typed `ConnectionTest`/`DriverDescriptor`, no result-set handle or
/// window (ADR-0004 keeps the data plane off this surface deliberately). Query history, saved
/// queries and session restore used to be on that list and are not any more: `history`,
/// `history_add`, `history_clear`, `saved_queries` and `session` exist as of 29 Sep 2026, which is
/// why this paragraph no longer names them.
/// Everything still on the list is an FFI addition, not an app-side one.
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

    /// Stops everything this engine still has running, used when the app terminates: a run left
    /// in flight would keep working after the window is gone.
    func terminateAll()

    /// Runs one command to completion on the calling thread and discards its events.
    ///
    /// One caller: `AppModel` writing the session during `applicationWillTerminate`. `run` reports
    /// completion by hopping to the main queue, and at termination the main queue is the one
    /// waiting — a completion scheduled there would never arrive, so the save would be asked for
    /// and never happen. This blocks instead of hopping.
    ///
    /// Synchronous by necessity, so it is deliberately not the general path: everything that can
    /// wait uses `run`, which keeps the main thread free.
    func runBlocking(_ command: String, env: [String: String])
}

/// A handle to one running operation. Callers only ever stop it, so stopping it is all this
/// promises. `RustRun` is the only conformer — the handle is per-run because one process can run
/// several commands at once, one per tab, and stopping one must not touch the others.
protocol EngineRun: AnyObject {
    func terminate()
}

/// The engine the app runs on, and the composition root the blueprint's §1.6 rule needs: only this
/// line names a concrete engine, so every call site follows from here.
enum Engine {
    static let current: any DatabaseEngine = RustEngine()
}
