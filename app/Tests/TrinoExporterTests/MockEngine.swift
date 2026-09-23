import Foundation

@testable import QueryHive

/// A `DatabaseEngine` that answers from a script.
///
/// The protocol exists so that app logic can be tested without an engine (blueprint §1.6), and
/// this is the engine that makes that sentence true: no child process, no FFI, no server, no
/// environment it has to understand. It lives in the test target rather than in the app because
/// nothing in the app can be *handed* an engine yet — `Engine.current` is a `static let` — so a
/// scripted engine in the binary would be code the app can never run, and the day the app can be
/// handed one it should be handed a test's, not a shipped one.
///
/// It also carries the negative control for the contract tests: `onMainQueue: false` answers off
/// the main queue, which is the mistake those tests exist to catch. A test double that could only
/// behave correctly could not show that the check has teeth.
final class MockEngine: DatabaseEngine, @unchecked Sendable {
    /// What one command answers with.
    enum Answer {
        /// These events, in this order, then a clean exit.
        case events([Event])
        /// These events, then a non-zero exit whose reason is this text — the shape a real engine
        /// has when it starts fine and fails while running.
        case failure(events: [Event] = [], status: Int32 = 1, reason: String)
        /// Never starts: `run` returns `nil` and has already called `onExit`, which is the
        /// protocol's one failure path.
        case cannotStart(reason: String)
    }

    /// One run this engine was asked for, kept so a test can assert what the app asked *for* and
    /// not only what it did with the answer.
    struct Invocation: Equatable {
        let command: String
        let env: [String: String]
    }

    /// Whether answers are delivered on the main queue, which is what the protocol promises.
    let onMainQueue: Bool

    private let lock = NSLock()
    private var answers: [String: Answer] = [:]
    private var invocations: [Invocation] = []

    init(onMainQueue: Bool = true) {
        self.onMainQueue = onMainQueue
    }

    /// Scripts one command. A command with no answer refuses to start, so a test that forgets to
    /// script something hears about it instead of getting silence.
    func answer(_ command: String, with answer: Answer) {
        lock.lock()
        defer { lock.unlock() }
        answers[command] = answer
    }

    /// Every run this engine was asked for, in order.
    var calls: [Invocation] {
        lock.lock()
        defer { lock.unlock() }
        return invocations
    }

    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        lock.lock()
        invocations.append(Invocation(command: command, env: env))
        let answer = answers[command]
        lock.unlock()

        guard let answer else {
            onExit(-1, "MockEngine has nothing scripted for '\(command)'.")
            return nil
        }

        switch answer {
        case .cannotStart(let reason):
            // Before returning, so the caller's one failure path works the way the protocol says.
            onExit(-1, reason)
            return nil
        case .events(let events):
            deliver(events, status: 0, stderr: "", onEvent: onEvent, onExit: onExit)
        case .failure(let events, let status, let reason):
            deliver(events, status: status, stderr: reason, onEvent: onEvent, onExit: onExit)
        }
        return MockRun(command: command)
    }

    func terminateAll() {}

    private func deliver(_ events: [Event], status: Int32, stderr: String,
                         onEvent: @escaping (Event) -> Void,
                         onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) {
        let body = {
            events.forEach(onEvent)
            onExit(status, stderr)
        }
        if onMainQueue {
            DispatchQueue.main.async(execute: body)
        } else {
            DispatchQueue.global().async(execute: body)
        }
    }
}

/// The handle `MockEngine` hands back. `EngineRun` promises that callers can stop a run, so the
/// mock records that they asked to, which is the only thing a scripted engine can honour.
final class MockRun: EngineRun, @unchecked Sendable {
    let command: String
    private let lock = NSLock()
    private var wasTerminated = false

    init(command: String) {
        self.command = command
    }

    var terminated: Bool {
        lock.lock()
        defer { lock.unlock() }
        return wasTerminated
    }

    func terminate() {
        lock.lock()
        defer { lock.unlock() }
        wasTerminated = true
    }
}
