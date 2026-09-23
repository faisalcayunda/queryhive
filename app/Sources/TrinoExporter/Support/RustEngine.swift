import Foundation
import QueryHiveFFI

/// The Fase 2 engine: the same fourteen commands, reached by calling into the Rust engine instead
/// of spawning the bundled Python process (blueprint §1.6, ADR-0004).
///
/// This is the only type in the app that imports the FFI module, and `Engine.current` does not
/// name it yet: the legacy engine is removed in the same change that flips that line
/// (`PROGRESS.md`, "Keputusan sapu bersih"), and that change is not this one.
///
/// The two gaps this file used to document are closed, and what is left is the two places the FFI
/// is *deliberately* not the process engine:
///
/// 1. **Events stream.** `run` takes an `EventSink` and the engine calls it as it produces each
///    event, so an `export`'s progress moves while the export runs — the same thing the Python
///    engine's `print(..., flush=True)` bought, and the reason a `Vec<String>` return value was
///    not enough no matter how complete it was.
/// 2. **Stop stops the query.** `run` also takes a `RunCancel`, made here before the call and kept
///    on the handle, so `terminate()` reaches a run that is still in flight. The engine reads the
///    same flag a SIGTERM sets — between rows and between statements — so a stopped `export`
///    finishes the statement it is on, keeps the bytes it already wrote, and reports `done` with
///    `cancelled: true`. One honest limit, unchanged from the Python engine: `preview` and
///    `explain` do not poll the flag (`commands.rs`, "nothing polls for a cancel here"), so
///    stopping a preview takes effect at the end of the statement rather than during it. In the
///    old engine the same was true and it was invisible, because SIGTERM killed the child at the
///    end anyway.
///
/// The one thing that is genuinely gone is the child process:
///
/// - **There is no stderr.** The protocol's second callback argument is the child's log, and the
///   call sites that read it take its last line as a failure message. There is no log here, so
///   this engine writes the message into both places at once: an `error` event carrying it, and
///   the same text in that argument. A call site that looks at either one keeps working
///   unchanged. The exit status is 1 for a failure and 0 otherwise, which is what the CLI exits
///   with for the same run.
/// - **A secret cannot leak the way it used to, and that is not the same as being redacted.**
///   `PythonEngine` scrubbed its environment's secret-looking values out of every message before
///   the UI saw one, because a Python traceback can echo a live password. These messages are
///   built by the engine rather than by an interpreter: a connection renders through
///   `ConnectionConfig::redacted()` / its own `Debug`, neither of which prints a password, and the
///   tunnel's failures name a setting rather than its value (`tunnel.rs`'s `TunnelConfigError`).
///   So the scrubber is not carried over — and no test asserts that absence, which is the honest
///   state of it: it holds by construction, not by check.
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

        // Made *before* the call, and that ordering is the whole reason `RunCancel` is the
        // caller's object rather than `run`'s return value: the FFI call does not return until the
        // command is over, so a handle it handed back could only ever be used once there was
        // nothing left to stop. Stop is pressed on the main queue while this call is blocked on
        // another thread, which is why the handle has to exist first.
        let cancel = RunCancel()
        handle.attach(cancel: cancel)
        let sink = Sink(handle: handle, onEvent: onEvent)

        DispatchQueue.global().async {
            // One FFI call, and it is a blocking one by design (`uniffi_api.rs`, "The call blocks
            // until the command has ended, so it belongs off the main thread"): it must not run on
            // the main thread, and a concurrent queue is not the main thread. The call also builds
            // its own tokio runtime and drops it, which is why a run costs a thread pool rather
            // than reusing one.
            //
            // No `do`/`catch`: the FFI has no failure to throw. A command that fails emits one
            // `error` event through the sink, so there is exactly one failure path for the caller
            // to read — the same one the CLI writes.
            QueryHiveFFI.run(command: named, settings: settings, sink: sink, cancel: cancel)

            // Every event has already been handed to the sink's queue by now, so this lands after
            // the last one: the queue is serial and the sink hopped to it first.
            DispatchQueue.main.async {
                Self.finish(handle, onExit: onExit, sink: sink)
            }
        }
        return handle
    }

    func terminateAll() {
        // Every run is asked to stop, which is what the app delegate wants at termination: a
        // query left running on the coordinator after the window is gone is the thing this call
        // exists to prevent. The runs are dropped from the set here rather than by their own
        // completion so that a run whose thread is wedged cannot accumulate across calls.
        let handles = Self.running
        Self.running.removeAll()
        handles.forEach { $0.terminate() }
    }

    /// Everything the main queue does once the call has returned: the exit, after the events.
    ///
    /// The events are not passed through here, and that is the change streaming brought: the sink
    /// delivered each one as the engine produced it, so all that is left is the ordering rule's
    /// second half — `onExit`, exactly once, on the main queue, after the last event.
    private static func finish(_ handle: RustRun,
                               onExit: @escaping (_ status: Int32, _ stderr: String) -> Void,
                               sink: Sink) {
        running.remove(handle)

        if let message = sink.failure {
            // The message the engine wrote, in both places at once: the exit status the call
            // sites branch on, and the argument `PythonEngine` put its log in. It was already
            // delivered as the `error` event, by the sink, on the way here.
            onExit(1, message)
            return
        }
        // Status 0 and no stderr, which is what a Python run that ended cleanly reports too — and
        // what a *cancelled* run reports: stopping is a decision the user made, not a failure.
        onExit(0, "")
    }
}

/// The sink the FFI is handed: decode one line, deliver it on the main queue.
///
/// A class, because `EventSink` is one (UniFFI keeps it in a handle map and calls back into the
/// same object), and because the failure message it remembers has to outlive the calls.
final class Sink: EventSink, @unchecked Sendable {
    /// The run this sink belongs to, so a stopped run's late events are not painted into a view
    /// that has moved on.
    private let handle: RustRun
    /// Named `deliver` rather than `onEvent`: the protocol's own method is `onEvent(line:)`, and a
    /// stored property of that name is a redeclaration in the same type.
    private let deliver: (Event) -> Void

    /// Guarded because the FFI calls this from its own thread while the main queue reads it at
    /// the end of the run. `RustRun`'s own lock has the same shape and the same reason.
    private let lock = NSLock()
    private var message: String?

    init(handle: RustRun, onEvent: @escaping (Event) -> Void) {
        self.handle = handle
        self.deliver = onEvent
    }

    /// The `error` event's message, or `nil` if the run did not fail.
    ///
    /// Kept here rather than returned, because there is nowhere to return it to: the FFI call
    /// reports a failure on the same channel as everything else, which is the point of it.
    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return message
    }

    func onEvent(line: String) {
        // The decode lives in `EngineWire` because `PythonEngine` reads the same lines out of the
        // child's stdout, and the two must not disagree about what one means. A line that is not
        // an event is dropped rather than failing the run, exactly as the old engine dropped it
        // into stderr: the engine writes nothing else to this channel.
        guard let event = EngineWire.event(in: Data(line.utf8)) else { return }

        if event.event == "error" {
            lock.lock()
            message = event.message
            lock.unlock()
        }

        // Hopped rather than delivered here: the FFI calls this from the thread that is running
        // the command, and the protocol promises every delivery on the main queue. The hop is
        // unconditional, so the events arrive in the order the engine produced them — a serial
        // queue is what makes that ordering survive the hop, and what makes them all land before
        // the exit that `finish` hops to after this call returns.
        DispatchQueue.main.async { [handle, deliver] in
            guard !handle.isStopped else { return }
            deliver(event)
        }
    }
}

/// The handle a caller holds while a Rust run is in flight.
///
/// `EngineRun` promises one thing — "the handle that stops it" — and this is all of it: `terminate()`
/// asks the engine to stop, and the engine stops reading rows. It is a request rather than a kill,
/// which is worth knowing when a statement is slow to notice: a stopped `export` keeps the rows it
/// already wrote and reports them, and a stopped `preview` ends with the statement it is on.
///
/// A run is its own identity — a second run of the same command is a different run — so equality
/// is identity rather than the fields a subclass-style `Equatable` would compare.
final class RustRun: EngineRun, Hashable {
    let command: String

    private let lock = NSLock()
    /// The engine's own handle, handed over by `run` before the call starts.
    ///
    /// Readable outside this file so a test can check that a stop crossed into the engine rather
    /// than only setting a flag here — which is the difference the old version of this file could
    /// not test, and said so.
    private(set) var cancelHandle: RunCancel?
    private var stopped = false

    init(command: String) {
        self.command = command
    }

    /// The caller's cancel handle, handed over by `run` before the FFI call starts. Locked
    /// because `terminate()` is called from the main queue — including the app delegate's
    /// `applicationWillTerminate` — while `run` sets it from wherever the call was made.
    func attach(cancel: RunCancel) {
        lock.lock()
        defer { lock.unlock() }
        self.cancelHandle = cancel
    }

    /// Whether the caller has stopped waiting for this run.
    var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    func terminate() {
        lock.lock()
        let cancel = cancelHandle
        stopped = true
        lock.unlock()
        // Outside the lock: the FFI call crosses into Rust, and holding a Swift lock across it
        // would make the stop wait on whatever the run happens to be doing.
        //
        // Idempotent, because a user presses Stop and then presses it again, and it is safe
        // before the engine has opened anything and after it has finished — `RunCancel` says so.
        cancel?.requestCancel()
    }

    static func == (lhs: RustRun, rhs: RustRun) -> Bool { lhs === rhs }

    func hash(into hasher: inout Hasher) { hasher.combine(ObjectIdentifier(self)) }
}
