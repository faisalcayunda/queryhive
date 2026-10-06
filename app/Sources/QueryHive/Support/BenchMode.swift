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
/// - `frame_cost_*_ms`: the main thread's own cost per scrolled frame, tick to the end of the turn that
///   drew it. A 60 Hz panel cannot show a 120 Hz frame time, but it can show this.
/// - `stage_*_ms`, `delta_*_ms`: where a Run's time goes, from the stamps in `PerfSignposts.Stage`.
/// - `part_*`: sub-intervals inside a keystroke, from `PerfSignposts.part`.
///
/// Two families of database scenarios: the plan's own SQL (`SELECT * FROM wide_500k`, the `*t` and
/// `rows-pg-table` names), which the gates grade, and the generated SQL the earlier scenarios run,
/// which stays as a regression guard. Every database line carries `sql_kind`, and a `notes` string
/// of `key=value` pairs the report reads (`sql_kind`, `via`, `rtt_ms`, `warm_up`, `first_run`).
enum BenchMode {
    static let synthetic = ["scroll-30x1m", "scroll-30x1m-spilled", "scroll-500x10k", "open-500x10k", "type-10k",
                            "type-10k-plan", "type-2m", "type-coloured-195k",
                            "tabs-100", "tabs-100-held", "launch-warm", "launch-cold"]
    static var database: [String] { cases.keys.sorted() }
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

    /// The budget `scroll-30x1m-spilled` runs on: a fraction of the fixture, so the store spills.
    static let spilledBudget: UInt64 = 64 << 20

    /// The store budget for this process. The host takes one configuration for its whole life, so a
    /// `--bench` run names the scenarios that share it: the tab scenarios need a budget small enough
    /// that their stores really spill, the database ones run on the product's own 256 MiB with spill,
    /// and the synthetic grid scenarios get 2 GiB with no spill (comparable with Fase 0, where the
    /// rows were an array in the heap). Run `scroll-30x1m`, `scroll-30x1m-spilled` and `tabs-*` in
    /// separate processes.
    private static func configureStores(for scenarios: [String], support: URL) {
        let spill = support.appendingPathComponent("spill").path
        if scenarios.contains(where: { $0.hasPrefix("tabs-") }) {
            RustEngine.ensureStoresConfigured(spillDir: spill, budgetBytes: 2 << 20)
        } else if scenarios.contains("scroll-30x1m-spilled") {
            // Most of 1M x 30 does not fit, so most pages are read back from disk while scrolling.
            RustEngine.ensureStoresConfigured(spillDir: spill, budgetBytes: spilledBudget)
        } else if scenarios.contains(where: { cases[$0] != nil }) {
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
            await runChild("launch-warm", arguments: ["--bench", "launch-warm"], environment: environment)
        }
    }

    /// S4, the first Run after the app opens, can only be sampled once per process: every sample is a
    /// child that runs the scenario as a one-shot (`QH_BENCH_ONESHOT`), with a throwaway support
    /// directory of its own, like any other `--bench` process.
    @MainActor
    private static func firstRuns(_ scenario: String, repeats: Int) async {
        var environment = ProcessInfo.processInfo.environment
        environment["QH_BENCH_ONESHOT"] = "1"
        for _ in 0..<repeats {
            await runChild(scenario, arguments: ["--bench", scenario, "--bench-repeat", "1"], environment: environment)
        }
    }

    private static var oneShot: Bool { ProcessInfo.processInfo.environment["QH_BENCH_ONESHOT"] != nil }

    /// Runs this binary as a child, passes the sample lines it prints on, and says why when it printed none.
    @MainActor
    private static func runChild(_ scenario: String, arguments: [String], environment: [String: String]) async {
        let child = Process()
        child.executableURL = URL(fileURLWithPath: Bundle.main.executablePath ?? CommandLine.arguments[0])
        child.arguments = arguments
        child.environment = environment
        let pipe = Pipe()
        child.standardOutput = pipe
        do { try child.run() } catch {
            return emitStatus(scenario, "[belum diukur]", "cannot start a child: \(error)")
        }
        let data = await Task.detached { pipe.fileHandleForReading.readDataToEndOfFile() }.value
        child.waitUntilExit()
        let lines = String(decoding: data, as: UTF8.self).split(separator: "\n").filter { $0.hasPrefix("{") }
        if lines.isEmpty {
            emitStatus(scenario, "[belum diukur]", "the child exited with status \(child.terminationStatus) and no sample")
        }
        lines.forEach { writeRaw(String($0)) }
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
        case "scroll-30x1m-spilled":
            await scroll(scenario, rows: 1_000_000, columns: 30, both: false, repeats: repeats, spilled: true)
        case "scroll-500x10k": await scroll(scenario, rows: 10_000, columns: 500, both: true, repeats: repeats)
        case "open-500x10k": await open(scenario, rows: 10_000, columns: 500, repeats: repeats)
        case "type-10k": await type(scenario, lines: 10_000, minimumCharacters: 0, repeats: repeats)
        // The plan's fixture: 10k lines of about 40 characters, and a fresh line for every repeat.
        case "type-10k-plan": await type(scenario, lines: 10_000, minimumCharacters: 0, repeats: repeats, plan: true)
        case "type-2m": await type(scenario, lines: 0, minimumCharacters: 2_000_000, repeats: repeats)
        // Under the analysis ceiling, so the document is fully coloured and a keystroke repaints;
        // typed text is SQL with keywords, and every 20th key is followed by a pause longer than the
        // editor's debounces, so the analysis and the model write run between keystrokes.
        case "type-coloured-195k": await type(scenario, lines: 0, minimumCharacters: 194_000, repeats: repeats, coloured: true)
        case "tabs-100": await tabs(scenario, repeats: repeats)
        case "tabs-100-held": await tabsHeld(scenario, repeats: repeats)
        case "launch-warm": await launchWarm(repeats: repeats)
        case "launch-cold": emitStatus(scenario, "tidak diukur (butuh sudo)", "a cold start needs `purge`, which needs sudo")
        case "ttfr-s4-first-run" where !oneShot: await firstRuns(scenario, repeats: repeats)
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

    static func percentile(_ values: [Double], _ p: Double) -> Double {
        guard !values.isEmpty else { return .nan }
        let sorted = values.sorted()
        return sorted[min(sorted.count - 1, max(0, Int((Double(sorted.count) * p).rounded(.up)) - 1))]
    }

    // MARK: Pure parts

    /// What a Run's stamps say, as metrics: every stage's offset from `run`, and the four intervals the
    /// W8-T2 diagnosis asks about. A stage nobody stamped is left out, so a number is never a guess.
    ///
    /// - H1, `delta_grid_attach_ms`: the grid attached after `columns` (a grid rebuilt per Run).
    /// - H2, `delta_done_hop_ms`: `runDone` after the engine's `done` (the main thread was busy).
    /// - H3, `delta_first_draw_ms`: the first draw after the first rows (a display-cycle wait).
    /// - H4, `delta_flush_ms`: `firstPaint` after the first draw (the flush).
    /// - H5, `first_draw_after_done_count`: 1 when no paint happened before the engine finished.
    static func stageMetrics(_ stamps: [String: CFAbsoluteTime]) -> [String: Double] {
        typealias Stage = PerfSignposts.Stage
        guard let run = stamps[Stage.run.rawValue] else { return [:] }
        var metrics = [String: Double]()
        for stage in Stage.allCases where stage != .run {
            if let at = stamps[stage.rawValue] { metrics["stage_\(stage.metric)_ms"] = (at - run) * 1000 }
        }
        func interval(_ name: String, _ later: Stage, _ earlier: Stage) {
            if let after = stamps[later.rawValue], let before = stamps[earlier.rawValue] {
                metrics[name] = (after - before) * 1000
            }
        }
        interval("delta_grid_attach_ms", .gridAttached, .columns)
        interval("delta_done_hop_ms", .runDone, .engineDone)
        interval("delta_first_draw_ms", .firstDraw, .firstRows)
        interval("delta_flush_ms", .firstPaint, .firstDraw)
        if let draw = stamps[Stage.firstDraw.rawValue], let done = stamps[Stage.engineDone.rawValue] {
            metrics["first_draw_after_done_count"] = draw > done ? 1 : 0
        }
        return metrics
    }

    /// The sub-intervals of a typing repeat, one dictionary per key: p50 and p99 of the time each named
    /// part took in a key (a key it did not run in counts as 0), and the mean count per key. A part
    /// that only counts leaves out the times.
    static func partMetrics(_ turns: [[String: PerfSignposts.PartTotal]]) -> [String: Double] {
        guard !turns.isEmpty else { return [:] }
        var metrics = [String: Double]()
        for name in Set(turns.flatMap(\.keys)) {
            let milliseconds = turns.map { $0[name]?.ms ?? 0 }
            let counts = turns.map { Double($0[name]?.count ?? 0) }
            let key = "part_\(PerfSignposts.snakeCase(name))"
            if milliseconds.contains(where: { $0 > 0 }) {
                metrics["\(key)_p50_ms"] = percentile(milliseconds, 0.5)
                metrics["\(key)_p99_ms"] = percentile(milliseconds, 0.99)
            }
            metrics["\(key)_count"] = counts.reduce(0, +) / Double(counts.count)
        }
        return metrics
    }

    /// The E4 question (W8-T2 §1): does a Run right after a capped one pay more than a Run after a
    /// finished one? `uncapped` is that warm baseline, `capped` the second capped Run, `warm` a
    /// capped Run after a finished one. The trigger is "more than one RTT"; on loopback one RTT is
    /// taken as 1 ms.
    static func rerunMetrics(uncapped: Double, warm: Double, capped: Double, sort: Double, rttMS: Double) -> [String: Double] {
        let penalty = capped - uncapped
        return ["rerun_uncapped_ttfr_ms": uncapped, "rerun_warm_ttfr_ms": warm,
                "rerun_capped_ttfr_ms": capped, "rerun_sort_ttfr_ms": sort,
                "rerun_penalty_ms": penalty, "rerun_penalty_capped_ms": capped - warm,
                "rerun_exceeds_rtt_ratio": penalty > max(rttMS, 1) ? 1 : 0,
                "rtt_ms": rttMS]
    }

    /// A typing document is cut this far below the editor's ceiling, so the characters typed into it
    /// cannot cross the ceiling (past it the analysis colours nothing).
    static let ceilingMargin = 4_096

    /// `document` cut to the ceiling minus the margin, in UTF-16 units. A document that is already
    /// shorter is returned as it is.
    static func fitToCeiling(_ document: String, ceiling: Int) -> String {
        let limit = max(0, ceiling - ceilingMargin)
        let text = document as NSString
        return text.length > limit ? text.substring(to: limit) : document
    }

    /// The plan's editor fixture: `lines` statements of about 40 characters each (10,000 of them are
    /// about 400k characters), so a line is typed on that has not been typed on before.
    static func planDocument(lines: Int) -> String {
        var text = ""
        text.reserveCapacity(lines * 40)
        for index in 0..<lines {
            text += "SELECT a, b FROM t\(index) WHERE id = \(index);\n"
        }
        return text
    }

    /// The start of the line before the one that holds `location`: a line no `type-*` scenario types
    /// on, whose first word is a keyword, so it keeps a colour for as long as the analysis colours
    /// the window. Nil on the document's first line.
    static func probeIndex(in text: NSString, before location: Int) -> Int? {
        let line = text.lineRange(for: NSRange(location: min(location, text.length), length: 0))
        guard line.location > 0 else { return nil }
        return text.lineRange(for: NSRange(location: line.location - 1, length: 0)).location
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
        /// The main thread's cost of each frame this driver moved, in milliseconds: from the tick to
        /// the end of the run-loop turn that laid out and drew it (an observer after Core
        /// Animation's own commit, like the keystroke interval). It does not depend on the panel's
        /// refresh rate, so a 60 Hz panel can hold it to the 120 Hz budget when the frame time
        /// itself cannot show one.
        private(set) var costs: [Double] = []
        private var costObserver: CFRunLoopObserver?

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
            if let observer = costObserver { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
            costObserver = nil
            continuation?.resume()
            continuation = nil
        }

        /// One observer at a time: the turn that ran this tick ends before the next tick can start.
        private func measureCost(from entered: CFAbsoluteTime) {
            guard costObserver == nil else { return }
            let observer = CFRunLoopObserverCreateWithHandler(
                nil, CFRunLoopActivity.beforeWaiting.rawValue, false, 2_000_001
            ) { [weak self] observer, _ in
                if let observer { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
                guard let self else { return }
                costs.append((CFAbsoluteTimeGetCurrent() - entered) * 1000)
                costObserver = nil
            }
            costObserver = observer
            CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
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
            let entered = CFAbsoluteTimeGetCurrent()
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
            measureCost(from: entered)
        }
    }

    private static func refreshRate(of window: NSWindow) -> Int {
        (window.screen ?? NSScreen.main)?.maximumFramesPerSecond ?? 60
    }

    @MainActor
    private static func scroll(_ scenario: String, rows: Int, columns: Int, both: Bool, repeats: Int,
                               spilled: Bool = false) async {
        guard let (_, tab, window) = await gridWindow() else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        let result = syntheticResult(rows: rows, columns: columns)
        guard let openMS = await show(result, in: tab) else {
            return emitStatus(scenario, "[belum diukur]", "the grid drew no row within the time limit")
        }
        // A "spilled" run that did not spill would measure the resident path under the wrong name.
        let stats = RustEngine.storeStats()
        if spilled, (stats?.spilledBytes ?? 0) == 0 {
            return emitStatus(scenario, "[belum diukur]",
                              "the store did not spill (budget \(stats?.budgetBytes ?? 0) bytes), so this would measure the resident path")
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
            if !driver.costs.isEmpty {
                metrics["frame_cost_p50_ms"] = percentile(driver.costs, 0.5)
                metrics["frame_cost_p99_ms"] = percentile(driver.costs, 0.99)
            }
            // The cold first open of the process: pass 0 only, so n = 1 and it is never a median.
            if pass == 0 { metrics["render_ms"] = openMS }
            if spilled {
                let now = RustEngine.storeStats()
                metrics["spilled_bytes"] = Double(now?.spilledBytes ?? 0)
                metrics["budget_bytes"] = Double(now?.budgetBytes ?? 0)
            }
            var extra: [String: Any] = ["display_hz": hz, "pass": pass]
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
        // One open more than asked for: the first is the process's cold open and is reported as
        // `render_cold_ms`, so `render_ms` is the median of `repeats` warm opens (W8-T2 §1, row 8).
        for pass in 0...repeats {
            tab.preview = nil
            tab.activeResult?.release()
            tab.activeResult = nil
            await sleep(0.5)
            guard let ms = await show(result, in: tab) else {
                return emitStatus(scenario, "[belum diukur]", "the grid drew no row within the time limit")
            }
            // The same stamps a database Run leaves, minus the engine's: where the open spends its time.
            var metrics = [pass == 0 ? "render_cold_ms" : "render_ms": ms]
            metrics.merge(stageMetrics(PerfSignposts.stampSnapshot())) { _, new in new }
            emit(scenario, metrics, extra: ["pass": pass])
        }
    }

    // MARK: Type

    static func sqlDocument(lines: Int, minimumCharacters: Int) -> String {
        var text = ""
        var index = 0
        while index < lines || text.utf16.count < minimumCharacters {
            text += "SELECT id, name, amount FROM public.table_\(index) WHERE id = \(index) AND status = 'open' ORDER BY id;\n"
            index += 1
        }
        return text
    }

    /// Whether the character at `index` still carries a colour from the analysis, or nil when this
    /// text view cannot say (TextKit 2: reading `layoutManager` there would drop it to TextKit 1,
    /// which a measurement must not do).
    @MainActor
    private static func isColoured(_ textView: SQLTextView, at index: Int) -> Bool? {
        guard textView.textLayoutManager == nil, let layoutManager = textView.layoutManager,
              index < (textView.textStorage?.length ?? 0) else { return nil }
        return layoutManager.temporaryAttribute(.foregroundColor, atCharacterIndex: index, effectiveRange: nil) != nil
    }

    /// `plan`: the plan's fixture (`planDocument`) and a fresh line for every repeat, so no repeat
    /// types on a line the one before it made longer. Without it the typed line grows by 200
    /// characters a repeat, which is what `type-10k` has always done and keeps doing.
    ///
    /// Every key is followed by a check that the analysis still colours the window, by looking at the
    /// first word of a line nobody types on. A key that finds it bare is an "uncoloured turn", and
    /// a run that has any measured the uncoloured path under a coloured name: the report does not
    /// grade it (`type-2m` did exactly that before W8-F0).
    @MainActor
    private static func type(_ scenario: String, lines: Int, minimumCharacters: Int, repeats: Int,
                             coloured: Bool = false, plan: Bool = false) async {
        let model = AppModel()
        guard let tab = model.selectedTab else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        // The analysis colours nothing past its ceiling, and the typed characters count: a document
        // that starts at the ceiling crosses it on the first key and benches the uncoloured path.
        let ceiling = (try? editorCeiling()) ?? 2_000_000
        let document = fitToCeiling(plan ? planDocument(lines: lines)
                                         : sqlDocument(lines: lines, minimumCharacters: minimumCharacters),
                                    ceiling: ceiling)
        let source = document as NSString
        let length = source.length
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
        let middle = source.lineRange(for: NSRange(location: length / 2, length: 0)).location
        textView.setSelectedRange(NSRange(location: middle, length: 0))
        textView.scrollRangeToVisible(NSRange(location: middle, length: 0))
        await sleep(1.0)

        // Before the first key: the document has to be coloured, or there is nothing to measure.
        let probe = probeIndex(in: source, before: middle)
        if let probe, isColoured(textView, at: probe) != nil {
            let ready = await wait(timeout: 120) { isColoured(textView, at: probe) ?? true }
            guard ready else {
                return emitStatus(scenario, "[belum diukur]",
                                  "the document never took its colours (\(length) characters, ceiling \(ceiling)), so the keystrokes would measure the uncoloured path")
            }
        }

        // Coloured typing is realistic SQL: short lines and one statement per `;`, so each repeat
        // measures the same shape instead of one line that grows by 200 characters a repeat.
        let text = Array(coloured ? "select id, name\nfrom t\nwhere x = 1 and y in (2);\n" : "x_foo bar_1 baz ")
        for pass in 0..<repeats {
            if plan, pass > 0 {
                // The line after the one just typed on, untouched.
                let typed = (textView.string as NSString).lineRange(for: NSRange(location: textView.selectedRange().location, length: 0))
                let fresh = NSRange(location: NSMaxRange(typed), length: 0)
                textView.setSelectedRange(fresh)
                textView.scrollRangeToVisible(fresh)
                await sleep(1.0)
            }
            var insert = [Double](), didChange = [Double](), keystroke = [Double](), apply = [Double]()
            var turns = [[String: PerfSignposts.PartTotal]]()
            var probed = 0, uncoloured = 0
            for i in 0..<200 {
                PerfSignposts.keystrokeReset()
                PerfSignposts.applyReset()
                PerfSignposts.partsReset()
                let started = CFAbsoluteTimeGetCurrent()
                PerfSignposts.keystrokeBegin()
                textView.insertText(String(text[i % text.count]),
                                    replacementRange: NSRange(location: NSNotFound, length: 0))
                insert.append((CFAbsoluteTimeGetCurrent() - started) * 1000)
                // The repaint, not a sleep: the next key waits for this one's colours to display.
                _ = await wait(timeout: 10) { PerfSignposts.lastApplyMS != nil }
                if let probe, let lit = isColoured(textView, at: probe) {
                    probed += 1
                    if !lit { uncoloured += 1 }
                }
                if coloured, i % 20 == 19 { await sleep(0.2) }
                if let ms = PerfSignposts.lastDidChangeMS { didChange.append(ms) }
                if let ms = PerfSignposts.lastKeystrokeMS { keystroke.append(ms) }
                if let ms = PerfSignposts.lastApplyMS { apply.append(ms) }
                turns.append(PerfSignposts.partsSnapshot())
            }
            let mode = textView.textLayoutManager != nil ? 2 : 1
            var metrics = ["keystroke_main_p50_ms": percentile(keystroke, 0.5),
                           "keystroke_main_p99_ms": percentile(keystroke, 0.99),
                           "apply_main_p50_ms": percentile(apply, 0.5),
                           "apply_main_p99_ms": percentile(apply, 0.99),
                           "did_change_p99_ms": percentile(didChange, 0.99),
                           "insert_text_p99_ms": percentile(insert, 0.99),
                           "characters_count": Double(length),
                           "keystroke_samples_count": Double(keystroke.count)]
            // Absent, not 0, when this text view cannot be probed: "no uncoloured turn" is a claim.
            if probed > 0 {
                metrics["uncoloured_turns_count"] = Double(uncoloured)
                metrics["probed_turns_count"] = Double(probed)
            }
            metrics.merge(partMetrics(turns)) { _, new in new }
            emit(scenario, metrics, extra: ["textkit": mode, "display_hz": refreshRate(of: window), "pass": pass])
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

    /// Where toxiproxy listens (`deploy/dev/up.sh toxiproxy`: PostgreSQL behind 15 ms each way) and the
    /// round trip that adds. Both can be moved for another proxy.
    private static var rttPort: Int { Int(ProcessInfo.processInfo.environment["QH_BENCH_RTT_PORT"] ?? "") ?? 55435 }
    private static var rttMS: Double { Double(ProcessInfo.processInfo.environment["QH_BENCH_RTT_MS"] ?? "") ?? 30 }

    private static func connectionFromEnvironment(portOverride: Int? = nil) -> (Connection, String)? {
        let env = ProcessInfo.processInfo.environment
        guard let host = env["QH_BENCH_HOST"], let port = portOverride ?? env["QH_BENCH_PORT"].flatMap(Int.init),
              let user = env["QH_BENCH_USER"], let database = env["QH_BENCH_DB"] else { return nil }
        let kind = ConnectionKind(rawValue: env["QH_BENCH_KIND"] ?? "postgres") ?? .postgres
        let connection = Connection(id: UUID(), name: "bench", color: .blue, kind: kind, host: host, port: port,
                                    scheme: kind == .trino ? "http" : "", sslmode: kind == .postgres ? "disable" : "",
                                    user: user, database: database, schema: kind == .postgres ? "public" : "",
                                    verify: false)
        return (connection, env["QH_BENCH_PASSWORD"] ?? "")
    }

    /// `rows` rows of `columns` columns, generated by the server. Postgres and Trino only, and
    /// Trino's `sequence` is capped at 10,000 entries. The server computes every value (md5 per cell
    /// on PostgreSQL), so these scenarios measure the server as much as the app: they are the
    /// regression guard, and the plan's own table scans (`DatabaseCase.sqlKind` "table") are what the
    /// gates grade.
    static func generatedSQL(_ kind: ConnectionKind, rows: Int, columns: Int) -> String? {
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

    /// What a database scenario runs and how it is driven. The timing lives in `fetch`, `cancel` and
    /// `rerun`; this is what they ask the server.
    struct DatabaseCase {
        enum Flow {
            /// Run, wait for the end, one record per pass.
            case fetch
            /// Run, press Stop after this many seconds.
            case cancelAfter(Double)
            /// Run, press Stop once this many rows are in.
            case cancelMidStream(rows: Int)
            /// An uncapped Run, a capped one after it, the same capped Run again, then a server sort.
            case rerun
            /// The first Run of a process: every sample is a fresh child.
            case firstRun
        }

        /// The kinds this runs on; empty means any kind `sql` has a statement for.
        var kinds: [ConnectionKind] = []
        var sql: (ConnectionKind) -> String?
        /// The Run's row limit.
        var cap: Int
        /// "table": the plan's SQL against a stored table. "generated": a server-side generator, so
        /// the server's compute is part of the number. "sleep" and "heavy": no rows to measure.
        var sqlKind: String
        var flow = Flow.fetch
        /// Through toxiproxy, not straight to the server.
        var viaToxiproxy = false
        /// One Run first that is not recorded, for the scenarios the plan calls warm (S1, S2, S3).
        var warmUp = false
    }

    /// The plan's SQL for axes 1 to 3: `docs/architecture/performance-plan.md` §2, S1.
    static let planSQL = "SELECT * FROM wide_500k"

    private static func lineitem(_ rows: Int) -> String { "SELECT * FROM tpch.sf1.lineitem LIMIT \(rows)" }

    /// Every database scenario. The generated-SQL ones keep their names and numbers as the G-BENCHQ
    /// regression guard; the rest are the plan's.
    static let cases: [String: DatabaseCase] = {
        func generated(_ rows: Int, _ columns: Int) -> (ConnectionKind) -> String? {
            { BenchMode.generatedSQL($0, rows: rows, columns: columns) }
        }
        func fixed(_ sql: String) -> (ConnectionKind) -> String? { { _ in sql } }

        let base = DatabaseCase(kinds: [.postgres], sql: fixed(BenchMode.planSQL), cap: 1_000, sqlKind: "table", warmUp: true)
        func table(cap: Int) -> DatabaseCase { var c = base; c.cap = cap; return c }
        func scan(_ kind: ConnectionKind, _ sql: String, cap: Int) -> DatabaseCase {
            DatabaseCase(kinds: [kind], sql: fixed(sql), cap: cap, sqlKind: "table")
        }
        func cancelling(_ kind: ConnectionKind, _ sql: String, sqlKind: String, _ flow: DatabaseCase.Flow,
                        cap: Int = 1_000) -> DatabaseCase {
            DatabaseCase(kinds: [kind], sql: fixed(sql), cap: cap, sqlKind: sqlKind, flow: flow)
        }
        var rtt = base; rtt.viaToxiproxy = true
        var rerunCase = base; rerunCase.flow = .rerun; rerunCase.warmUp = false
        var rerunRTT = rerunCase; rerunRTT.viaToxiproxy = true
        var firstRun = base; firstRun.flow = .firstRun; firstRun.warmUp = false

        return [
            // The regression guard: generated SQL, the names and numbers of before W8.
            "ttfr-s1-1k": DatabaseCase(sql: generated(1_000, 3), cap: 1_000, sqlKind: "generated"),
            "ttfr-s1-10k": DatabaseCase(sql: generated(10_000, 3), cap: 10_000, sqlKind: "generated"),
            "rows-wide-500k": DatabaseCase(sql: generated(500_000, 30), cap: 500_000, sqlKind: "generated"),
            "mem-500k": DatabaseCase(sql: generated(500_000, 30), cap: 500_000, sqlKind: "generated"),
            // The plan's scenarios. S1 to S3 are warm; S4 is the one that is not.
            "ttfr-s1t-1k": table(cap: 1_000),
            "ttfr-s1t-10k": table(cap: 10_000),
            "ttfr-s2t-500k": table(cap: 500_000),
            "ttfr-s3-rtt30": rtt,
            "ttfr-s4-first-run": firstRun,
            "rerun-capped": rerunCase,
            "rerun-capped-rtt30": rerunRTT,
            // Axis 2 and 3: a table scan, not a generator, and no warm-up (the first rows are not the point).
            "rows-pg-table-500k": scan(.postgres, BenchMode.planSQL, cap: 500_000),
            "rows-mysql-500k": scan(.mysql, BenchMode.planSQL, cap: 500_000),
            "mem-mysql-500k": scan(.mysql, BenchMode.planSQL, cap: 500_000),
            "rows-trino-500k": scan(.trino, BenchMode.lineitem(500_000), cap: 500_000),
            "rows-lineitem-1m": scan(.trino, BenchMode.lineitem(1_000_000), cap: 1_000_000),
            // 5M rows without a generator's compute: the table read ten times over on PostgreSQL.
            "mem-5m": DatabaseCase(kinds: [.postgres, .trino], sql: { kind in
                switch kind {
                case .postgres: return "SELECT w.* FROM wide_500k AS w CROSS JOIN generate_series(1, 10) AS g"
                case .trino: return BenchMode.lineitem(5_000_000)
                case .mysql: return nil
                }
            }, cap: 5_000_000, sqlKind: "table"),
            // Axis 6. The statement outlives any wait, so the app has to be the one that stops it.
            "cancel-pg-sleep": cancelling(.postgres, "SELECT pg_sleep(30)", sqlKind: "sleep", .cancelAfter(1.5)),
            "cancel-mysql-sleep": cancelling(.mysql, "SELECT SLEEP(30)", sqlKind: "sleep", .cancelAfter(1.5)),
            "cancel-stream-wide": cancelling(.postgres, BenchMode.planSQL, sqlKind: "table", .cancelMidStream(rows: 100_000),
                                             cap: 500_000),
            // sf100 is generated on the fly, so the aggregate has 600M rows to make (the tpch catalog names
            // its columns without the table prefix).
            "cancel-trino-heavy": cancelling(.trino,
                                             "SELECT count(*), sum(extendedprice * (1 - discount)) FROM tpch.sf100.lineitem",
                                             sqlKind: "heavy", .cancelAfter(1.5)),
        ]
    }()

    /// `key=value; key=value`, the form `bench_fetch.py` reads back out of a record's notes.
    static func recordNotes(_ pairs: [(String, String)]) -> String {
        pairs.map { "\($0.0)=\($0.1)" }.joined(separator: "; ")
    }

    /// One database scenario's model and tab, and the labels each of its records carries.
    private struct DatabaseRun {
        let scenario: String
        let spec: DatabaseCase
        let model: AppModel
        let tab: QueryTab
        let hz: Int

        func extra(pass: Int) -> [String: Any] {
            var pairs = [("sql_kind", spec.sqlKind)]
            if spec.viaToxiproxy { pairs += [("via", "toxiproxy:\(BenchMode.rttPort)"), ("rtt_ms", String(Int(BenchMode.rttMS)))] }
            if spec.warmUp { pairs.append(("warm_up", "1")) }
            if BenchMode.oneShot { pairs.append(("first_run", "1")) }
            return ["display_hz": hz, "row_limit_cap": AppModel.rowLimitCeiling, "sql_kind": spec.sqlKind,
                    "pass": pass, "notes": BenchMode.recordNotes(pairs)]
        }
    }

    @MainActor
    private static func databaseScenario(_ scenario: String, repeats: Int) async {
        guard let spec = cases[scenario] else {
            return emitStatus(scenario, "[belum diukur]", "no database case is defined for \(scenario)")
        }
        guard let (connection, password) = connectionFromEnvironment(portOverride: spec.viaToxiproxy ? rttPort : nil) else {
            return emitStatus(scenario, "[belum diukur]", "QH_BENCH_HOST, _PORT, _USER and _DB are not all set")
        }
        guard spec.kinds.isEmpty || spec.kinds.contains(connection.kind) else {
            return emitStatus(scenario, "tidak mendukung",
                              "\(scenario) needs QH_BENCH_KIND=\(spec.kinds.map(\.rawValue).joined(separator: " or "))")
        }
        guard let sql = spec.sql(connection.kind) else {
            return emitStatus(scenario, "tidak mendukung", "no generator for \(connection.kind.rawValue) at \(spec.cap) rows")
        }
        AppModel.benchPassword = password
        let model = AppModel()
        model.connections = [connection]
        model.rebuildTree()
        guard let tab = model.selectedTab else { return emitStatus(scenario, "[belum diukur]", "no tab") }
        tab.connectionID = connection.id
        model.panelCollapsed = false
        tab.panel = .result
        // The app's first local command opens the execution log (and migrates a fresh database)
        // for its first Run; the app makes one at launch (the session restore), a bench model does
        // not, so make it here and keep that open out of the first sample's time to first row.
        model.loadHistory()
        let window = openWindow(model)
        await sleep(1.0)
        tab.sql = sql
        tab.rowLimit = max(1_000, spec.cap)
        // Bench-only: the product clamps a Run to 200,000 rows, which would turn rows-wide-500k and
        // mem-500k into 200k runs under 500k names. Raised here to what the scenario asks for and
        // reported as `row_limit_cap` in every line, so a number can never hide its cap.
        AppModel.rowLimitCeiling = max(AppModel.productRowLimitCeiling, tab.rowLimit)
        let run = DatabaseRun(scenario: scenario, spec: spec, model: model, tab: tab, hz: refreshRate(of: window))
        switch spec.flow {
        case .fetch: await fetch(run, repeats: repeats)
        // Only a one-shot child gets here (`execute` hands every other `firstRun` to `firstRuns`).
        case .firstRun: await fetch(run, repeats: 1)
        case .cancelAfter, .cancelMidStream: await cancel(run, repeats: repeats)
        case .rerun: await rerun(run, repeats: repeats)
        }
    }

    /// One Run that is not recorded, so what follows is warm: the connection is open and the server
    /// has the table's pages.
    @MainActor
    private static func warmUp(_ r: DatabaseRun) async {
        log("warm-up Run, not recorded")
        r.tab.preview = nil
        r.model.preview(r.tab)
        _ = await wait(timeout: 540) { !r.tab.previewing }
        await sleep(1.0)
    }

    /// Run, wait for the end, and report time to first row, throughput and memory.
    @MainActor
    private static func fetch(_ r: DatabaseRun, repeats: Int) async {
        let tab = r.tab
        let sampler = FootprintSampler()
        if r.spec.warmUp { await warmUp(r) }
        for pass in 0..<repeats {
            log("repeat \(pass + 1)/\(repeats)")
            tab.preview = nil
            await sleep(1.0)
            let baseline = FootprintSampler.current()
            sampler.start()
            // Every stage stamp goes, the engine's too: an `onlyFirst` stamp left from the repeat
            // before would read as this one's.
            PerfSignposts.clearStages()
            r.model.preview(tab)
            guard let started = PerfSignposts.time(of: .run) else {
                emitStatus(r.scenario, "[belum diukur]", tab.previewError ?? "the run did not start")
                _ = sampler.stop()
                continue
            }
            let finished = await wait(timeout: 540) { !tab.previewing }
            await sleep(1.0)
            let settled = FootprintSampler.current()
            let peak = sampler.stop()
            guard finished, tab.previewError == nil, let done = PerfSignposts.time(of: .runDone) else {
                emitStatus(r.scenario, "[belum diukur]", tab.previewError ?? "the run did not finish")
                continue
            }
            var metrics: [String: Double] = ["duration_ms": (done - started) * 1000,
                                             "rows_count": Double(tab.preview?.rowCount ?? 0),
                                             "footprint_delta_bytes": Double(Int64(settled) - Int64(baseline)),
                                             "peak_footprint_delta_bytes": Double(Int64(peak) - Int64(baseline))]
            if let painted = PerfSignposts.time(of: .firstPaint) { metrics["ttfr_ms"] = (painted - started) * 1000 }
            metrics["rows_per_s"] = Double(tab.preview?.rowCount ?? 0) / max(done - started, 0.001)
            // R-29: what the store holds and what it spilled, next to the footprint.
            let stats = RustEngine.storeStats()
            metrics["spilled_bytes"] = Double(stats?.spilledBytes ?? 0)
            metrics["budget_bytes"] = Double(stats?.budgetBytes ?? 0)
            metrics.merge(stageMetrics(PerfSignposts.stampSnapshot())) { _, new in new }
            emit(r.scenario, metrics, extra: r.extra(pass: pass))
        }
    }

    /// Run, press Stop, and time how long the app takes to be done with it.
    @MainActor
    private static func cancel(_ r: DatabaseRun, repeats: Int) async {
        let tab = r.tab
        for pass in 0..<repeats {
            log("repeat \(pass + 1)/\(repeats)")
            tab.preview = nil
            await sleep(1.0)
            PerfSignposts.clearStages()
            r.model.preview(tab)
            guard PerfSignposts.time(of: .run) != nil else {
                emitStatus(r.scenario, "[belum diukur]", tab.previewError ?? "the run did not start")
                continue
            }
            switch r.spec.flow {
            case .cancelAfter(let seconds):
                await sleep(seconds)
            case .cancelMidStream(let rows):
                _ = await wait(timeout: 60) { !tab.previewing || max(tab.fetchedRows, tab.activeResult?.fetched ?? 0) >= rows }
            default:
                break
            }
            // A run that ended by itself has nothing to cancel, and its number would be a lie.
            guard tab.previewing else {
                emitStatus(r.scenario, "[belum diukur]", tab.previewError ?? "the run ended before Stop was pressed")
                continue
            }
            let rowsAtCancel = max(tab.fetchedRows, tab.activeResult?.fetched ?? 0)
            PerfSignposts.clear("cancel", "cancelEnd")
            r.model.cancelPreview(tab)
            _ = await wait(timeout: 60) { !tab.previewing }
            guard let began = PerfSignposts.time(of: "cancel"), let ended = PerfSignposts.time(of: "cancelEnd") else {
                emitStatus(r.scenario, "[belum diukur]", "the run never ended")
                continue
            }
            var metrics = ["cancel_ms": (ended - began) * 1000]
            if let engine = PerfSignposts.time(of: .engineDone) { metrics["cancel_engine_done_ms"] = (engine - began) * 1000 }
            if case .cancelMidStream = r.spec.flow { metrics["rows_at_cancel_count"] = Double(rowsAtCancel) }
            emit(r.scenario, metrics, extra: r.extra(pass: pass))
        }
    }

    /// One Run that `start` presses: milliseconds to the first row on screen, and to the end of the run.
    /// Nil when the run did not start, failed, or never painted.
    @MainActor
    private static func timedRun(_ tab: QueryTab, start: () -> Void) async -> (ttfr: Double, total: Double)? {
        PerfSignposts.clearStages()
        start()
        guard let began = PerfSignposts.time(of: .run) else { return nil }
        guard await wait(timeout: 540, { !tab.previewing }), tab.previewError == nil,
              await wait(timeout: 10, { PerfSignposts.time(of: .firstPaint) != nil }),
              let painted = PerfSignposts.time(of: .firstPaint), let done = PerfSignposts.time(of: .runDone) else { return nil }
        return ((painted - began) * 1000, (done - began) * 1000)
    }

    /// The E4 question, asked of one session: what does a Run cost right after a capped one? Per sample,
    /// an uncapped Run twice (so the second is a warm re-run), the capped Run after a finished one, the
    /// same capped Run again, and a server sort of that result. See `rerunMetrics`.
    @MainActor
    private static func rerun(_ r: DatabaseRun, repeats: Int) async {
        let tab = r.tab
        let model = r.model
        let uncapped = 500_000
        let cap = max(1_000, r.spec.cap)
        AppModel.rowLimitCeiling = max(AppModel.rowLimitCeiling, uncapped)
        let rtt = r.spec.viaToxiproxy ? rttMS : 0
        for pass in 0..<repeats {
            log("repeat \(pass + 1)/\(repeats)")
            func failed(_ step: String) {
                emitStatus(r.scenario, "[belum diukur]", "\(step) did not finish: \(tab.previewError ?? "it never painted a row")")
            }
            tab.rowLimit = uncapped
            tab.preview = nil
            await sleep(1.0)
            guard await timedRun(tab, start: { model.preview(tab) }) != nil else { failed("the first uncapped Run"); continue }
            await sleep(0.5)
            guard let baseline = await timedRun(tab, start: { model.preview(tab) }) else { failed("the warm uncapped Run"); continue }
            await sleep(0.5)
            tab.rowLimit = cap
            guard let warm = await timedRun(tab, start: { model.preview(tab) }) else { failed("the capped Run"); continue }
            await sleep(0.5)
            guard let again = await timedRun(tab, start: { model.preview(tab) }) else { failed("the capped Run again"); continue }
            await sleep(0.5)
            guard let column = tab.preview?.columns.first else { failed("the capped Run (it has no columns)"); continue }
            guard let sorted = await timedRun(tab, start: {
                model.sortOnServer(tab, column: column, source: 0, direction: .ascending)
            }) else { failed("the server sort"); continue }
            emit(r.scenario, rerunMetrics(uncapped: baseline.ttfr, warm: warm.ttfr, capped: again.ttfr,
                                          sort: sorted.ttfr, rttMS: rtt),
                 extra: r.extra(pass: pass))
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
