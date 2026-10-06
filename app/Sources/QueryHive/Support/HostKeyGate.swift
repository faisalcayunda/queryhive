import Foundation

/// The one point where an SSH host key the engine refused reaches a person (blueprint w11
/// section 9.4, D-20).
///
/// A dozen call sites can open a tunnel (run, preview, count, explain, tree, objects, `table_op`,
/// import, apply, Test), and a prompt patched into each would be a dozen ways to miss one. They all
/// go through `Engine.current`, so this wraps it: it forwards **everything**, asks the engine for
/// the `host_key` detail on any run that names a bastion, and hands each `error` event that carries
/// one to `HostKeyCenter` without changing the event. The failed operation still shows its error
/// (the tree node, the tab's log); the sheet is an addition, not a replacement.
///
/// All seven requirements of `DatabaseEngine` are implemented here **explicitly**, and `warmUp` is
/// the one that matters: the protocol gives it an empty default in an extension, so a wrapper that
/// forgot it would compile, inherit the empty default, and silently turn warm-up off for the whole
/// app. `HostKeyGateTests` proves each one reaches the wrapped engine.
///
/// What it never does: accept a key. There is no method here that takes a fingerprint. The only
/// route to trusting one is `HostKeyCenter.trust(_:)` on a prompt built from an event, from a person
/// pressing a button.
final class HostKeyGate: DatabaseEngine, @unchecked Sendable {
    private let wrapped: any DatabaseEngine
    private let center: HostKeyCenter

    init(wrapping engine: any DatabaseEngine, center: HostKeyCenter) {
        wrapped = engine
        self.center = center
        // The trust run goes to the wrapped engine, not through this gate: a pinned attempt that
        // is itself refused is shown on the sheet that is already up, not as a second prompt.
        center.engine = engine
    }

    /// Asks for the detail on any run that opens a tunnel. The caller never has to remember.
    private static func detailed(_ env: [String: String]) -> [String: String] {
        guard let host = env["SSH_HOST"], !host.isEmpty else { return env }
        var out = env
        out["SSH_HOST_KEY_DETAIL"] = "1"
        return out
    }

    /// The event callback, with the one addition: a refused host key goes to the center, on the
    /// main queue, after the caller has seen the event.
    private func observing(_ env: [String: String],
                           _ onEvent: @escaping (Event) -> Void) -> (Event) -> Void {
        { [center] event in
            onEvent(event)
            guard event.event == "error", let detail = event.hostKey else { return }
            let report = { center.report(detail, env: env) }
            if Thread.isMainThread { report() } else { DispatchQueue.main.async(execute: report) }
        }
    }

    // MARK: DatabaseEngine, all seven

    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        wrapped.run(command, env: Self.detailed(env), onEvent: observing(env, onEvent), onExit: onExit)
    }

    /// Preview and explain run through here, and they are the operations that open a tunnel most
    /// often: a gate that observed only `run` would never prompt for them.
    @discardableResult
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void,
                      onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        wrapped.runIntoStore(command, env: Self.detailed(env), store: store,
                             onEvent: observing(env, onEvent), onExit: onExit)
    }

    func terminateAll() { wrapped.terminateAll() }

    /// No event channel, so nothing to observe: it saves a session while the app quits.
    func runBlocking(_ command: String, env: [String: String]) { wrapped.runBlocking(command, env: env) }

    func makeResultStore() throws -> StoreRows { try wrapped.makeResultStore() }

    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        try wrapped.storeFromRows(columns: columns, rows: rows)
    }

    /// Explicit, see the type's note. A warm-up never prompts: its errors are swallowed, so the
    /// detail is not asked for and nothing is reported.
    func warmUp(env: [String: String]) { wrapped.warmUp(env: env) }
}
