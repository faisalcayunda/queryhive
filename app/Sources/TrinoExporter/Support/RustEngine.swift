import Foundation
import QueryHiveFFI

/// The Fase 2 engine: the same fourteen commands, reached by calling into the Rust engine instead
/// of spawning the bundled Python process (blueprint §1.6, ADR-0004).
///
/// This is the only type in the app that imports the FFI module, and `Engine.current` does not
/// name it yet: the legacy engine is removed in the same change that flips that line
/// (`PROGRESS.md`, "Keputusan sapu bersih"), and that change is not this one. So what this file is
/// allowed to be today is a conformer that compiles, honours the contract below, and does not
/// pretend about the three places the FFI cannot answer yet:
///
/// 1. **Events are not streamed.** `run` in `crates/qh-ffi/src/uniffi_api.rs` builds a runtime of
///    its own, runs the whole command, and returns every line at once (`Result<Vec<String>,
///    EngineError>`). The Python engine's events arrive as the child writes them, which is what
///    makes an `export`'s progress move while the export runs; here the same events arrive in the
///    same order, at the end. Nothing a caller sees is wrong, but a long export would look frozen.
///    The FFI's own module note says why: typed events and a stream are the next step, and doing
///    them before there is a consumer would design a hierarchy against no caller.
/// 2. **Cancellation is not wired.** The same note says a cancel entry point is separate, later
///    work because "the app's cancel has to reach a command that is already running". There is
///    nothing in the surface to reach for: `CancelFlag` exists in the crate but is not exported
///    over UniFFI. So `terminate()` here stops the *delivery*, not the query — see `RustRun`.
/// 3. **There is no process, and no stderr.** The protocol's second callback argument is the
///    child's stderr, and the seven call sites that read it parse its last line for a failure
///    message. The FFI reports failures as a typed `EngineError` instead, so this engine writes
///    the message into both places at once: an `error` event carrying it, and the same text in
///    that argument. A call site that looks at either one keeps working unchanged. The exit
///    status becomes 1, which is what the CLI exits with for the same failure.
///
/// `env` is passed through as data, with no translation at all: the FFI's `Setting` pairs are the
/// same keys the CLI reads, so the app's existing environment is already the FFI's input. That is
/// the one part of this engine that needed no decision.
struct RustEngine: DatabaseEngine {
    /// The runs this engine has started and not yet finished, so `terminateAll()` can stop them
    /// all. Static, for `PythonEngine`'s reason: the app delegate holds no engine, it asks
    /// `Engine.current`, and one instance may not be the one that started a run. Mutated on the
    /// main queue (`run` is called from it, and the completion handler returns there), which is
    /// the same assumption `PythonEngine.running` makes.
    private static var running = Set<RustRun>()

    /// The status this engine reports for a command it cannot start. `-1` is `PythonEngine`'s for
    /// the same situation, and no process exit status is ever negative, so a caller cannot confuse
    /// it with a failed run.
    private static let cannotStart: Int32 = -1

    /// The commands the FFI has a case for, spelled as the CLI spells them.
    ///
    /// A dictionary rather than a `switch` because there is a test that compares its keys against
    /// the FFI's own `commandNames()`: UniFFI generates the enum and a list of names but no way to
    /// turn a name back into a case, so this mapping is a second list of the same words and the
    /// only thing standing between the two is that test (`RustEngineTests`).
    private static let commands: [String: EngineCommand] = [
        "db_drivers": .dbDrivers,
        "connections": .connections,
        "import_connections": .importConnections,
        "credential": .credential,
        "objects": .objects,
        "test": .test,
        "catalogs": .catalogs,
        "schemas": .schemas,
        "tables": .tables,
        "export": .export,
        "to_table": .toTable,
        "preview": .preview,
        "count": .count,
        "explain": .explain,
    ]

    /// Every word this engine answers to. Internal so the test can hold it against `commandNames()`.
    static var commandWords: [String] { commands.keys.sorted() }

    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        guard let named = Self.commands[command] else {
            // The one way this engine cannot start, and the reason the protocol asks for `nil`
            // plus a completed `onExit`: the caller's own request named something this engine has
            // no case for. The CLI reaches the same situation by printing a usage line; an FFI
            // caller asked by name, so the name is what the reason is about.
            onExit(Self.cannotStart, "The Rust engine does not offer '\(command)'.")
            return nil
        }

        let handle = RustRun(command: command)
        Self.running.insert(handle)
        // The keys, not just the values: `Setting` is a record, and the app's dictionary is
        // already exactly that. `map` keeps the caller's dictionary iteration order out of the
        // picture by never relying on it — settings are read by key on the other side.
        let settings = env.map { Setting(key: $0.key, value: $0.value) }

        DispatchQueue.global().async {
            // One FFI call, and it is a blocking one by design (`uniffi_api.rs`, "Blocking,
            // deliberately"): it must not run on the main thread, and a concurrent queue is not
            // the main thread. The call also builds its own tokio runtime and drops it, which is
            // why a run costs a thread pool rather than reusing one.
            let outcome: Result<[String], EngineError>
            do {
                outcome = .success(try QueryHiveFFI.run(command: named, settings: settings))
            } catch let error as EngineError {
                outcome = .failure(error)
            } catch {
                // UniFFI only throws the declared error type, but the surface is allowed to grow
                // one, and a `catch` that is not exhaustive would be a crash rather than a message.
                DispatchQueue.main.async {
                    Self.finish(handle, onEvent: onEvent, onExit: onExit, lines: [], failure: nil,
                                unexpected: error)
                }
                return
            }

            DispatchQueue.main.async {
                switch outcome {
                case .success(let lines):
                    Self.finish(handle, onEvent: onEvent, onExit: onExit, lines: lines,
                                failure: nil, unexpected: nil)
                case .failure(let error):
                    Self.finish(handle, onEvent: onEvent, onExit: onExit, lines: [],
                                failure: error, unexpected: nil)
                }
            }
        }
        return handle
    }

    func terminateAll() {
        // Dropping the handles is the whole of it: the FFI call itself cannot be interrupted
        // (see the type note), so all `terminate()` can do is promise no event reaches the UI
        // after the window is gone — which is the reason the app delegate calls this at all.
        let handles = Self.running
        Self.running.removeAll()
        handles.forEach { $0.terminate() }
    }

    /// Everything the main queue does once the call has returned: the events, then the exit.
    ///
    /// One helper for all three ways the call can end, because the ordering rule ("events, then
    /// `onExit`, exactly once, on the main queue") is the part callers depend on and it should not
    /// exist in three slightly different copies.
    private static func finish(_ handle: RustRun, onEvent: @escaping (Event) -> Void,
                               onExit: @escaping (_ status: Int32, _ stderr: String) -> Void,
                               lines: [String], failure: EngineError?, unexpected: (any Error)?) {
        running.remove(handle)

        if let unexpected {
            // A failure the FFI did not declare. Worded like the CLI's own catch-all so the user
            // reads the same sentence whichever engine wrote it.
            let message = "internal error: \(unexpected.localizedDescription)"
            onEvent(Event(event: "error", message: message))
            onExit(1, message)
            return
        }

        if let failure {
            let (message, warnings) = describe(failure)
            // The same pairing the CLI writes for a failure: the event the UI shows, and the text
            // it keeps as detail. `warnings` is left out when empty — mirroring `main.rs`, where
            // the field is omitted rather than sent as `[]`.
            if !handle.isStopped {
                onEvent(Event(event: "error", message: message,
                              warnings: warnings.isEmpty ? nil : warnings))
            }
            onExit(1, message)
            return
        }

        for line in lines where !handle.isStopped {
            if let event = EngineWire.event(in: Data(line.utf8)) { onEvent(event) }
        }
        // Status 0 and no stderr, which is what a Python run that ended cleanly reports too.
        onExit(0, "")
    }

    /// What to tell the user about a typed failure.
    ///
    /// Not `localizedDescription`: UniFFI generates that as `String(reflecting: self)`, which
    /// prints the Swift case name and its labels at the user
    /// (`EngineError.Connect(message: "…")`). The app's error bar is already worded for users, so
    /// this reads the message the engine wrote. `Failed`'s `kind` and `Partial`'s `warnings` are
    /// not dropped silently: the warnings are reported beside the message, as the CLI reports
    /// them, and the `kind` is the one piece of the typed error that has nowhere to go yet —
    /// `PROGRESS.md` records that using it means changing every failure path in `AppModel`, which
    /// this change is not allowed to touch.
    private static func describe(_ error: EngineError) -> (message: String, warnings: [String]) {
        switch error {
        case .Usage(let message), .Connect(let message), .Query(let message):
            return (message, [])
        case .Partial(let message, let warnings):
            return (message, warnings)
        case .Failed(_, let message):
            return (message, [])
        }
    }
}

/// The handle a caller holds while a Rust run is in flight.
///
/// `EngineRun` promises one thing — "the handle that stops it" — and this is the honest half of it.
/// The FFI call cannot be interrupted (there is no cancel entry point to call), so `terminate()`
/// stops the events from being delivered to a caller that has moved on, and the query keeps
/// running to completion in the background where nobody is waiting for its result. That is worth
/// writing down rather than hiding: an app that believed `terminate()` had stopped the query would
/// be wrong about its own database load.
///
/// A run is its own identity — a second run of the same command is a different run — so equality
/// is identity rather than the fields a subclass-style `Equatable` would compare.
final class RustRun: EngineRun, Hashable {
    let command: String

    private let lock = NSLock()
    private var stopped = false

    init(command: String) {
        self.command = command
    }

    /// Whether the caller has stopped waiting for this run. Read on the main queue, written from
    /// wherever `terminate()` is called — including the app delegate's `applicationWillTerminate`,
    /// which is why this is locked rather than a plain `Bool`.
    var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    func terminate() {
        lock.lock()
        defer { lock.unlock() }
        stopped = true
    }

    static func == (lhs: RustRun, rhs: RustRun) -> Bool { lhs === rhs }

    func hash(into hasher: inout Hasher) { hasher.combine(ObjectIdentifier(self)) }
}
