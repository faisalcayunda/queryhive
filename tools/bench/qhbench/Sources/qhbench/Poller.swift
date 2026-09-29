import Foundation

/// Polls a database server every 5 ms and records `(time, 1 | 0)`: 1 while the target statement is
/// still running on the server, 0 once it is not. One poller per server kind.
///
/// SQL servers are polled through one long-lived client process fed a query every 5 ms on stdin,
/// so each sample costs a round trip and not a process spawn.
class Poller {
    private let lock = NSLock()
    private var samples: [(t: UInt64, v: Int)] = []
    fileprivate var running = false

    func record(_ v: Int) {
        lock.lock(); samples.append((Clock.nowNs(), v)); lock.unlock()
    }

    /// Subclasses call this first, so a poller never carries samples from an earlier run.
    func resetSamples() {
        lock.lock(); samples = []; lock.unlock()
    }

    func start() throws {}
    func stop() {}

    /// True once a sample has seen the statement running.
    func waitActive(timeoutMs: Double) -> Bool {
        let deadline = Clock.nowNs() + UInt64(timeoutMs * 1e6)
        while Clock.nowNs() < deadline {
            lock.lock(); let seen = samples.contains { $0.v >= 1 }; lock.unlock()
            if seen { return true }
            sleepMs(2)
        }
        return false
    }

    /// The time of the first sample after `t0` that no longer sees the statement.
    func waitGone(after t0: UInt64, timeoutMs: Double) -> UInt64? {
        let deadline = Clock.nowNs() + UInt64(timeoutMs * 1e6)
        while Clock.nowNs() < deadline {
            lock.lock(); let hit = samples.first { $0.t >= t0 && $0.v == 0 }; lock.unlock()
            if let hit { return hit.t }
            sleepMs(1)
        }
        return nil
    }
}

struct PollerError: Error, CustomStringConvertible {
    let description: String
}

final class SQLPoller: Poller {
    private let executable: String
    private let arguments: [String]
    private let environment: [String: String]
    private let query: String
    private var proc: Process?
    private var stdin: FileHandle?

    init(executable: String, arguments: [String], environment: [String: String], query: String) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.query = query
    }

    override func start() throws {
        resetSamples()
        let p = Process()
        p.executableURL = URL(fileURLWithPath: executable)
        p.arguments = arguments
        var env = ProcessInfo.processInfo.environment
        for (k, v) in environment { env[k] = v }
        p.environment = env
        let inPipe = Pipe(), outPipe = Pipe()
        p.standardInput = inPipe
        p.standardOutput = outPipe
        p.standardError = FileHandle.nullDevice
        do { try p.run() } catch { throw PollerError(description: "cannot start \(executable): \(error)") }
        proc = p
        stdin = inPipe.fileHandleForWriting
        running = true
        let line = Data((query + "\n").utf8)
        let writer = inPipe.fileHandleForWriting
        let reader = outPipe.fileHandleForReading
        // Lock-step: send one query, wait for its reply, then sleep the rest of the 5 ms. A reply is
        // always the answer to the query sent just before it, so its timestamp is not queue lag.
        Thread.detachNewThread { [weak self] in
            var buffer = Data()
            while let self, self.running {
                let sent = Clock.nowNs()
                do { try writer.write(contentsOf: line) } catch { break }
                var value: Int?
                while value == nil {
                    if let nl = buffer.firstIndex(of: 0x0A) {
                        let text = String(data: buffer[buffer.startIndex..<nl], encoding: .utf8)?
                            .trimmingCharacters(in: .whitespaces) ?? ""
                        buffer.removeSubrange(buffer.startIndex...nl)
                        if let v = Int(text) { value = v }
                        continue
                    }
                    let d = reader.availableData
                    if d.isEmpty { return }  // client exited
                    buffer.append(d)
                }
                if let value { self.record(value) }
                let spent = Clock.ms(from: sent, to: Clock.nowNs())
                if spent < 5 { sleepMs(5 - spent) }
            }
        }
    }

    override func stop() {
        running = false
        try? stdin?.close()
        proc?.terminate()
    }
}

/// Trino: the REST API at `/v1/query`, matched by a marker comment in the statement text.
final class TrinoPoller: Poller {
    private let base: String
    private let marker: String
    private let session = URLSession(configuration: .ephemeral)

    init(base: String, marker: String) {
        self.base = base
        self.marker = marker
    }

    override func start() throws {
        resetSamples()
        guard URL(string: base + "/v1/query") != nil else { throw PollerError(description: "bad Trino URL \(base)") }
        running = true
        Thread.detachNewThread { [weak self] in
            var queryId: String?
            while let self, self.running {
                let path = queryId.map { "/v1/query/\($0)" } ?? "/v1/query"
                var req = URLRequest(url: URL(string: self.base + path)!)
                req.setValue("qhbench", forHTTPHeaderField: "X-Trino-User")
                let sem = DispatchSemaphore(value: 0)
                var body: Data?
                self.session.dataTask(with: req) { d, _, _ in body = d; sem.signal() }.resume()
                sem.wait()
                if let body, let json = try? JSONSerialization.jsonObject(with: body) {
                    let items: [[String: Any]]
                    if let arr = json as? [[String: Any]] { items = arr } else if let one = json as? [String: Any] { items = [one] } else { items = [] }
                    // Newest non-terminal statement carrying this run's marker; once its id is known
                    // only that query is asked for.
                    let live: Set<String> = ["QUEUED", "WAITING_FOR_RESOURCES", "DISPATCHING", "PLANNING", "STARTING", "RUNNING", "FINISHING"]
                    let created: ([String: Any]) -> String = { ($0["queryStats"] as? [String: Any])?["createTime"] as? String ?? "" }
                    let matches = items.filter { ($0["query"] as? String)?.contains(self.marker) == true }
                    let nonTerminal = matches.filter { live.contains($0["state"] as? String ?? "") }
                    // The id is taken only from a live match, never from a finished query: Trino keeps
                    // those for about 15 minutes.
                    let mine = queryId != nil ? matches.first : nonTerminal.max { created($0) < created($1) }
                    if let mine {
                        queryId = mine["queryId"] as? String
                        let state = mine["state"] as? String ?? ""
                        self.record(live.contains(state) ? 1 : 0)
                    }
                }
                sleepMs(5)
            }
        }
    }

    override func stop() { running = false }
}

enum PollerFactory {
    /// A poller for the scenario, or the reason none can be built.
    static func make(kind: String, matchPattern: String, marker: String) -> (Poller?, String?) {
        let env = ProcessInfo.processInfo.environment
        switch kind {
        case "pg":
            let q = "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND state = 'active' AND query ILIKE '\(matchPattern)';"
            let host = env["QHBENCH_PG_HOST"] ?? "127.0.0.1"
            let port = env["QHBENCH_PG_PORT"] ?? "55432"
            let user = env["QHBENCH_PG_USER"] ?? "qh"
            let db = env["QHBENCH_PG_DB"] ?? "qh"
            let candidates = ["/opt/homebrew/opt/libpq/bin/psql", "/opt/homebrew/bin/psql", "/usr/local/bin/psql"]
            if let pw = env["QHBENCH_PG_PASSWORD"], let psql = candidates.first(where: FileManager.default.isExecutableFile) {
                return (SQLPoller(executable: psql, arguments: ["-X", "-q", "-At", "-h", host, "-p", port, "-U", user, "-d", db],
                                  environment: ["PGPASSWORD": pw], query: q), nil)
            }
            if let podman = ["/opt/homebrew/bin/podman", "/usr/local/bin/podman"].first(where: FileManager.default.isExecutableFile) {
                return (SQLPoller(executable: podman, arguments: ["exec", "-i", "qh-postgres", "psql", "-X", "-q", "-At", "-U", user, "-d", db],
                                  environment: [:], query: q), nil)
            }
            return (nil, "no psql with QHBENCH_PG_PASSWORD and no podman found")
        case "mysql":
            guard let pw = env["QHBENCH_MYSQL_ROOT_PASSWORD"] else { return (nil, "QHBENCH_MYSQL_ROOT_PASSWORD is not set") }
            guard let podman = ["/opt/homebrew/bin/podman", "/usr/local/bin/podman"].first(where: FileManager.default.isExecutableFile)
            else { return (nil, "podman not found (no host mysql client)") }
            let q = "SELECT COUNT(*) FROM information_schema.processlist WHERE id <> CONNECTION_ID() AND info LIKE '\(matchPattern)';"
            let db = env["QHBENCH_MYSQL_DB"] ?? "qh"
            return (SQLPoller(executable: podman, arguments: ["exec", "-e", "MYSQL_PWD", "-i", "qh-mysql", "mysql", "-uroot", "-N", "-B", "-n", db],
                              environment: ["MYSQL_PWD": pw], query: q), nil)
        case "trino":
            return (TrinoPoller(base: env["QHBENCH_TRINO_URL"] ?? "http://127.0.0.1:58080", marker: marker), nil)
        default:
            return (nil, "unknown server kind \(kind)")
        }
    }
}
