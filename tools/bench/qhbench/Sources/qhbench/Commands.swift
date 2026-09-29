import Foundation
import AppKit
import ApplicationServices
import Darwin

/// What every subcommand needs before it starts. Nothing here requests a permission: a missing one
/// becomes one sample line with the contract's "tidak diukur (izin OS)" status.
struct Ctx {
    let args: Args
    let profile: Profile
    let out: Out
    let scenario: String
    let app: String
}

/// Text describing the resolved target, appended to every sample's notes.
nonisolated(unsafe) var targetNoteText = ""

/// Validates the scenario, then permissions, then the `--drive` opt-in, all before any app is touched.
func makeCtx(_ args: Args, defaultScenario: String, allowed: [String], needScreen: Bool, needAX: Bool, drives: Bool) -> Ctx? {
    let appName = args.string("app") ?? "queryhive"
    let scenario = args.string("scenario") ?? defaultScenario
    var out = Out(scenario: scenario, app: appName)
    out.label = args.string("label")
    guard let profile = Profile.load(app: appName, dirOverride: args.string("profiles-dir")) else {
        out.status(Status.notMeasured, notes: "no profile profiles/\(appName).json found; use --app queryhive|tablepro or --profiles-dir")
        return nil
    }
    guard allowed.contains(scenario) else {
        out.status(Status.notMeasured, notes: "unknown scenario \(scenario) for this subcommand; valid: \(allowed.joined(separator: ", "))")
        return nil
    }
    let perms = PermissionState.current()
    var missing: [String] = []
    if needScreen && !perms.screenRecording { missing.append("Screen Recording") }
    if needAX && !perms.accessibility { missing.append("Accessibility") }
    if !missing.isEmpty {
        out.status(Status.permission, notes: "missing: " + missing.joined(separator: ", "))
        return nil
    }
    if drives && !args.has("drive") {
        let msg = "refused: this subcommand activates \(profile.appName) and sends input to it; pass --drive to allow that (see README). Nothing was touched"
        logErr("qhbench: " + msg)
        out.status(Status.notMeasured, notes: msg)
        return nil
    }
    return Ctx(args: args, profile: profile, out: out, scenario: scenario, app: appName)
}

private func refuse(_ ctx: Ctx, _ why: String) {
    logErr("qhbench: " + why)
    ctx.out.status(Status.notMeasured, notes: why)
}

private func bundlePath(_ a: NSRunningApplication) -> String {
    a.bundleURL?.resolvingSymlinksInPath().path ?? "?"
}

/// Exactly one target process. `--pid` must belong to the profile's bundle id. Without it, more than
/// one running copy of the bundle id is refused unless `--path` picks one bundle. No name fallback.
func resolveTarget(_ ctx: Ctx) -> NSRunningApplication? {
    let bid = ctx.profile.bundleId
    if ctx.args.has("pid") {
        guard let pid = ctx.args.string("pid").flatMap(Int.init), let app = NSRunningApplication(processIdentifier: pid_t(pid)) else {
            refuse(ctx, "--pid needs a running process id"); return nil
        }
        guard app.bundleIdentifier == bid else {
            refuse(ctx, "pid \(pid) is \(app.bundleIdentifier ?? "unknown"), not \(bid)"); return nil
        }
        return app
    }
    var instances = NSRunningApplication.runningApplications(withBundleIdentifier: bid)
    if let p = ctx.args.string("path") {
        let want = URL(fileURLWithPath: p).resolvingSymlinksInPath().path
        instances = instances.filter { bundlePath($0) == want }
    }
    if instances.isEmpty {
        refuse(ctx, "no running \(bid) instance found; start it first"); return nil
    }
    if instances.count > 1 {
        let list = instances.map { "pid \($0.processIdentifier) \(bundlePath($0))" }.joined(separator: "; ")
        refuse(ctx, "\(instances.count) instances of \(bid) are running (\(list)); pass --pid or --path"); return nil
    }
    return instances[0]
}

private func rectText(_ r: CGRect) -> String {
    "\(Int(r.minX)),\(Int(r.minY)) \(Int(r.width))x\(Int(r.height))"
}

/// Resolves the target, brings it to front (an opt-in action), resolves regions and prints them.
private func acquire(_ ctx: Ctx) -> (NSRunningApplication, Regions)? {
    guard let app = resolveTarget(ctx) else { return nil }
    let pid = app.processIdentifier
    Input.guardPid = pid
    app.activate()
    var front = false
    for _ in 0..<40 {
        if Input.frontmostOK() { front = true; break }
        sleepMs(50)
    }
    guard front else { refuse(ctx, "pid \(pid) did not become the frontmost app; nothing was sent"); return nil }
    sleepMs(300)
    guard let regions = resolveRegions(profile: ctx.profile, app: app) else {
        refuse(ctx, "no window found through the accessibility tree; nothing was sent"); return nil
    }
    var note = "pid \(pid); bundle \(bundlePath(app)); window \(rectText(regions.windowFrame)); grid \(rectText(regions.grid))"
    note += regions.gridFromFallback ? " (window-fraction fallback)" : ""
    note += "; editor " + (regions.editor.map { rectText($0.1) } ?? "not found")
    targetNoteText = note
    logErr("qhbench: target " + note)
    return (app, regions)
}

private func profileNote(_ ctx: Ctx, _ r: Regions? = nil) -> String {
    var parts: [String] = [targetNoteText]
    if !ctx.profile.verified { parts.append("profile unverified") }
    return parts.filter { !$0.isEmpty }.joined(separator: "; ")
}

private let focusLost = "the target lost focus, run aborted; nothing more was sent"

// MARK: - ttfr

func cmdTtfr(_ args: Args) async {
    let scenarios = ["ttfr-s1-1k", "ttfr-s1-10k", "ttfr-s2-500k", "ttfr-s3-rtt30", "ttfr-s4-first-run"]
    guard let ctx = makeCtx(args, defaultScenario: "ttfr-s1-1k", allowed: scenarios, needScreen: true, needAX: true, drives: true),
          let (app, r) = acquire(ctx) else { return }
    guard let editor = r.editor else { refuse(ctx, "editor not found through the accessibility tree"); return }
    let runs = args.int("runs", 20)
    let timeoutMs = args.double("timeout-ms", 5000)
    let settleMs = args.double("settle-ms", 1500)
    let tolerance = args.int("match-tolerance", 4)
    // A different result every run, so the grid always has something new to paint. `{n}` is the run number.
    let template = args.string("sql") ?? "SELECT {n} AS qhbench_run, * FROM wide_500k"
    do {
        let watcher = try await FrameWatcher.start(pid: app.processIdentifier, region: r.grid)
        watcher.minChangedPixels = args.int("min-changed", 4)
        for n in 1...max(1, runs) {
            let sql = template.replacingOccurrences(of: "{n}", with: String(n))
            AX.focus(editor.0)
            guard AXUIElementSetAttributeValue(editor.0, kAXValueAttribute as CFString, sql as CFString) == .success else {
                refuse(ctx, "could not set the editor text through AX; no Run was sent"); break
            }
            sleepMs(300)
            watcher.arm(collect: true)
            guard let t0 = Input.chord(ctx.profile.run) else { refuse(ctx, focusLost); break }
            if let hit = watcher.waitForChange(timeoutMs: timeoutMs) {
                sleepMs(settleMs)
                let frames = watcher.collectedFrames()
                var m: [String: Any] = ["ttfr_first_change_ms": Clock.ms(from: t0, to: hit)]
                // The settled result is the last frame; TTFR is the first frame that already shows it.
                if let final = frames.last?.sample,
                   let match = frames.first(where: { FrameWatcher.differing($0.sample, final) <= tolerance }) {
                    m["ttfr_ms"] = Clock.ms(from: t0, to: match.pts)
                }
                ctx.out.metrics(m, notes: "run \(n) of \(runs); ttfr_ms is the first frame matching the settled grid, ttfr_first_change_ms the first differing frame; " + profileNote(ctx, r))
            } else {
                _ = watcher.collectedFrames()
                ctx.out.status(Status.notMeasured, notes: "the grid region never changed within \(Int(timeoutMs)) ms after Run \(n); " + profileNote(ctx, r))
                sleepMs(settleMs)
            }
        }
        await watcher.stop()
    } catch {
        ctx.out.status(Status.notMeasured, notes: "capture failed: \(error)")
    }
}

// MARK: - type

func cmdType(_ args: Args) async {
    guard let ctx = makeCtx(args, defaultScenario: "type-10k", allowed: ["type-10k", "type-2m"], needScreen: true, needAX: true, drives: true),
          let (app, r) = acquire(ctx) else { return }
    guard let editor = r.editor else { refuse(ctx, "editor not found through the accessibility tree"); return }
    let n = args.int("chars", 40)
    let minChanged = args.int("min-changed", 200)
    let chars = AX.placeCaretInMiddle(editor.0)
    sleepMs(500)
    guard let caret = AX.caretRect(editor.0) else {
        refuse(ctx, "caret bounds unavailable through AXBoundsForRange; nothing was typed"); return
    }
    // The band to the right of the caret: the typed glyph lands here, the caret's own blink mostly does not.
    let bandRight = min(editor.1.maxX, caret.minX + Double(n) * 10)
    let region = CGRect(x: caret.minX + 3, y: caret.minY, width: max(20, bandRight - caret.minX - 3), height: max(12, caret.height))
    do {
        let watcher = try await FrameWatcher.start(pid: app.processIdentifier, region: region)
        watcher.minChangedPixels = minChanged
        var samples: [Double] = []
        var misses = 0
        var aborted = false
        for _ in 0..<n {
            watcher.arm()
            guard let t0 = Input.type("M") else { aborted = true; break }
            if let hit = watcher.waitForChange(timeoutMs: 1000) { samples.append(Clock.ms(from: t0, to: hit)) } else { misses += 1 }
            sleepMs(args.double("gap-ms", 80))
        }
        await watcher.stop()
        if aborted { refuse(ctx, focusLost) }
        if samples.isEmpty {
            if !aborted { ctx.out.status(Status.notMeasured, notes: "no typed glyph was seen in \(n) tries; lower --min-changed") }
        } else {
            let note = "typed \(samples.count + misses) chars of M into the document, left in place (undo them); caret in the middle of \(chars.map(String.init) ?? "unknown") chars; min-changed \(minChanged) px; \(misses) misses; " + profileNote(ctx, r)
            ctx.out.metrics(["input_to_photon_ms": samples], notes: note)
        }
    } catch {
        ctx.out.status(Status.notMeasured, notes: "capture failed: \(error)")
    }
}

// MARK: - scroll

func cmdScroll(_ args: Args) async {
    guard let ctx = makeCtx(args, defaultScenario: "scroll-30x1m", allowed: ["scroll-30x1m", "scroll-500x10k"], needScreen: true, needAX: true, drives: true),
          let (app, r) = acquire(ctx) else { return }
    let seconds = args.double("seconds", 5)
    let pxPerSec = args.double("speed", 4800)
    let hz = args.double("display-hz", 120)
    let both = ctx.scenario == "scroll-500x10k" || args.has("both-axes")
    let center = CGPoint(x: r.grid.midX, y: r.grid.midY)
    let trace = FileManager.default.temporaryDirectory.appendingPathComponent("qhbench-\(getpid())-\(Int(Date().timeIntervalSince1970)).trace").path

    do {
        let watcher = try await FrameWatcher.start(pid: app.processIdentifier, region: r.grid)
        watcher.minChangedPixels = 1

        // Animation Hitches attached to the target; its recording overlaps the scroll.
        let xc = Process()
        xc.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
        xc.arguments = ["xctrace", "record", "--template", "Animation Hitches", "--attach", String(app.processIdentifier),
                        "--time-limit", "\(Int(seconds) + 4)s", "--output", trace, "--no-prompt"]
        xc.standardOutput = FileHandle.nullDevice
        xc.standardError = FileHandle.nullDevice
        var tracing = true
        do { try xc.run() } catch { tracing = false }
        if tracing { sleepMs(2500) }

        guard Input.moveMouse(to: center) else {
            if tracing { xc.terminate() }
            await watcher.stop(); refuse(ctx, focusLost); return
        }
        sleepMs(100)
        watcher.startRecording()
        let start = Clock.nowNs()
        let tick = 1000.0 / hz
        var next = Double(start)
        let dyPerTick = Int32((pxPerSec / hz).rounded())
        var aborted = false
        while Clock.ms(from: start, to: Clock.nowNs()) < seconds * 1000 {
            let elapsed = Clock.ms(from: start, to: Clock.nowNs())
            let horizontal = both && elapsed > seconds * 500
            if !Input.scroll(dx: horizontal ? dyPerTick : 0, dy: horizontal ? 0 : dyPerTick, at: center) { aborted = true; break }
            next += tick * 1e6
            let wait = (next - Double(Clock.nowNs())) / 1e6
            if wait > 0 { sleepMs(wait) }
        }
        let frames = watcher.stopRecording()
        await watcher.stop()
        if aborted {
            if tracing { xc.terminate() }
            refuse(ctx, focusLost); return
        }

        var m: [String: Any] = [:]
        var notes: [String] = []
        let intervals = zip(frames, frames.dropFirst()).map { Clock.ms(from: $0, to: $1) }
        let expected = 1000.0 / hz
        if intervals.count > 10 {
            m["frame_p99_ms"] = percentile(intervals, 0.99)
            m["frame_p50_ms"] = percentile(intervals, 0.50)
            m["frame_count"] = frames.count
        }
        var hitchFromTrace: Double?
        if tracing {
            xc.waitUntilExit()
            hitchFromTrace = parseHitchMs(trace: trace).map { $0 / seconds }
        }
        if let h = hitchFromTrace {
            m["hitch_ms_per_s"] = h
            notes.append("hitch from xctrace Animation Hitches")
        } else if intervals.count > 10 {
            m["hitch_ms_per_s"] = intervals.reduce(0) { $0 + max(0, $1 - expected) } / seconds
            notes.append("hitch approximated from ScreenCaptureKit frame intervals over \(expected.rounded()) ms; the xctrace hitch table could not be read (xctrace attach needs Developer Tools access)")
        }
        try? FileManager.default.removeItem(atPath: trace)
        notes.append(profileNote(ctx, r))
        notes.append("wheel \(Int(pxPerSec)) px/s for \(seconds) s; mouse moved to the grid centre")
        if m.isEmpty {
            ctx.out.status(Status.notMeasured, notes: "no frames captured while scrolling; " + profileNote(ctx, r))
        } else {
            ctx.out.metrics(m, notes: notes.joined(separator: "; "))
        }
    } catch {
        ctx.out.status(Status.notMeasured, notes: "capture failed: \(error)")
    }
}

/// Sum of hitch durations in ms from an Animation Hitches trace, or nil when the table cannot be read.
/// The export layout (`hitches` schema, first `duration` column in ns) is UNVERIFIED on this machine.
func parseHitchMs(trace: String) -> Double? {
    let xpath = "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"hitches\"]"
    let r = sh("/usr/bin/xcrun", ["xctrace", "export", "--input", trace, "--xpath", xpath])
    guard r.status == 0, r.out.contains("<node") || r.out.contains("<row") else { return nil }
    guard let rowRe = try? NSRegularExpression(pattern: "<row>(.*?)</row>", options: [.dotMatchesLineSeparators]),
          let durRe = try? NSRegularExpression(pattern: "<duration[^>]*>(\\d+)</duration>") else { return nil }
    let ns = r.out as NSString
    var totalNs = 0.0
    for row in rowRe.matches(in: r.out, range: NSRange(location: 0, length: ns.length)) {
        let body = ns.substring(with: row.range(at: 1)) as NSString
        if let d = durRe.firstMatch(in: body as String, range: NSRange(location: 0, length: body.length)),
           let v = Double(body.substring(with: d.range(at: 1))) {
            totalNs += v
        }
    }
    return totalNs / 1e6
}

// MARK: - cancel

func cmdCancel(_ args: Args) async {
    struct Plan { let kind: String; let sql: String; let pattern: String; let midStream: Bool }
    // `{m}` is this run's marker comment, so no statement or poll can match an earlier run.
    let plans: [String: Plan] = [
        "cancel-pg-sleep": Plan(kind: "pg", sql: "SELECT pg_sleep(30) /* {m} */", pattern: "%/* {m} */%", midStream: false),
        "cancel-mysql-sleep": Plan(kind: "mysql", sql: "SELECT SLEEP(30) /* {m} */", pattern: "%/* {m} */%", midStream: false),
        "cancel-stream-wide": Plan(kind: "pg", sql: "SELECT * FROM wide_500k /* {m} */", pattern: "%/* {m} */%", midStream: true),
        "cancel-trino-heavy": Plan(kind: "trino",
                                   sql: "SELECT /* {m} */ count(*) FROM tpch.sf1.lineitem a JOIN tpch.sf1.lineitem b ON a.l_orderkey = b.l_orderkey",
                                   pattern: "", midStream: false),
    ]
    // The scenario is validated inside makeCtx, before any app is resolved or activated.
    guard let ctx = makeCtx(args, defaultScenario: "cancel-pg-sleep", allowed: Array(plans.keys), needScreen: false, needAX: true, drives: true),
          let plan = plans[ctx.scenario], let (_, r) = acquire(ctx) else { return }
    guard let editor = r.editor else { refuse(ctx, "editor not found through the accessibility tree"); return }
    let runs = args.int("runs", 10)
    // Unique per invocation and per run: pid and epoch seconds separate invocations, and the
    // marker is matched as the whole delimited comment, so `_1` never matches `_10`.
    let session = "\(getpid())_\(Int(Date().timeIntervalSince1970))"
    for n in 1...max(1, runs) {
        let marker = "qhbench_cancel_\(session)_\(n)"
        let sql = plan.sql.replacingOccurrences(of: "{m}", with: marker)
        let (maybePoller, why) = PollerFactory.make(kind: plan.kind, matchPattern: plan.pattern.replacingOccurrences(of: "{m}", with: marker),
                                                    marker: "/* \(marker) */")
        guard let poller = maybePoller else { refuse(ctx, why ?? "no poller"); return }
        // Put the statement in the editor without typing it, so autocompletion cannot rewrite it.
        AX.focus(editor.0)
        guard AXUIElementSetAttributeValue(editor.0, kAXValueAttribute as CFString, sql as CFString) == .success else {
            refuse(ctx, "could not set the editor text through AX; no Run was sent"); return
        }
        sleepMs(300)
        do { try poller.start() } catch { refuse(ctx, "poller failed: \(error)"); return }
        sleepMs(150)
        guard Input.chord(ctx.profile.run) != nil else { poller.stop(); refuse(ctx, focusLost); return }
        guard poller.waitActive(timeoutMs: 15000) else {
            poller.stop()
            ctx.out.status(Status.notMeasured, notes: "the server never showed the statement running; check the connection tab is on the right server; " + profileNote(ctx, r))
            return
        }
        if plan.midStream { sleepMs(args.double("mid-stream-ms", 800)) }
        guard let t0 = Input.chord(ctx.profile.stop) else { poller.stop(); refuse(ctx, focusLost); return }
        if let gone = poller.waitGone(after: t0, timeoutMs: 10000) {
            ctx.out.metrics(["cancel_ms": Clock.ms(from: t0, to: gone)],
                            notes: "run \(n) of \(runs); marker \(marker); polled every 5 ms in lock-step via \(plan.kind); editor text replaced; " + profileNote(ctx, r))
        } else {
            ctx.out.status(Status.notMeasured, notes: "the server still showed the statement 10 s after Stop; " + profileNote(ctx, r))
        }
        poller.stop()
        sleepMs(1500)
    }
}

// MARK: - launch

func cmdLaunch(_ args: Args) async {
    guard let ctx = makeCtx(args, defaultScenario: "launch-warm", allowed: ["launch-warm", "launch-cold"], needScreen: true, needAX: false, drives: true) else { return }
    let runs = args.int("runs", 10)
    let processName = ctx.profile.processName
    let bid = ctx.profile.bundleId
    let path = args.string("path")
    let quit = args.has("quit")
    if args.has("path") && path == nil { refuse(ctx, "--path needs an .app bundle path"); return }
    if quit && path == nil { refuse(ctx, "--quit needs --path so only that bundle is closed"); return }
    func running() -> [NSRunningApplication] { NSRunningApplication.runningApplications(withBundleIdentifier: bid) }
    func mine() -> [NSRunningApplication] {
        guard let p = path else { return [] }
        let want = URL(fileURLWithPath: p).resolvingSymlinksInPath().path
        return running().filter { bundlePath($0) == want }
    }
    func windowIds() -> Set<Int> {
        let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
        return Set(list.filter { ($0[kCGWindowOwnerName as String] as? String) == processName }
            .compactMap { $0[kCGWindowNumber as String] as? Int })
    }
    for n in 1...max(1, runs) {
        if quit {
            let victims = mine()
            for v in victims { logErr("qhbench: quitting pid \(v.processIdentifier) \(bundlePath(v))") }
            for v in victims { v.terminate() }
            let deadline = Date().addingTimeInterval(8)
            while Date() < deadline, !mine().isEmpty { sleepMs(50) }
            sleepMs(1000)
        }
        if !running().isEmpty {
            let list = running().map { "pid \($0.processIdentifier) \(bundlePath($0))" }.joined(separator: "; ")
            refuse(ctx, "\(bid) is still running (\(list)); close it, or use --quit with --path of that bundle"); return
        }
        let before = windowIds()
        let t0 = Clock.nowNs()
        let open = Process()
        open.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        open.arguments = path.map { [$0] } ?? ["-a", ctx.profile.appName]
        do { try open.run() } catch { refuse(ctx, "open failed: \(error)"); return }
        var seen: UInt64?
        while Clock.ms(from: t0, to: Clock.nowNs()) < 15000 {
            if let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
                for w in list where (w[kCGWindowOwnerName as String] as? String) == processName && (w[kCGWindowLayer as String] as? Int) == 0
                    && !before.contains(w[kCGWindowNumber as String] as? Int ?? -1) {
                    if let b = w[kCGWindowBounds as String] as? [String: Double], (b["Width"] ?? 0) > 200, (b["Height"] ?? 0) > 200,
                       (w[kCGWindowAlpha as String] as? Double ?? 1) > 0 {
                        seen = Clock.nowNs()
                        break
                    }
                }
            }
            if seen != nil { break }
            usleep(1000)
        }
        if let seen {
            var notes = "run \(n) of \(runs); opened \(path ?? "-a " + ctx.profile.appName); first on-screen window of \(processName) via CGWindowList polled every 1 ms; not a proof of an interactive frame"
            if !ctx.profile.verified { notes += "; profile unverified" }
            ctx.out.metrics(["launch_ms": Clock.ms(from: t0, to: seen)], notes: notes)
        } else {
            ctx.out.status(Status.notMeasured, notes: "no window of \(processName) appeared within 15 s")
        }
        sleepMs(1500)
    }
}

// MARK: - memory

func footprintBytes(pid: pid_t) -> UInt64? {
    var info = rusage_info_v4()
    let rc = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
    }
    return rc == 0 ? info.ri_phys_footprint : nil
}

func cmdMemory(_ args: Args) async {
    // Reading another process's footprint needs no TCC permission and no input. Only the Run
    // trigger needs Accessibility, `--drive` and a frontmost target.
    let noRun = args.has("no-run")
    guard let ctx = makeCtx(args, defaultScenario: "mem-500k", allowed: ["mem-500k", "mem-5m"], needScreen: false, needAX: !noRun, drives: !noRun) else { return }
    guard let app = resolveTarget(ctx) else { return }
    let pid = app.processIdentifier
    targetNoteText = "pid \(pid); bundle \(bundlePath(app))"
    logErr("qhbench: target " + targetNoteText)
    let duration = args.double("duration", 20)
    if !noRun {
        Input.guardPid = pid
        app.activate()
        var front = false
        for _ in 0..<40 { if Input.frontmostOK() { front = true; break }; sleepMs(50) }
        guard front else { refuse(ctx, "pid \(pid) did not become the frontmost app; no Run was sent"); return }
        sleepMs(400)
    }
    var idle: [Double] = []
    for _ in 0..<10 {
        if let f = footprintBytes(pid: pid) { idle.append(Double(f)) }
        sleepMs(20)
    }
    guard !idle.isEmpty else { refuse(ctx, "proc_pid_rusage failed for pid \(pid)"); return }
    let idleBytes = percentile(idle, 0.5)
    var peak = idleBytes
    if !noRun, Input.chord(ctx.profile.run) == nil { refuse(ctx, focusLost); return }
    let start = Clock.nowNs()
    while Clock.ms(from: start, to: Clock.nowNs()) < duration * 1000 {
        if let f = footprintBytes(pid: pid) { peak = max(peak, Double(f)) }
        sleepMs(20)
    }
    var m: [String: Any] = ["footprint_delta_bytes": peak - idleBytes, "peak_bytes": peak, "idle_bytes": idleBytes]
    if let b = args.string("budget-bytes").flatMap(Double.init) { m["budget_bytes"] = b }
    var notes = "phys_footprint every 20 ms for \(duration) s; peak minus median idle of 10 samples; \(noRun ? "observed only, no Run pressed" : "Run pressed once"); " + targetNoteText
    if !ctx.profile.verified { notes += "; profile unverified" }
    ctx.out.metrics(m, notes: notes)
}

// MARK: - check-permissions

func cmdCheckPermissions() {
    let p = PermissionState.current()
    let d: [String: Any] = [
        "tool": "qhbench",
        "screen_recording": p.screenRecording ? "granted" : "not granted",
        "accessibility": p.accessibility ? "granted" : "not granted",
        "missing": p.missingNames,
        "note": "read-only check; qhbench never requests or changes these permissions",
    ]
    if let data = try? JSONSerialization.data(withJSONObject: d, options: [.sortedKeys]), let s = String(data: data, encoding: .utf8) {
        print(s)
    }
    logErr("Screen Recording: \(p.screenRecording ? "granted" : "NOT granted")")
    logErr("Accessibility:    \(p.accessibility ? "granted" : "NOT granted")")
    if !p.missingNames.isEmpty {
        logErr("Grant them in System Settings > Privacy & Security to the terminal app that runs qhbench.")
    }
}
