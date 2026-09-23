import Foundation

/// The bundled Python engine at Contents/Resources/engine/ — the Fase 1 implementation behind
/// `DatabaseEngine` (ADR-0001). Shared by the export flow and the connection editor's Test button,
/// so the launch rules live in exactly one place.
///
/// A value with no stored state: the children it has started are tracked in `running`, so any
/// instance can stop them all. It is what makes the migration a seam rather than a rewrite — the
/// calls the UI makes do not know which engine answers them.
struct PythonEngine: DatabaseEngine {
    /// The children this process has started. Private because callers get a handle per run, not the
    /// set; `terminateAll()` is the only thing the app delegate needs on quit.
    private static var running = Set<Process>()

    private struct Location {
        let python: URL
        let script: URL
        let sitePackages: URL
    }

    private static func locate() -> Location? {
        guard let resources = Bundle.main.resourceURL else { return nil }
        let root = resources.appendingPathComponent("engine")
        let python = root.appendingPathComponent("python/bin/python3")
        let script = root.appendingPathComponent("queryhive_engine.py")
        guard FileManager.default.isExecutableFile(atPath: python.path),
              FileManager.default.fileExists(atPath: script.path) else { return nil }
        return Location(python: python, script: script, sitePackages: root.appendingPathComponent("site-packages"))
    }

    /// Runs `queryhive_engine.py <command>` with `env` overriding the bundled-engine
    /// environment. Events and the exit handler arrive on the main queue. Values in `env` that
    /// look like secrets (and any `:`-split component of one that is still long enough to be
    /// sensitive) are redacted out of stderr and out of `error` event messages before either
    /// reaches the UI.
    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        guard let location = Self.locate() else {
            onExit(-1, "The bundled engine is missing from the app. Rebuild with app/build.sh.")
            return nil
        }
        // stderr goes to a file: a full stderr pipe nobody reads would block python. 0600 so
        // no other local account can read a traceback that might echo a bad value.
        let log = FileManager.default.temporaryDirectory.appendingPathComponent("queryhive-\(UUID().uuidString).log")
        FileManager.default.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600])
        let process = Process()
        let out = Pipe()
        process.executableURL = location.python
        process.arguments = ["-s", "-u", location.script.path, command]
        process.currentDirectoryURL = ConnectionStore.runDirectory
        var processEnv = ProcessInfo.processInfo.environment
        // Drop every inherited PYTHON* variable (not just PYTHONHOME) so nothing from the
        // user's shell profile leaks into the bundled interpreter's own settings.
        for key in processEnv.keys.filter({ $0.hasPrefix("PYTHON") }) {
            processEnv.removeValue(forKey: key)
        }
        processEnv["PYTHONPATH"] = location.sitePackages.path
        processEnv["PYTHONNOUSERSITE"] = "1"
        processEnv["PYTHONDONTWRITEBYTECODE"] = "1"
        processEnv.merge(env) { _, new in new }
        process.environment = processEnv
        process.standardOutput = out
        do {
            process.standardError = try FileHandle(forWritingTo: log)
            try process.run()
        } catch {
            try? FileManager.default.removeItem(at: log)
            onExit(-1, "Could not start the bundled engine: \(error.localizedDescription)")
            return nil
        }
        Self.running.insert(process)

        // Values that must never reach the UI, drawn from the env this run got. A secret that
        // is itself a compound value (e.g. "id:secret") also redacts each ':'-split piece long
        // enough to be sensitive on its own, in case only a fragment of it gets echoed back.
        let markers = ["CREDENTIAL", "SECRET", "TOKEN", "PASSWORD", "KEY"]
        let secretValues = env.filter { key, value in
            value.count >= 6 && markers.contains { key.uppercased().contains($0) }
        }.values
        var redactionSet = Set<String>()
        for secret in secretValues where !secret.isEmpty {
            redactionSet.insert(secret)
            for part in secret.split(separator: ":") where part.count >= 6 {
                redactionSet.insert(String(part))
            }
        }
        // Longest first, so a shorter piece can't mask a match that would have redacted more.
        let redactions = redactionSet.sorted { $0.count > $1.count }
        func redact(_ text: String) -> String {
            var result = text
            for value in redactions { result = result.replacingOccurrences(of: value, with: "••••") }
            return result
        }

        DispatchQueue.global().async {
            var buffer = Data()
            var unparsed: [String] = []
            func tryDecode(_ data: Data) {
                // The decode itself lives in `EngineWire` because `RustEngine` reads the same
                // lines out of the FFI's return value, and the two must not disagree about what
                // one means.
                if var event = EngineWire.event(in: data) {
                    if let message = event.message { event.message = redact(message) }
                    DispatchQueue.main.async { onEvent(event) }
                } else {
                    unparsed.append(String(decoding: data, as: UTF8.self))
                }
            }
            while true {
                let chunk = out.fileHandleForReading.availableData
                if chunk.isEmpty { break }
                buffer.append(chunk)
                while let newline = buffer.firstIndex(of: 10) {
                    let line = buffer[..<newline]
                    buffer.removeSubrange(...newline)
                    tryDecode(line)
                }
            }
            if !buffer.isEmpty, !String(decoding: buffer, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                tryDecode(buffer)
            }
            process.waitUntilExit()
            var stderr = String(decoding: (try? Data(contentsOf: log))?.suffix(16_000) ?? Data(), as: UTF8.self)
            try? FileManager.default.removeItem(at: log)
            if !unparsed.isEmpty {
                stderr += "\n\nUnparsed stdout:\n" + unparsed.joined(separator: "\n")
            }
            stderr = redact(stderr)
            DispatchQueue.main.async {
                Self.running.remove(process)
                onExit(process.terminationStatus, stderr)
            }
        }
        return process
    }

    func terminateAll() {
        Self.running.forEach { $0.terminate() }
    }
}
