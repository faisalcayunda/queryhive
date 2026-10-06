import AppKit
import Darwin
import QuartzCore
import SwiftUI

/// `--bench <scenario>` drives the grid and the editor without a person, prints one JSON object
/// per sample on stdout and exits:
///
///     QueryHive.app/Contents/MacOS/QueryHive --bench scroll-30x1m --bench-repeat 5 \
///         | python3 deploy/dev/bench_app.py --source app --axis 4
///
/// The shape of each line is the input contract in `deploy/dev/bench_app.py`'s docstring. Stdout
/// carries nothing else; progress and failures go to stderr.
///
/// Synthetic scenarios need no database. A result is put into the tab the way a finished Run puts
/// one (a Rust store from `store_from_rows`, built before the clock starts, then `QueryTab.preview`),
/// so the grid, its layout and its paint are the real ones. The DB-backed scenarios run the real `AppModel.preview` against the server named by
/// `QH_BENCH_KIND` / `_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_DB`.
///
/// Everything runs against a throwaway support directory (`ConnectionStore.root`, which
/// `AppModel.localEnvironment` turns into `DB_PATH` too), and the password is handed to the model
/// directly, so no run reads or writes the user's connections, history or Keychain.
///
/// Measurement notes, so a number is not read for more than it is:
/// - `render_ms`, `ttfr_ms`: to the first grid row on screen. The end is the main-queue turn after
///   the row's `onAppear`, after a `CATransaction.flush()`. A frame the GPU has not shown yet is
///   not included.
/// - frame times: the interval between `CADisplayLink` timestamps. A frame whose main-thread work
///   overran shows up as a multiple of the refresh interval; `hitch_ms_per_s` is the overrun,
///   summed and divided by the seconds scrolled.
/// - `keystroke_main_p99_ms`: key-down (here, just before `insertText`) to the end of the run-loop
///   turn that laid out and drew the change. Main-thread work, not vsync wait.
/// - `launch_ms`: kernel process-start to the first window update after launch, plus one turn.
enum BenchMode {
    static let synthetic = ["scroll-30x1m", "scroll-500x10k", "open-500x10k", "type-10k", "type-2m", "type-coloured-195k",
                            "tabs-100", "tabs-100-held", "launch-warm", "launch-cold"]
    static let database = ["ttfr-s1-1k", "ttfr-s1-10k", "rows-wide-500k", "mem-500k", "rows-mysql-500k", "rows-trino-500k", "cancel-pg-sleep"]
    /// The names development-plan.md uses, mapped to the ones the report grades.
    static let aliases = ["scroll-1m": "scroll-30x1m", "scroll-500c": "scroll-500x10k",
                          "open-500c": "open-500x10k", "ttfr-pg": "ttfr-s1-1k", "launch": "launch-warm"]

    private static func value(of flag: String) -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    /// `--bench a,b,c`, aliases resolved. Nil when the flag has no value.
    static func requestedScenarios() -> [String]? {
        guard let raw = value(of: "--bench"), !raw.hasPrefix("--") else { return nil }
        return raw.split(separator: ",").map { aliases[String($0)] ?? String($0) }
    }

    static func requestedRepeat() -> Int { max(1, value(of: "--bench-repeat").flatMap(Int.init) ?? 1) }

    /// Seconds one scroll pass lasts. `QH_BENCH_SCROLL_SECONDS` shortens it for a smoke test.
    private static let scrollSeconds = Double(ProcessInfo.processInfo.environment["QH_BENCH_SCROLL_SECONDS"] ?? "") ?? 3

    // MARK: Entry

    private static var root: URL?
    private static var label = "bench"
    private static var signalSources: [DispatchSourceSignal] = []
    private static let delegate = BenchDelegate()

    /// A quit request (from a script, another harness, the Dock) does not end a measurement: it is
    /// logged with whoever sent it, and refused.
    static func refuseTermination() -> NSApplication.TerminateReply {
        let sender = NSAppleEventManager.shared().currentAppleEvent?
            .attributeDescriptor(forKeyword: AEKeyword(keySenderPIDAttr))?.int32Value
        log("ignored a quit request from pid \(sender.map(String.init) ?? "unknown")")
        return .terminateCancel
    }

    /// Runs on every way out of the process: a run that produced no line says so, and the throwaway
    /// support directory goes.
    fileprivate static func atExit() {
        outputLock.lock()
        let silent = written == 0
        outputLock.unlock()
        if silent { emitStatus(label, "[belum diukur]", "the process exited without a sample") }
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    /// Bench-only values for the preferences the run would otherwise read from the user's domain,
    /// so a machine's own settings cannot move a number. Volatile: nothing is written.
    private static func pinPreferences() {
        UserDefaults.standard.setVolatileDomain([
            "ApplePersistenceIgnoreState": true,
            "statementTimeoutMS": 60_000,
            "defaultRowLimit": 1_000,
            "recordsHistory": false,
            FilterPresetStore.defaultsKey: Data(),
        ], forName: UserDefaults.argumentDomain)
    }

    /// The store budget for this process. The host takes one configuration for its whole life, so a
    /// `--bench` run names the scenarios that share it: the tab scenarios need a budget small enough
    /// that their stores really spill, the database ones run on the product's own 256 MiB with spill,
    /// and the synthetic grid scenarios get 2 GiB with no spill (comparable with Fase 0, where the
    /// rows were an array in the heap). Run `scroll-30x1m` and `tabs-*` in separate processes.
    private static func configureStores(for scenarios: [String], support: URL) {
        let spill = support.appendingPathComponent("spill").path
        if scenarios.contains(where: { $0.hasPrefix("tabs-") }) {
            RustEngine.ensureStoresConfigured(spillDir: spill, budgetBytes: 2 << 20)
        } else if scenarios.contains(where: { database.contains($0) }) {
            RustEngine.ensureStoresConfigured(spillDir: spill, budgetBytes: 256 << 20)
        } else {
            RustEngine.ensureStoresConfigured(spillDir: nil, budgetBytes: 2 << 30)
        }
    }

    /// Returns only in a `launch-warm` child, so the real app can start and report its first frame.
    @MainActor
    static func run() {
        guard let scenarios = requestedScenarios(), !scenarios.isEmpty else {
            log("--bench needs a scenario. Known: \((synthetic + database).joined(separator: ", "))")
            exit(64)
        }
        if let unknown = scenarios.first(where: { !synthetic.contains($0) && !database.contains($0) }) {
            log("unknown scenario '\(unknown)'. Known: \((synthetic + database).joined(separator: ", "))")
            exit(64)
        }
        label = scenarios.joined(separator: ",")
        let repeats = requestedRepeat()
        let child = ProcessInfo.processInfo.environment["QH_BENCH_CHILD"] != nil

        signal(SIGPIPE, SIG_IGN)
        for number in [SIGTERM, SIGINT, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .global())
            source.setEventHandler {
                emitStatus(label, "[belum diukur]", "stopped by signal \(number)")
                exit(128 + number)
            }
            source.resume()
            signalSources.append(source)
        }
        atexit { BenchMode.atExit() }

        let support = child
            ? URL(fileURLWithPath: ProcessInfo.processInfo.environment["QH_BENCH_ROOT"] ?? NSTemporaryDirectory())
            : FileManager.default.temporaryDirectory.appendingPathComponent("qh-bench-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
        if !child { root = support }
        ConnectionStore.root = support
        pinPreferences()
        configureStores(for: scenarios, support: support)
        PerfSignposts.recording = true
        watchdog(seconds: Double(ProcessInfo.processInfo.environment["QH_BENCH_TIMEOUT"] ?? "") ?? 600)

        if child {
            PerfSignposts.onLaunchFrame = { ms in
                emit("launch-warm", ["launch_ms": ms])
                exit(0)
            }
            return
        }

        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        app.delegate = delegate
        Task { @MainActor in
            for scenario in scenarios {
                log("start \(scenario) x\(repeats)")
                await execute(scenario, repeats: repeats)
            }
            log("end \(label), \(written) line(s)")
            exit(0)
        }
        app.run()
        log("NSApplication.run returned")
        emitStatus(label, "[belum diukur]", "the run loop ended before the scenario did")
        exit(70)
    }

    /// One real launch per sample, in a child process whose home directory is the throwaway one, so
    /// its preferences, Application Support and saved window state are empty and land nowhere the
    /// owner keeps anything.
    @MainActor
    private static func launchWarm(repeats: Int) async {
        for _ in 0..<repeats {
            let home = FileManager.default.temporaryDirectory.appendingPathComponent("qh-bench-home-\(UUID().uuidString)")
            try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: home) }
            var environment = ProcessInfo.processInfo.environment
            environment["QH_BENCH_CHILD"] = "1"
            environment["QH_BENCH_ROOT"] = home.appendingPathComponent("support").path
            environment["CFFIXED_USER_HOME"] = home.path
            let child = Process()
            child.executableURL = URL(fileURLWithPath: Bundle.main.executablePath ?? CommandLine.arguments[0])
            child.arguments = ["--bench", "launch-warm"]
            child.environment = environment
            let pipe = Pipe()
            child.standardOutput = pipe
            do { try child.run() } catch {
                emitStatus("launch-warm", "[belum diukur]", "cannot start a child: \(error)")
                continue
            }
            let data = await Task.detached { pipe.fileHandleForReading.readDataToEndOfFile() }.value
            child.waitUntilExit()
            let lines = String(decoding: data, as: UTF8.self).split(separator: "\n").filter { $0.hasPrefix("{") }
            if lines.isEmpty {
                emitStatus("launch-warm", "[belum diukur]", "the child exited with status \(child.terminationStatus) and no sample")
            }
            lines.forEach { writeRaw(String($0)) }
        }
    }

    /// A hung main thread cannot report itself, so this runs off it. A scenario that outlives it
    /// is a result too: the grid did not finish within the budget.
    private static func watchdog(seconds: Double) {
        DispatchQueue.global().asyncAfter(deadline: .now() + seconds) {
            emitStatus(label, "[belum diukur]", "timed out after \(Int(seconds)) s")
            exit(3)
        }
    }

    @MainActor
    private static func execute(_ scenario: String, repeats: Int) async {
        switch scenario {
        case "scroll-30x1m": await scroll(scenario, rows: 1_000_000, columns: 30, both: false, repeats: repeats)
        case "scroll-500x10k": await scroll(scenario, rows: 10_000, columns: 500, both: true, repeats: repeats)
        case "open-500x10k": await open(scenario, rows: 10_000, columns: 500, repeats: repeats)
        case "type-10k": await type(scenario, lines: 10_000, minimumCharacters: 0, repeats: repeats)
        case "type-2m": await type(scenario, lines: 0, minimumCharacters: 2_000_000, repeats: repeats)
        // Under the analysis ceiling, so the document is fully coloured and a keystroke repaints;
        // typed text is SQL with keywords, and every 20th key is followed by a pause longer than the
        // editor's debounces, so the analysis and the model write run between keystrokes.
        case "type-coloured-195k": await type(scenario, lines: 0, minimumCharacters: 194_000, repeats: repeats, coloured: true)
        case "tabs-100": await tabs(scenario, repeats: repeats)
        case "tabs-100-held": await tabsHeld(scenario, repeats: repeats)
        case "launch-warm": await launchWarm(repeats: repeats)
        case "launch-cold": emitStatus(scenario, "tidak diukur (butuh sudo)", "a cold start needs `purge`, which needs sudo")
        default: await databaseScenario(scenario, repeats: repeats)
        }
    }

    // MARK: Output

    private static func log(_ message: String) {
        FileHandle.standardError.write(Data("bench: \(message)\n".utf8))
    }

    /// Lines written so far, and the lock that keeps the counter and stdout coherent: the watchdog
    /// and the signal handlers write from other threads.
    private static var written = 0
    private static let outputLock = NSLock()

    private static func writeRaw(_ line: String) {
        outputLock.lock()
        written += 1
        print(line)
        fflush(stdout)
        outputLock.unlock()
    }

    private static func write(_ object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let line = String(data: data, encoding: .utf8) else { return }
        writeRaw(line)
    }

    static func emit(_ scenario: String, _ metrics: [String: Double], extra: [String: Any] = [:]) {
        var object = extra
        object["scenario"] = scenario
        object["app"] = "queryhive"
        object["metrics"] = metrics.filter(\.value.isFinite).mapValues { ($0 * 1000).rounded() / 1000 }
        write(object)
    }

    static func emitStatus(_ scenario: String, _ status: String, _ notes: String) {
        write(["scenario": scenario, "app": "queryhive", "status": status, "notes": notes])
    }

    private static func percentile(_ values: [Double], _ p: Double) -> Double {
        guard !values.isEmpty else { return .nan }
        let sorted = values.sorted()
        return sorted[min(sorted.count - 1, max(0, Int((Double(sorted.count) * p).rounded(.up)) - 1))]
    }

    // MARK: Window and waiting

    @MainActor
    private static func openWindow(_ model: AppModel) -> NSWindow {
        // Pinned even with nothing requested, as `--snapshot` does: a benchmark must not write the
        // user's appearance preferences.
        ThemeStore.shared.pin()
        let scheme = ThemeStore.shared.mode.colorScheme
        let size = NSSize(width: 1320, height: 880)
        let hosting = NSHostingView(rootView: RootView()
            .environment(model)
            .frame(width: size.width, height: size.height)
            .preferredColorScheme(scheme))
        hosting.frame = NSRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: hosting.frame, styleMask: [.titled, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.contentView = hosting
        if let screen = NSScreen.main {
            window.setFrameOrigin(NSPoint(x: screen.visibleFrame.minX + 40,
                                          y: screen.visibleFrame.maxY - size.height - 40))
        }
        window.orderFrontRegardless()
        return window
    }

    @MainActor
    private static func sleep(_ seconds: Double) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    @MainActor
    private static func wait(timeout: Double, _ condition: () -> Bool) async -> Bool {
        let deadline = CFAbsoluteTimeGetCurrent() + timeout
        while !condition() {
            if CFAbsoluteTimeGetCurrent() > deadline { return false }
            await sleep(0.002)
        }
        return true
    }

    private static func descendants(of view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap(descendants(of:))
    }

    // MARK: Fixtures

    private static let columnTypes = ["varchar", "bigint", "double", "timestamp(6)"]

    /// Columns and the rows to put under them, before they are in a store.
    private struct Fixture {
        var columns: [Event.Column]
        var rows: [[String?]]
    }

    /// Rows of short strings and a NULL now and then. Built outside any timing.
    private static func syntheticResult(rows: Int, columns: Int) -> Fixture {
        let header = (0..<columns).map { Event.Column(name: "col_\($0)", type: columnTypes[$0 % 4]) }
        var data = [[String?]]()
        data.reserveCapacity(rows)
        for r in 0..<rows {
            var row = [String?]()
            row.reserveCapacity(columns)
            for c in 0..<columns {
                switch c % 4 {
                case 0: row.append(String(r &+ c))
                case 1: row.append(c % 7 == 1 && r % 5 == 0 ? nil : "name \(r % 977)")
                case 2: row.append(String(Double(r) * 0.37 + Double(c)))
                default: row.append("2026-07-25 15:30:06")
                }
            }
            data.append(row)
        }
        return Fixture(columns: header, rows: data)
    }

    /// A model with one idle tab and the result panel open, and a window showing it.
    @MainActor
    private static func gridWindow() async -> (AppModel, QueryTab, NSWindow)? {
        let model = AppModel()
        guard let tab = model.selectedTab else { return nil }
        tab.stage = .done
        tab.panel = .result
        model.panelCollapsed = false
        let window = openWindow(model)
        await sleep(1.0)
        return (model, tab, window)
    }

    /// Puts a result into the tab the way a finished preview does, and returns the milliseconds to
    /// the first grid row on screen, or nil when it never came.
    @MainActor
    private static func show(_ result: Fixture, in tab: QueryTab, timeout: Double = 300) async -> Double? {
        // The store is built before the clock starts: what is timed is the grid taking a finished
        // result, as it does after a Run, not the ingest (blueprint §17.4, R-38).
        tab.activeResult?.release()
        tab.activeResult = nil
        let ingestStarted = CFAbsoluteTimeGetCurrent()
        let store: StoreRows
        do { store = try Engine.current.storeFromRows(columns: result.columns, rows: result.rows) } catch {
            log("store_from_rows failed: \(error)")
            return nil
        }
        log(String(format: "store_from_rows: %d rows x %d columns in %.1f s",
                   result.rows.count, result.columns.count, CFAbsoluteTimeGetCurrent() - ingestStarted))
        tab.columns = result.columns
        tab.previewedSQL = tab.sql
        PerfSignposts.runBegin()
        let started = CFAbsoluteTimeGetCurrent()
        tab.activeResult = store
        tab.fetchedRows = store.fetched
        tab.preview = PreviewResult(columns: result.columns, rowCount: store.fetched, truncated: false,
                                    queryID: nil, elapsedMS: 0)
        guard await wait(timeout: timeout, { PerfSignposts.time(of: "firstPaint") != nil }),
              let painted = PerfSignposts.time(of: "firstPaint") else { return nil }
        return (painted - started) * 1000
    }

    // MARK: Scroll

    private final class ScrollDriver: NSObject {
        private let scrollView: NSScrollView
        private let seconds: Double
        private let speed: CGPoint
        private var link: CADisplayLink?
        private var continuation: CheckedContinuation<Void, Never>?
        private var timer: DispatchSourceTimer?
        private let pendingLock = NSLock()
        private var pending = false
        /// "displaylink", or "timer" when the display never ticked (screen locked, window occluded).
        private(set) var source = "displaylink"
        private var start: CFTimeInterval = 0
        private var last: CFTimeInterval = 0
        private(set) var deltas: [Double] = []

        init(scrollView: NSScrollView, seconds: Double, speed: CGPoint) {
            self.scrollView = scrollView
            self.seconds = seconds
            self.speed = speed
        }

        @MainActor
        func run(in view: NSView, hz: Int) async {
            await withCheckedContinuation { continuation in
                self.continuation = continuation
                let link = view.displayLink(target: self, selector: #selector(tick(_:)))
                let rate = Float(hz)
                link.preferredFrameRateRange = CAFrameRateRange(minimum: rate, maximum: rate, preferred: rate)
                link.add(to: .main, forMode: .common)
                self.link = link
                // A window that is not being displayed never ticks. Fall back to a timer at the same
                // rate, which still measures the main thread's work per frame but not vsync.
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                    guard let self, self.start == 0 else { return }
                    self.link?.invalidate()
                    self.link = nil
                    self.source = "timer"
                    self.startTimer(hz: hz)
                }
            }
        }

        /// One main-queue hop in flight at a time: a stalled main thread must show up as one long
        /// frame, not as a burst of queued ticks that run back to back.
        private func startTimer(hz: Int) {
            let timer = DispatchSource.makeTimerSource(queue: .global(qos: .userInteractive))
            timer.schedule(deadline: .now(), repeating: .nanoseconds(1_000_000_000 / max(1, hz)))
            timer.setEventHandler { [weak self] in
                guard let self else { return }
                self.pendingLock.lock()
                let busy = self.pending
                self.pending = true
                self.pendingLock.unlock()
                if busy { return }
                DispatchQueue.main.async {
                    self.handle(CACurrentMediaTime())
                    self.pendingLock.lock()
                    self.pending = false
                    self.pendingLock.unlock()
                }
            }
            timer.resume()
            self.timer = timer
        }

        private func finish() {
            timer?.cancel()
            timer = nil
            link?.invalidate()
            link = nil
            continuation?.resume()
            continuation = nil
        }

        /// Position is a function of elapsed time, not of frame count, so a slow frame does not slow
        /// the scroll down and hide itself. It bounces between the two ends.
        private func position(_ velocity: CGFloat, elapsed: Double, extent: CGFloat) -> CGFloat {
            guard extent > 0 else { return 0 }
            let travelled = (velocity * elapsed).truncatingRemainder(dividingBy: 2 * extent)
            return travelled < extent ? travelled : 2 * extent - travelled
        }

        @objc private func tick(_ link: CADisplayLink) { handle(link.timestamp) }

        private func handle(_ now: CFTimeInterval) {
            guard continuation != nil else { return }
            if start == 0 { start = now; last = now; return }
            deltas.append((now - last) * 1000)
            last = now
            let elapsed = now - start
            if elapsed >= seconds { finish(); return }
            guard let document = scrollView.documentView else { return }
            let clip = scrollView.contentView
            let target = NSPoint(
                x: position(speed.x, elapsed: elapsed, extent: document.frame.width - clip.bounds.width),
                y: position(speed.y, elapsed: elapsed, extent: document.frame.height - clip.bounds.height))
            clip.scroll(to: target)
            scrollView.reflectScrolledClipView(clip)
        }
    }

    private static func refreshRate(of window: NSWindow) -> Int {
        (window.screen ?? NSScreen.main)?.maximumFramesPerSecond ?? 60
    }

    @MainActor
    private static func scroll(_ scenario: String, rows: Int, columns: Int, both: Bool, repeats: Int) async {
        guard let (_, tab, window) = await gridWindow() else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        let result = syntheticResult(rows: rows, columns: columns)
        guard let openMS = await show(result, in: tab) else {
            return emitStatus(scenario, "[belum diukur]", "the grid drew no row within the time limit")
        }
        await sleep(1.0)
        guard let content = window.contentView,
              let scrollView = descendants(of: content).compactMap({ $0 as? NSScrollView })
                .max(by: { ($0.documentView?.frame.height ?? 0) * ($0.documentView?.frame.width ?? 0)
                            < ($1.documentView?.frame.height ?? 0) * ($1.documentView?.frame.width ?? 0) }),
              (scrollView.documentView?.frame.height ?? 0) > scrollView.contentView.bounds.height else {
            return emitStatus(scenario, "[belum diukur]", "no scrollable NSScrollView behind the grid")
        }
        let hz = refreshRate(of: window)
        let sampler = FootprintSampler()
        for pass in 0..<repeats {
            sampler.start()
            let driver = ScrollDriver(scrollView: scrollView, seconds: scrollSeconds,
                                      speed: CGPoint(x: both ? 1500 : 0, y: 3000))
            await driver.run(in: content, hz: hz)
            let peak = sampler.stop()
            guard driver.deltas.count > 1 else {
                emitStatus(scenario, "[belum diukur]", "no frame was measured")
                continue
            }
            let budget = 1000 / Double(hz)
            let hitch = driver.deltas.reduce(0) { $0 + max(0, $1 - budget) } / scrollSeconds
            var metrics = ["frame_p50_ms": percentile(driver.deltas, 0.5),
                           "frame_p99_ms": percentile(driver.deltas, 0.99),
                           "hitch_ms_per_s": hitch,
                           "frames_count": Double(driver.deltas.count),
                           "peak_footprint_bytes": Double(peak)]
            if pass == 0 { metrics["render_ms"] = openMS }
            var extra: [String: Any] = ["display_hz": hz]
            if driver.source != "displaylink" {
                extra["notes"] = "frame source: \(driver.source); the display link never ticked (screen locked or window occluded), so vsync is not part of these frame times"
            }
            emit(scenario, metrics, extra: extra)
        }
    }

    // MARK: Open

    @MainActor
    private static func open(_ scenario: String, rows: Int, columns: Int, repeats: Int) async {
        guard let (_, tab, _) = await gridWindow() else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        let result = syntheticResult(rows: rows, columns: columns)
        for _ in 0..<repeats {
            tab.preview = nil
            tab.activeResult?.release()
            tab.activeResult = nil
            await sleep(0.5)
            guard let ms = await show(result, in: tab) else {
                return emitStatus(scenario, "[belum diukur]", "the grid drew no row within the time limit")
            }
            emit(scenario, ["render_ms": ms])
        }
    }

    // MARK: Type

    private static func sqlDocument(lines: Int, minimumCharacters: Int) -> String {
        var text = ""
        var index = 0
        while index < lines || text.utf16.count < minimumCharacters {
            text += "SELECT id, name, amount FROM public.table_\(index) WHERE id = \(index) AND status = 'open' ORDER BY id;\n"
            index += 1
        }
        return text
    }

    @MainActor
    private static func type(_ scenario: String, lines: Int, minimumCharacters: Int, repeats: Int,
                             coloured: Bool = false) async {
        let model = AppModel()
        guard let tab = model.selectedTab else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        var document = sqlDocument(lines: lines, minimumCharacters: minimumCharacters)
        // The analysis colours nothing past its ceiling; a document longer than that would bench
        // the uncoloured path under a coloured scenario's name.
        let ceiling = (try? editorCeiling()) ?? 2_000_000
        if (document as NSString).length > ceiling {
            document = (document as NSString).substring(to: ceiling)
        }
        let length = (document as NSString).length
        let window = openWindow(model)
        await sleep(1.0)
        tab.sql = document
        var found: SQLTextView?
        let loaded = await wait(timeout: 300) {
            found = window.contentView.flatMap { descendants(of: $0).compactMap { $0 as? SQLTextView }.first }
            return found.map { ($0.string as NSString).length == length } ?? false
        }
        guard loaded, let textView = found else {
            return emitStatus(scenario, "[belum diukur]", "the editor did not take the document")
        }
        window.makeFirstResponder(textView)
        let middle = (document as NSString).lineRange(for: NSRange(location: length / 2, length: 0)).location
        textView.setSelectedRange(NSRange(location: middle, length: 0))
        textView.scrollRangeToVisible(NSRange(location: middle, length: 0))
        await sleep(1.0)

        // Coloured typing is realistic SQL: short lines and one statement per `;`, so each repeat
        // measures the same shape instead of one line that grows by 200 characters a repeat.
        let text = Array(coloured ? "select id, name\nfrom t\nwhere x = 1 and y in (2);\n" : "x_foo bar_1 baz ")
        for _ in 0..<repeats {
            var insert = [Double](), didChange = [Double](), keystroke = [Double](), apply = [Double]()
            for i in 0..<200 {
                PerfSignposts.keystrokeReset()
                PerfSignposts.applyReset()
                let started = CFAbsoluteTimeGetCurrent()
                PerfSignposts.keystrokeBegin()
                textView.insertText(String(text[i % text.count]),
                                    replacementRange: NSRange(location: NSNotFound, length: 0))
                insert.append((CFAbsoluteTimeGetCurrent() - started) * 1000)
                // The repaint, not a sleep: the next key waits for this one's colours to display.
                _ = await wait(timeout: 10) { PerfSignposts.lastApplyMS != nil }
                if coloured, i % 20 == 19 { await sleep(0.2) }
                if let ms = PerfSignposts.lastDidChangeMS { didChange.append(ms) }
                if let ms = PerfSignposts.lastKeystrokeMS { keystroke.append(ms) }
                if let ms = PerfSignposts.lastApplyMS { apply.append(ms) }
            }
            let mode = textView.textLayoutManager != nil ? 2 : 1
            emit(scenario, ["keystroke_main_p50_ms": percentile(keystroke, 0.5),
                            "keystroke_main_p99_ms": percentile(keystroke, 0.99),
                            "apply_main_p50_ms": percentile(apply, 0.5),
                            "apply_main_p99_ms": percentile(apply, 0.99),
                            "did_change_p99_ms": percentile(didChange, 0.99),
                            "insert_text_p99_ms": percentile(insert, 0.99),
                            "characters_count": Double(length),
                            "keystroke_samples_count": Double(keystroke.count)],
                 extra: ["textkit": mode, "display_hz": refreshRate(of: window)])
        }
    }

    // MARK: Tabs

    private final class Weak { weak var tab: QueryTab? }

    @MainActor
    private static func tabs(_ scenario: String, repeats: Int) async {
        guard let (model, _, _) = await gridWindow() else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        let sampler = FootprintSampler()
        let fixture = syntheticResult(rows: 2_000, columns: 10)
        for _ in 0..<repeats {
            var boxes = [Weak]()
            sampler.start()
            let before = FootprintSampler.current()
            for _ in 0..<100 {
                model.newTab()
                guard let tab = model.selectedTab else { break }
                let box = Weak()
                box.tab = tab
                boxes.append(box)
                tab.stage = .done
                tab.panel = .result
                model.panelCollapsed = false
                tab.columns = fixture.columns
                tab.showRows(columns: fixture.columns, rows: fixture.rows)
                await sleep(0.03)
                model.closeSelectedTab()
                await sleep(0.01)
            }
            await sleep(1.0)
            let peak = sampler.stop()
            let after = FootprintSampler.current()
            // The engine's own count of what is still alive: zero stores and zero spilled bytes once
            // every tab is closed (§19, TM-9). Anything else is a store a tab forgot to release.
            let stats = RustEngine.storeStats()
            emit(scenario, ["leak_count": Double(boxes.filter { $0.tab != nil }.count),
                            "stores_alive_count": Double(stats?.stores ?? 0),
                            "spilled_bytes_alive": Double(stats?.spilledBytes ?? 0),
                            "footprint_delta_bytes": Double(Int64(after) - Int64(before)),
                            "peak_footprint_delta_bytes": Double(Int64(peak) - Int64(before))])
        }
    }

    /// The process's open file descriptors, from `proc_pidinfo(PROC_PIDLISTFDS)`. The size-only call
    /// (nil buffer) reports the fd table's capacity, which grows and never shrinks, so it cannot see
    /// a descriptor close; list into a buffer and count what comes back.
    private static func openFileDescriptors() -> Int {
        let stride = MemoryLayout<proc_fdinfo>.stride
        let capacity = Int(proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, nil, 0)) / stride + 64
        var fds = [proc_fdinfo](repeating: proc_fdinfo(), count: capacity)
        let bytes = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, &fds, Int32(capacity * stride))
        return bytes > 0 ? Int(bytes) / stride : 0
    }

    /// R-19 (§17.6, §19): 100 tabs open at once, each holding a store that has spilled, so each holds
    /// a descriptor. Records the descriptor count before, at the peak, and after every tab is closed,
    /// which must come back to where it started.
    @MainActor
    private static func tabsHeld(_ scenario: String, repeats: Int) async {
        guard let (model, first, _) = await gridWindow() else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        let fixture = syntheticResult(rows: 2_000, columns: 10)
        for _ in 0..<repeats {
            let before = openFileDescriptors()
            var opened = [QueryTab]()
            for _ in 0..<100 {
                model.newTab()
                guard let tab = model.selectedTab else { break }
                tab.stage = .done
                tab.panel = .result
                tab.columns = fixture.columns
                tab.showRows(columns: fixture.columns, rows: fixture.rows)
                opened.append(tab)
            }
            await sleep(0.5)
            let peak = openFileDescriptors()
            let held = RustEngine.storeStats()
            for tab in opened { model.closeTab(tab.id) }
            if model.tabs.isEmpty { model.newTab() }
            await sleep(1.0)
            let after = openFileDescriptors()
            let stats = RustEngine.storeStats()
            _ = first
            emit(scenario, ["open_fds_before_count": Double(before),
                            "open_fds_peak_count": Double(peak),
                            "open_fds_after_count": Double(after),
                            "stores_held_count": Double(held?.stores ?? 0),
                            "spilled_bytes_held": Double(held?.spilledBytes ?? 0),
                            "stores_alive_count": Double(stats?.stores ?? 0),
                            "spilled_bytes_alive": Double(stats?.spilledBytes ?? 0)])
        }
    }

    // MARK: Database

    private static func connectionFromEnvironment() -> (Connection, String)? {
        let env = ProcessInfo.processInfo.environment
        guard let host = env["QH_BENCH_HOST"], let port = env["QH_BENCH_PORT"].flatMap(Int.init),
              let user = env["QH_BENCH_USER"], let database = env["QH_BENCH_DB"] else { return nil }
        let kind = ConnectionKind(rawValue: env["QH_BENCH_KIND"] ?? "postgres") ?? .postgres
        let connection = Connection(id: UUID(), name: "bench", color: .blue, kind: kind, host: host, port: port,
                                    scheme: kind == .trino ? "http" : "", sslmode: kind == .postgres ? "disable" : "",
                                    user: user, database: database, schema: kind == .postgres ? "public" : "",
                                    verify: false)
        return (connection, env["QH_BENCH_PASSWORD"] ?? "")
    }

    /// `rows` rows of `columns` columns, generated by the server. Postgres and Trino only, and
    /// Trino's `sequence` is capped at 10,000 entries.
    private static func generatedSQL(_ kind: ConnectionKind, rows: Int, columns: Int) -> String? {
        switch kind {
        case .postgres:
            let list = (0..<columns).map { i -> String in
                switch i % 3 {
                case 0: return "g + \(i) AS c\(i)"
                case 1: return "md5((g + \(i))::text) AS c\(i)"
                default: return "(g * 0.37 + \(i))::float8 AS c\(i)"
                }
            }
            return "SELECT \(list.joined(separator: ", ")) FROM generate_series(1, \(rows)) AS g"
        case .trino:
            guard rows <= 10_000 else { return nil }
            let list = (0..<columns).map { i -> String in
                switch i % 3 {
                case 0: return "g + \(i) AS c\(i)"
                case 1: return "to_hex(md5(to_utf8(cast(g + \(i) as varchar)))) AS c\(i)"
                default: return "cast(g as double) * 0.37 + \(i) AS c\(i)"
                }
            }
            return "SELECT \(list.joined(separator: ", ")) FROM UNNEST(sequence(1, \(rows))) AS t(g)"
        case .mysql:
            return nil
        }
    }

    @MainActor
    private static func databaseScenario(_ scenario: String, repeats: Int) async {
        guard let (connection, password) = connectionFromEnvironment() else {
            return emitStatus(scenario, "[belum diukur]", "QH_BENCH_HOST, _PORT, _USER and _DB are not all set")
        }
        AppModel.benchPassword = password
        let model = AppModel()
        model.connections = [connection]
        model.rebuildTree()
        guard let tab = model.selectedTab else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        tab.connectionID = connection.id
        model.panelCollapsed = false
        tab.panel = .result
        let window = openWindow(model)
        await sleep(1.0)
        let sampler = FootprintSampler()
        let hz = refreshRate(of: window)

        let rows: Int, columns: Int
        switch scenario {
        case "ttfr-s1-1k": (rows, columns) = (1_000, 3)
        case "ttfr-s1-10k": (rows, columns) = (10_000, 3)
        case "rows-wide-500k": (rows, columns) = (500_000, 30)
        case "mem-500k": (rows, columns) = (500_000, 30)
        case "rows-mysql-500k", "rows-trino-500k": (rows, columns) = (500_000, 0)
        default: (rows, columns) = (1, 1)
        }
        let sql: String
        if scenario == "cancel-pg-sleep" {
            guard connection.kind == .postgres else {
                return emitStatus(scenario, "tidak mendukung", "pg_sleep needs PostgreSQL")
            }
            sql = "SELECT pg_sleep(30)"
        } else if scenario == "rows-mysql-500k" || scenario == "rows-trino-500k" {
            // Fixed tables from deploy/dev (MySQL 53306, Trino 58080), not a generator.
            let wanted: ConnectionKind = scenario == "rows-mysql-500k" ? .mysql : .trino
            guard connection.kind == wanted else {
                return emitStatus(scenario, "tidak mendukung", "\(scenario) needs QH_BENCH_KIND=\(wanted.rawValue)")
            }
            sql = wanted == .mysql ? "SELECT * FROM wide_500k" : "SELECT * FROM tpch.sf1.lineitem LIMIT 500000"
        } else if let generated = generatedSQL(connection.kind, rows: rows, columns: columns) {
            sql = generated
        } else {
            return emitStatus(scenario, "tidak mendukung", "no generator for \(connection.kind.rawValue) at \(rows) rows")
        }
        tab.sql = sql
        tab.rowLimit = max(1_000, rows)
        // Bench-only: the product clamps a Run to 200,000 rows, which would turn rows-wide-500k and
        // mem-500k into 200k runs under 500k names. Raised here to what the scenario asks for and
        // reported as `row_limit_cap` in every line, so a number can never hide its cap.
        AppModel.rowLimitCeiling = max(AppModel.productRowLimitCeiling, tab.rowLimit)

        for pass in 0..<repeats {
            log("repeat \(pass + 1)/\(repeats)")
            tab.preview = nil
            await sleep(1.0)
            let baseline = FootprintSampler.current()
            sampler.start()
            PerfSignposts.clear("run", "runDone", "firstPaint")
            model.preview(tab)
            guard let started = PerfSignposts.time(of: "run") else {
                emitStatus(scenario, "[belum diukur]", tab.previewError ?? "the run did not start")
                _ = sampler.stop()
                continue
            }

            if scenario == "cancel-pg-sleep" {
                await sleep(1.5)
                PerfSignposts.clear("cancel", "cancelEnd")
                model.cancelPreview(tab)
                _ = await wait(timeout: 60) { !tab.previewing }
                _ = sampler.stop()
                guard let began = PerfSignposts.time(of: "cancel"), let ended = PerfSignposts.time(of: "cancelEnd") else {
                    emitStatus(scenario, "[belum diukur]", "the run never ended")
                    continue
                }
                var metrics = ["cancel_ms": (ended - began) * 1000]
                if let engine = PerfSignposts.time(of: "engine.done") { metrics["cancel_engine_done_ms"] = (engine - began) * 1000 }
                emit(scenario, metrics, extra: ["row_limit_cap": AppModel.rowLimitCeiling])
                continue
            }

            let finished = await wait(timeout: 540) { !tab.previewing }
            await sleep(1.0)
            let settled = FootprintSampler.current()
            let peak = sampler.stop()
            guard finished, tab.previewError == nil, let done = PerfSignposts.time(of: "runDone") else {
                emitStatus(scenario, "[belum diukur]", tab.previewError ?? "the run did not finish")
                continue
            }
            var metrics: [String: Double] = ["duration_ms": (done - started) * 1000,
                                             "rows_count": Double(tab.preview?.rowCount ?? 0),
                                             "footprint_delta_bytes": Double(Int64(settled) - Int64(baseline)),
                                             "peak_footprint_delta_bytes": Double(Int64(peak) - Int64(baseline))]
            if let painted = PerfSignposts.time(of: "firstPaint") { metrics["ttfr_ms"] = (painted - started) * 1000 }
            metrics["rows_per_s"] = Double(tab.preview?.rowCount ?? 0) / max(done - started, 0.001)
            // R-29: what the store holds and what it spilled, next to the footprint.
            let stats = RustEngine.storeStats()
            metrics["spilled_bytes"] = Double(stats?.spilledBytes ?? 0)
            metrics["budget_bytes"] = Double(stats?.budgetBytes ?? 0)
            emit(scenario, metrics, extra: ["display_hz": hz, "row_limit_cap": AppModel.rowLimitCeiling])
        }
    }
}

/// `phys_footprint` from `proc_pid_rusage`, the number Activity Monitor calls Memory, sampled every
/// 20 ms so a peak that lives for one run-loop turn is still seen.
final class FootprintSampler {
    private let queue = DispatchQueue(label: "bench.footprint")
    private var timer: DispatchSourceTimer?
    private let lock = NSLock()
    private var high: UInt64 = 0

    static func current() -> UInt64 {
        var info = rusage_info_current()
        let status = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) {
                proc_pid_rusage(getpid(), RUSAGE_INFO_CURRENT, $0)
            }
        }
        return status == 0 ? info.ri_phys_footprint : 0
    }

    func start() {
        lock.lock()
        high = Self.current()
        lock.unlock()
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(20))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let now = Self.current()
            self.lock.lock()
            self.high = max(self.high, now)
            self.lock.unlock()
        }
        timer.resume()
        self.timer = timer
    }

    /// Stops sampling and returns the highest value seen since `start`.
    func stop() -> UInt64 {
        timer?.cancel()
        timer = nil
        lock.lock()
        defer { lock.unlock() }
        return max(high, Self.current())
    }
}

/// The delegate of a `--bench` run's own `NSApplication`.
final class BenchDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        BenchMode.refuseTermination()
    }
}
