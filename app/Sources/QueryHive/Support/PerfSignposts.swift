import AppKit
import Foundation
import os

/// Signposts for the five intervals the performance plan (§4, item 0.4) measures, plus a small
/// stamp table that `--bench` reads: the named stages of a Run (`Stage`) and the sub-intervals of a
/// keystroke (`part`). Nothing here changes what the app does: a signpost that no one is recording
/// costs a check inside `os`, and the stamp and part tables are only written while `recording` is
/// on, which only `BenchMode` turns on.
///
/// Read them in Instruments (Points of Interest is not needed; the category is `perf`):
///
///     log stream --predicate 'subsystem == "<bundle id>" AND category == "perf"'
///
/// - `run`: `runPreview` starts it; a `firstRowsEvent` event marks the first batch of rows reaching
///   the main queue; the first grid row appearing on screen ends it.
/// - `keystroke`: the key going down in the editor, to the run-loop turn that displays the change.
/// - `apply`: the analysis repaint starting, to the run-loop turn that displays the new colours.
///   A keystroke that needs no repaint never opens one; `--bench type-*` waits for it instead
///   of sleeping past it.
/// - `cancel`: Stop pressed, to the run's exit reaching the app. The engine drops a stopped run's
///   `done` before the app sees it (`Sink` checks `isStopped`), so the exit is the last thing the
///   user can wait on; `engine.done` is stamped where the sink still sees it.
/// - `launch`: process start (from `kinfo_proc`) to the first window update after launch.
///
/// Main-thread state throughout, except `engineEvent`, which the FFI thread calls.
enum PerfSignposts {
    static let subsystem = Bundle.main.bundleIdentifier ?? "QueryHive"
    static let signposter = OSSignposter(subsystem: subsystem, category: "perf")
    static let logger = Logger(subsystem: subsystem, category: "perf")

    // MARK: Stamps, for `--bench`

    /// On only under `--bench`. A plain `Bool` read: it is set once, before any thread that reads it
    /// exists.
    static var recording = false
    private static let lock = NSLock()
    private static var stamps: [String: CFAbsoluteTime] = [:]

    static func stamp(_ name: String, onlyFirst: Bool = false) {
        guard recording else { return }
        let now = CFAbsoluteTimeGetCurrent()
        lock.lock()
        if !onlyFirst || stamps[name] == nil { stamps[name] = now }
        lock.unlock()
    }

    static func time(of name: String) -> CFAbsoluteTime? {
        lock.lock()
        defer { lock.unlock() }
        return stamps[name]
    }

    static func clear(_ names: String...) {
        lock.lock()
        names.forEach { stamps[$0] = nil }
        lock.unlock()
    }

    // MARK: Stages

    /// The stamps one Run leaves, in the order it passes them (W8-T2 decision §3). `--bench` reads
    /// them and prints every offset from `run` and the four deltas the hypotheses H1 to H5 name.
    /// A call site only has to say `PerfSignposts.stamp(.gridAttached)`; the bench does the rest.
    ///
    /// The `engine.*` ones are stamped on the FFI thread by `engineEvent`; every other stage is
    /// main-thread state. A stage nobody stamps is left out of the line, so a lane that has not
    /// landed its call site yet does not break the others.
    enum Stage: String, CaseIterable {
        case run
        case engineColumns = "engine.columns"
        case engineProgress = "engine.progress"
        /// The `columns` event handled on the main queue (AppModel+Run).
        case columns
        /// The grid's table view attached to a window (`GridTableView.viewDidMoveToWindow`).
        case gridAttached
        /// The first batch of rows seen by the store's poll (`StoreRows`, via `firstRowsEvent`).
        case firstRows
        /// The first `draw` that has rows to paint, before the flush.
        case firstDraw
        case engineDone = "engine.done"
        case runDone
        case firstPaint

        /// The name a metric uses: `engine.done` -> `engine_done`, `gridAttached` -> `grid_attached`.
        var metric: String { PerfSignposts.snakeCase(rawValue) }
    }

    static func stamp(_ stage: Stage, onlyFirst: Bool = false) { stamp(stage.rawValue, onlyFirst: onlyFirst) }

    static func time(of stage: Stage) -> CFAbsoluteTime? { time(of: stage.rawValue) }

    /// Forgets every stage stamp, the engine's included. `runBegin` does it for each Run, and a
    /// bench repeat does it again before it presses Run, so an `onlyFirst` stamp can never be the
    /// last repeat's.
    static func clearStages() {
        lock.lock()
        stamps = stamps.filter { name, _ in !name.hasPrefix("engine.") && Stage(rawValue: name) == nil }
        lock.unlock()
    }

    /// A copy of the stamp table, for the bench to turn into metrics.
    static func stampSnapshot() -> [String: CFAbsoluteTime] {
        lock.lock()
        defer { lock.unlock() }
        return stamps
    }

    /// `gridAttached` -> `grid_attached`, `engine.done` -> `engine_done`, `apply.layout` ->
    /// `apply_layout`: a metric name is lower-case words joined by `_`.
    static func snakeCase(_ name: String) -> String {
        var out = ""
        var separator = false
        var previous: Character?
        for character in name {
            guard character.isLetter || character.isNumber else { separator = true; continue }
            let wordBreak = character.isUppercase && previous.map { $0.isLowercase || $0.isNumber } == true
            if !out.isEmpty, separator || wordBreak { out.append("_") }
            out.append(contentsOf: character.lowercased())
            separator = false
            previous = character
        }
        return out
    }

    // MARK: Parts

    /// The sub-intervals inside one keystroke or apply turn (W8-F3's attribution): how long each
    /// named part took, summed over the turn, and how many times it ran. Free-form names are
    /// fine; these are the ones the decision lists, so two call sites cannot spell one differently.
    ///
    ///     PerfSignposts.part(PerfSignposts.Part.layout) { layoutManager.ensureLayout(forCharacterRange: r) }
    ///     PerfSignposts.count(PerfSignposts.Part.tempAttrAdd)
    ///
    /// A call is a plain `recording` check unless `--bench` is on. `--bench type-*` resets the table
    /// before each key and prints `part_<name>_p50_ms`, `_p99_ms` and the mean `_count` per key.
    enum Part {
        static let replaceAndRuler = "replaceAndRuler"
        static let inherit = "inherit"
        static let layout = "layout"
        static let draw = "draw"
        static let outlineApply = "outlineApply"
        static let tempAttrAdd = "tempAttrAdd"
        static let tempAttrRemove = "tempAttrRemove"
    }

    struct PartTotal: Equatable {
        var ms = 0.0
        var count = 0
    }

    private static var parts: [String: PartTotal] = [:]

    private static func addPart(_ name: String, ms: Double, count: Int) {
        lock.lock()
        var total = parts[name] ?? PartTotal()
        total.ms += ms
        total.count += count
        parts[name] = total
        lock.unlock()
    }

    /// Times `body` into the part `name`. Main-thread or not, the table is locked.
    static func part<T>(_ name: String, _ body: () throws -> T) rethrows -> T {
        guard recording else { return try body() }
        let started = CFAbsoluteTimeGetCurrent()
        defer { addPart(name, ms: (CFAbsoluteTimeGetCurrent() - started) * 1000, count: 1) }
        return try body()
    }

    /// Counts `by` occurrences of `name` without timing them, such as temporary attributes written.
    static func count(_ name: String, by amount: Int = 1) {
        guard recording else { return }
        addPart(name, ms: 0, count: amount)
    }

    static func partsReset() {
        lock.lock()
        parts = [:]
        lock.unlock()
    }

    static func partsSnapshot() -> [String: PartTotal] {
        lock.lock()
        defer { lock.unlock() }
        return parts
    }

    // MARK: run -> firstRowsEvent -> firstPaint

    private static var runState: OSSignpostIntervalState?
    private static var paintQueued = false

    static func runBegin() {
        if let state = runState { signposter.endInterval("run", state, "superseded") }
        clearStages()
        paintQueued = false
        stamp("run")
        runState = signposter.beginInterval("run", id: signposter.makeSignpostID())
    }

    /// The first batch of rows, as it reaches the main queue.
    static func firstRowsEvent() {
        guard runState != nil else { return }
        stamp("firstRows", onlyFirst: true)
        signposter.emitEvent("firstRowsEvent")
    }

    static func runDone() { stamp("runDone") }

    /// Called from the grid's first row appearing. Ends the interval on the next main-queue turn,
    /// which is as close to "on screen" as the view tree lets us stand; under `--bench` a
    /// `CATransaction.flush()` first pushes the frame to the render server.
    static func firstPaint() {
        guard runState != nil, !paintQueued else { return }
        paintQueued = true
        DispatchQueue.main.async {
            if recording { CATransaction.flush() }
            stamp("firstPaint", onlyFirst: true)
            if let state = runState { signposter.endInterval("run", state) }
            runState = nil
        }
    }

    /// Any event the engine emits, on the FFI thread, before the main-queue hop and before the
    /// stopped-run filter.
    static func engineEvent(_ name: String) {
        guard recording else { return }
        stamp("engine.\(name)", onlyFirst: name != "done")
    }

    // MARK: cancel

    private static var cancelState: OSSignpostIntervalState?

    /// Only when something is running: the callers check, because an interval that begins with
    /// nothing to cancel would never end.
    static func cancelBegin() {
        guard cancelState == nil else { return }
        clear("cancel", "cancelEnd", "engine.done")
        stamp("cancel")
        cancelState = signposter.beginInterval("cancel", id: signposter.makeSignpostID())
    }

    static func cancelEnd() {
        guard let state = cancelState else { return }
        stamp("cancelEnd")
        signposter.endInterval("cancel", state)
        cancelState = nil
    }

    // MARK: keystroke

    private static var keyState: OSSignpostIntervalState?
    private static var keyStart: CFAbsoluteTime = 0
    private static var keyObserver: CFRunLoopObserver?
    /// The last finished keystroke, key-down (or `keystrokeBegin`) to display, in milliseconds.
    private(set) static var lastKeystrokeMS: Double?
    /// The last `textDidChange`, in milliseconds.
    private(set) static var lastDidChangeMS: Double?

    /// Key down. Costs nothing unless something is listening (`--bench`, or Instruments attached).
    /// The interval ends in the run-loop turn that displays the change: an observer after Core
    /// Animation's own commit observer (order 2_000_000), fired once.
    static func keystrokeBegin() {
        guard recording || signposter.isEnabled, keyState == nil else { return }
        lastKeystrokeMS = nil
        keyStart = CFAbsoluteTimeGetCurrent()
        keyState = signposter.beginInterval("keystroke", id: signposter.makeSignpostID())
        let observer = CFRunLoopObserverCreateWithHandler(
            nil, CFRunLoopActivity.beforeWaiting.rawValue, false, 2_000_001
        ) { observer, _ in
            if let state = keyState { signposter.endInterval("keystroke", state) }
            lastKeystrokeMS = (CFAbsoluteTimeGetCurrent() - keyStart) * 1000
            keyState = nil
            if let observer { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
            keyObserver = nil
        }
        keyObserver = observer
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
    }

    /// Drops an open keystroke, so the next one starts clean. For `--bench`, between inserts.
    static func keystrokeReset() {
        if let state = keyState { signposter.endInterval("keystroke", state, "reset") }
        if let observer = keyObserver { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
        keyState = nil
        keyObserver = nil
        lastKeystrokeMS = nil
        lastDidChangeMS = nil
    }

    /// `textDidChange` measures itself: begin here, hand the token back at the end.
    static func didChangeBegin() -> CFAbsoluteTime { CFAbsoluteTimeGetCurrent() }

    // MARK: apply

    private static var applyState: OSSignpostIntervalState?
    private static var applyStart: CFAbsoluteTime = 0
    private static var applyObserver: CFRunLoopObserver?
    /// The last finished repaint, apply start to display, in milliseconds.
    private(set) static var lastApplyMS: Double?

    /// The repaint starts; it ends in the run-loop turn that displays it, like `keystroke`.
    static func applyBegin() {
        guard recording || signposter.isEnabled, applyState == nil else { return }
        lastApplyMS = nil
        applyStart = CFAbsoluteTimeGetCurrent()
        applyState = signposter.beginInterval("apply", id: signposter.makeSignpostID())
        let observer = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue, false, 2_000_001) { observer, _ in
            if let state = applyState { signposter.endInterval("apply", state) }
            lastApplyMS = (CFAbsoluteTimeGetCurrent() - applyStart) * 1000
            applyState = nil
            if let observer { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
            applyObserver = nil
        }
        applyObserver = observer
        CFRunLoopAddObserver(CFRunLoopGetMain(), observer, .commonModes)
    }

    /// Drops an open repaint, so the next one starts clean. For `--bench`, between inserts.
    static func applyReset() {
        if let state = applyState { signposter.endInterval("apply", state, "reset") }
        if let observer = applyObserver { CFRunLoopRemoveObserver(CFRunLoopGetMain(), observer, .commonModes) }
        applyState = nil
        applyObserver = nil
        lastApplyMS = nil
    }

    static func didChangeEnd(started: CFAbsoluteTime) {
        lastDidChangeMS = (CFAbsoluteTimeGetCurrent() - started) * 1000
    }

    // MARK: launch

    private static var launchState: OSSignpostIntervalState?
    private static var launchObserver: NSObjectProtocol?
    /// Set by `--bench launch` to receive the number instead of only the log.
    static var onLaunchFrame: ((Double) -> Void)?

    static func launchBegin() {
        launchState = signposter.beginInterval("launch", id: signposter.makeSignpostID())
    }

    /// The moment the process was created, from the kernel. Includes dyld and everything before
    /// `main`, which a timestamp taken in `main` could not.
    static func processStart() -> CFAbsoluteTime? {
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&mib, 4, &info, &size, nil, 0) == 0 else { return nil }
        let start = info.kp_proc.p_un.__p_starttime
        return CFAbsoluteTime(start.tv_sec) + CFAbsoluteTime(start.tv_usec) / 1e6
            - kCFAbsoluteTimeIntervalSince1970
    }

    /// Called once from `applicationDidFinishLaunching`: the first visible window to update after
    /// launch, plus one main-queue turn, is the first frame.
    static func watchFirstFrame() {
        launchObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didUpdateNotification, object: nil, queue: .main
        ) { note in
            guard let window = note.object as? NSWindow, window.isVisible, window.canBecomeMain else { return }
            if let observer = launchObserver { NotificationCenter.default.removeObserver(observer) }
            launchObserver = nil
            DispatchQueue.main.async {
                if recording { CATransaction.flush() }
                if let state = launchState { signposter.endInterval("launch", state) }
                launchState = nil
                let ms = processStart().map { (CFAbsoluteTimeGetCurrent() - $0) * 1000 }
                if let ms { logger.notice("launch to first frame: \(ms, format: .fixed(precision: 1)) ms") }
                if let ms { onLaunchFrame?(ms) }
            }
        }
    }

    // MARK: TextKit mode (plan item 0.9)

    /// Which TextKit the editor's text view runs on, once known: 2, or 1 after a fallback.
    private(set) static var textKitMode: Int?
    private static var textKitWatched = false

    /// Right after the text view is created, before anything touches its `layoutManager`. Touching
    /// that property is what drops a TextKit 2 view to TextKit 1, so the notification carries the
    /// stack of whoever did it.
    static func watchTextKit(_ textView: NSTextView) {
        guard !textKitWatched else { return }
        textKitWatched = true
        logger.notice("editor text view created on TextKit \(textView.textLayoutManager != nil ? 2 : 1)")
        NotificationCenter.default.addObserver(
            forName: NSTextView.willSwitchToNSLayoutManagerNotification, object: textView, queue: nil
        ) { _ in
            let stack = Thread.callStackSymbols.dropFirst(2).prefix(8).joined(separator: " | ")
            logger.notice("editor text view fell back to TextKit 1 at: \(stack, privacy: .public)")
        }
    }

    /// At the end of the editor's setup: the mode the view will actually run in.
    static func logTextKit(_ textView: NSTextView) {
        let mode = textView.textLayoutManager != nil ? 2 : 1
        if textKitMode == nil { logger.notice("editor text view runs on TextKit \(mode)") }
        textKitMode = mode
    }
}
