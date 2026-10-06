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

    /// An empty result store; its columns arrive later, with the run's `columns` event.
    func makeResultStore() throws -> StoreRows

    /// Runs `preview` or `explain` with its rows going into `store` instead of through `onEvent`
    /// (no `rows` events: the grid reads the store). Anything else is refused the way `run` refuses
    /// a command it cannot start. One store holds one run (D-12), so a new run makes a new store.
    @discardableResult
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void,
                      onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)?

    /// A finished result from rows in hand: fixtures, snapshots, benches and tests.
    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows

    /// Hints that a run against this connection is likely soon, so an engine that keeps sessions
    /// can open one now. Returns at once and reports nothing: it is an optimisation, and the run
    /// that follows behaves the same without it. `env` is what a run would send.
    func warmUp(env: [String: String])
}

extension DatabaseEngine {
    /// Most engines have nothing to warm; `MockEngine` and any other conformer inherit this.
    func warmUp(env: [String: String]) {}
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
    /// A `--snapshot` render draws fixture connections (`trino.internal` and friends) and must not
    /// dial them, so it gets an engine that accepts every run and never answers.
    static let current: any DatabaseEngine = Snapshot.requestedPath() != nil ? SilentEngine() : RustEngine()
}

/// Accepts every command and never reports anything, so a fixture's panes stay in their loading
/// state instead of retrying a host that does not exist.
struct SilentEngine: DatabaseEngine {
    private final class Idle: EngineRun { func terminate() {} }
    func run(_ command: String, env: [String: String], onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? { Idle() }
    func terminateAll() {}
    func runBlocking(_ command: String, env: [String: String]) {}
    // Making a store opens no connection, and a snapshot scene needs rows to draw.
    func makeResultStore() throws -> StoreRows { try RustEngine().makeResultStore() }
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void,
                      onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? { Idle() }
    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        try RustEngine().storeFromRows(columns: columns, rows: rows)
    }
}
