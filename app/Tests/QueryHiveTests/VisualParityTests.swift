import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// The visual parity gate (performance-plan §4 item 0.8, PRD NFR-V).
//
// Every scene is drawn through the real AppKit/SwiftUI view tree at a fixed size and appearance, and
// compared against a baseline recorded from the committed UI. The layers, so a failure says what kind
// of drift it is:
//
//   1. layout      exported numbers must be exactly equal. Grid: each separator line (x, top, bottom),
//                  the horizontal rules, where the rows start, the height of the rows and the row
//                  height that implies. Editor: gutter width, inset, first line fragment, scroll
//                  offset, run and fold marks, find bar frame. The grid's numbers are measured from
//                  the pixels, so the gate does not depend on the view type that draws them (the
//                  grid's renderer is planned to change).
//   2. colour      RGBA at sample points inside cell padding, where the 3x3 neighbourhood of the
//                  baseline is flat, must be identical: cell background, selection wash, stripe,
//                  staged wash. Points come from the baseline, so a moved column is layout, not colour.
//   3. markers     small things the pixel budget below cannot see. Each probe is a rectangle and a
//                  count of saturated pixels (dots, chevrons, funnels, chips, run triangles) or of ink
//                  pixels (the empty-string mark, NULL, the ellipsis, invisible characters). A marker
//                  that vanishes, or appears, fails.
//   4. pixels      at most 0.1% of the pixels may differ by more than 16/255 in any channel.
//   5. attributes  editor only: the attribute runs of the text that is on screen, without the bodies
//                  of folded regions, with adjacent equal runs merged, plus the layout manager's
//                  temporary attributes (find highlights). Which mechanism produced a run is not
//                  part of it.
//
// Record:   QH_RECORD_BASELINES=1 swift test --filter VisualParityTests   (fails on purpose, see below)
//           QH_RECORD_BASELINES=grid-selection,grid-edits swift test --filter VisualParityTests
//                                                                     (only those scenes, both looks)
// Compare:  swift test --filter VisualParityTests
//
// Recording fails the run after writing ("recorded N scenes; rerun without QH_RECORD_BASELINES"), so a
// gate left in record mode cannot report green. Re-recording needs the owner's approval
// (development-plan §0.5): a scene that drifts outside the changes registered in the PRD §6.5 is a
// failed gate, not a reason to record again.
//
// Not capturable by `cacheDisplay`, and therefore checked by hand: tooltips (`.help`), the cell-reader
// popover and the double-click that opens it, the spinner of `grid-loading` (it animates), and the
// caret (the text view is never first responder here, so none is drawn).

// MARK: - Sidecar

/// A small feature and how much of it is on screen: a rectangle in pixels and a pixel count.
struct Marker: Codable, Equatable {
    var name: String
    /// `sat`: pixels with a channel spread of 100 or more (accent, amber). `ink`: pixels that differ
    /// from the rectangle's most common colour by 30 or more.
    var kind: String
    /// x, y, width, height in pixels.
    var rect: [Int]
    var count: Int

    func measure(_ p: Pixels) -> Int {
        let x0 = max(rect[0], 0), y0 = max(rect[1], 0)
        let x1 = min(rect[0] + rect[2], p.width), y1 = min(rect[1] + rect[3], p.height)
        guard x1 > x0, y1 > y0 else { return 0 }
        var n = 0
        if kind == "sat" {
            for y in y0..<y1 {
                for x in x0..<x1 {
                    let r = p.channel(x, y, 0), g = p.channel(x, y, 1), b = p.channel(x, y, 2)
                    if max(r, g, b) - min(r, g, b) >= 100 { n += 1 }
                }
            }
        } else {
            let base = p.mode(x0, y0, x1, y1)
            for y in y0..<y1 {
                for x in x0..<x1 {
                    var worst = 0
                    for c in 0..<3 { worst = max(worst, abs(p.channel(x, y, c) - base[c])) }
                    if worst >= 30 { n += 1 }
                }
            }
        }
        return n
    }
}

/// What a scene exports besides its picture.
struct Sidecar: Codable, Equatable {
    var scene: String
    /// Pixel size of the picture. Part of the layout: a window that grew is a layout change.
    var pixelSize: [Int]
    /// The switches the scene was drawn under, so a baseline says what it is a baseline of.
    var facts: [String: String]
    /// Named integer arrays. Pixels for the grid, hundredths of a point for the editor.
    var metrics: [String: [Int]]
    /// `"x,y,rrggbbaa"` at flat points of the baseline. Grid scenes only.
    var samples: [String]
    var markers: [Marker]
    /// Editor scenes only: the distinct attribute sets, and the runs that use them as
    /// `[location, length, temporary (0 or 1), index into styles]`.
    var styles: [String]
    var runs: [[Int]]

    /// The runs with their styles written out, which is the form the comparator reads.
    var resolvedRuns: [AttrRun] {
        runs.map { AttrRun(range: [$0[0], $0[1]], temporary: $0[2] == 1, attrs: styles[$0[3]]) }
    }

    init(scene: String, pixelSize: [Int], facts: [String: String], metrics: [String: [Int]],
         samples: [String], markers: [Marker] = [], attributeRuns: [AttrRun] = []) {
        self.scene = scene
        self.pixelSize = pixelSize
        self.facts = facts
        self.metrics = metrics
        self.samples = samples
        self.markers = markers
        var table: [String] = []
        var runs: [[Int]] = []
        for run in attributeRuns {
            let index = table.firstIndex(of: run.attrs) ?? { table.append(run.attrs); return table.count - 1 }()
            runs.append([run.range[0], run.range[1], run.temporary ? 1 : 0, index])
        }
        self.styles = table
        self.runs = runs
    }
}

struct AttrRun: Equatable {
    var range: [Int]
    var temporary: Bool
    /// Every attribute of the run, sorted by name, as one string.
    var attrs: String
}

/// A decoded picture: sRGB, 8 bits, RGBA. Both the baseline and the fresh render go through the same
/// decode, so the comparison never sees a difference that is only the colour space of the file.
struct Pixels {
    let width: Int
    let height: Int
    var rgba: [UInt8]

    init(width: Int, height: Int, rgba: [UInt8]) {
        self.width = width
        self.height = height
        self.rgba = rgba
    }

    init?(png: Data) {
        guard let source = CGImageSourceCreateWithData(png as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
              let space = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        let w = image.width, h = image.height
        var buffer = [UInt8](repeating: 0, count: w * h * 4)
        let drawn = buffer.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(data: raw.baseAddress, width: w, height: h,
                                          bitsPerComponent: 8, bytesPerRow: w * 4, space: space,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        guard drawn else { return nil }
        self.init(width: w, height: h, rgba: buffer)
    }

    @inline(__always) func channel(_ x: Int, _ y: Int, _ c: Int) -> Int {
        Int(rgba[(y * width + x) * 4 + c])
    }

    /// Largest per-channel difference between two pixels of this picture.
    @inline(__always) func delta(_ x1: Int, _ y1: Int, _ x2: Int, _ y2: Int) -> Int {
        var largest = 0
        for c in 0..<3 { largest = max(largest, abs(channel(x1, y1, c) - channel(x2, y2, c))) }
        return largest
    }

    func hex(_ x: Int, _ y: Int) -> String {
        guard x >= 0, y >= 0, x < width, y < height else { return "out" }
        return (0..<4).map { String(format: "%02x", channel(x, y, $0)) }.joined()
    }

    /// The most common colour in a rectangle, as r, g, b.
    func mode(_ x0: Int, _ y0: Int, _ x1: Int, _ y1: Int) -> [Int] {
        var counts: [Int: Int] = [:]
        for y in y0..<y1 {
            for x in x0..<x1 {
                counts[(channel(x, y, 0) << 16) | (channel(x, y, 1) << 8) | channel(x, y, 2), default: 0] += 1
            }
        }
        let best = counts.max { $0.value < $1.value }?.key ?? 0
        return [(best >> 16) & 255, (best >> 8) & 255, best & 255]
    }

    /// Paint a rectangle with its own most common colour: what the picture would look like if
    /// whatever was drawn there were not.
    mutating func erase(_ rect: [Int]) {
        let x0 = max(rect[0], 0), y0 = max(rect[1], 0)
        let x1 = min(rect[0] + rect[2], width), y1 = min(rect[1] + rect[3], height)
        guard x1 > x0, y1 > y0 else { return }
        let base = mode(x0, y0, x1, y1)
        for y in y0..<y1 {
            for x in x0..<x1 {
                let i = (y * width + x) * 4
                rgba[i] = UInt8(base[0]); rgba[i + 1] = UInt8(base[1]); rgba[i + 2] = UInt8(base[2])
            }
        }
    }
}

// MARK: - Grid measurements

/// Layout and colour measured from a picture of the grid.
enum GridScan {
    /// Pixels per point in every picture this gate takes. 1x keeps the baselines small enough to
    /// commit; everything below is written in pixels and reads this instead of a literal.
    static let scale = 1

    /// Geometry from the pixels alone, for pictures at 1x.
    ///
    /// A separator is a line one point wide, so one or two pixels wide once it lands between pixels;
    /// it differs from both sides along a long run that is drawn on most of its length. Glyph edges
    /// differ from one side, or last a few rows, or (a column of left-aligned text) cover a third of
    /// the rows at most. Each separator keeps its own bottom; the top of the run is the toolbar's rule
    /// rather than the top of any line, because a line can chain onto a glyph above it. A rule is a
    /// row that differs from the row above across at least 90% of the content width, at a contrast
    /// that only rules have (stripes are fainter and are measured on their own). `contentWidth`
    /// leaves out a panel beside the grid, which has none of the grid's rules and whose own rule
    /// runs the whole height.
    static func metrics(_ p: Pixels, contentWidth: Int) -> [String: [Int]] {
        precondition(scale == 1, "the separator scan is written for 1x pictures")
        var lines: [(x: Int, top: Int, bottom: Int)] = []
        var lastX = -10
        for x in 1..<min(p.width - 3, contentWidth) where x - lastX > 2 {
            func on(_ y: Int) -> Bool {
                guard p.delta(x, y, x - 1, y) >= 3 else { return false }
                if p.delta(x, y, x + 1, y) >= 3 { return true }
                return p.delta(x + 1, y, x + 2, y) >= 3 && p.delta(x, y, x + 1, y) <= 2
            }
            // Runs of "this pixel column is a line", allowing the short gaps between one row's cell
            // and the next (a cell is a little shorter than its row).
            var runs: [(top: Int, bottom: Int, on: Int)] = []
            var start = -1, lastOn = -1, count = 0
            for y in 0..<p.height where on(y) {
                if start >= 0, y - lastOn > 10 { runs.append((start, lastOn + 1, count)); start = -1; count = 0 }
                if start < 0 { start = y }
                lastOn = y
                count += 1
            }
            if start >= 0 { runs.append((start, lastOn + 1, count)) }
            if let best = runs.filter({ $0.on * 10 >= ($0.bottom - $0.top) * 6 })
                .max(by: { $0.bottom - $0.top < $1.bottom - $1.top }),
               best.bottom - best.top >= 40 {
                lines.append((x, best.top, best.bottom))
                lastX = x
            }
        }

        var rules: [Int] = []
        let sampled = stride(from: 0, to: min(contentWidth, p.width), by: 2).map { $0 }
        for y in 1..<p.height {
            var hits = 0
            for x in sampled where p.delta(x, y, x, y - 1) >= 10 { hits += 1 }
            if hits * 10 >= sampled.count * 9 {
                if let previous = rules.last, y - previous <= 1 { continue }
                rules.append(y)
            }
        }
        // The grid's lines, without a panel's rule (which starts at the top of the picture).
        let grid = lines.filter { $0.top > 2 }
        let runTop = rules.first.map { $0 + 1 } ?? (grid.map(\.top).min() ?? 0)
        let runBottom = grid.map(\.bottom).max() ?? 0
        // The rows start under the header's own bottom rule, the first rule well below its top.
        let bodyTop = (rules.first { $0 >= runTop + 30 }).map { $0 + 1 } ?? 0
        // Stripe edges down the left padding, where nothing but the row's own background is drawn.
        var stripes: [Int] = []
        let stripeEnd = min(runBottom + 4, p.height)
        if bodyTop > 0, bodyTop < stripeEnd {
            for y in bodyTop..<stripeEnd where p.delta(3, y, 3, y - 1) >= 4 { stripes.append(y) }
        }
        return ["stripeEdges": stripes, "columnEdges": grid.map(\.x),
                "separators": grid.flatMap { [$0.x, $0.bottom] },
                "separatorRun": [runTop, runBottom], "horizontalRules": rules, "bodyTop": [bodyTop],
                "bodyHeight": [max(0, runBottom - bodyTop)]]
    }

    /// Where colour is sampled: 3pt into the left padding of every column (8pt, and text starts after
    /// it), and every 12pt down the header-and-rows run.
    static func samplePoints(_ metrics: [String: [Int]], width: Int) -> [(x: Int, y: Int)] {
        guard let edges = metrics["columnEdges"], !edges.isEmpty,
              let run = metrics["separatorRun"], run.count == 2, run[1] > run[0] else { return [] }
        let xs = [3 * scale] + edges.map { $0 + 4 * scale }.filter { $0 < width }
        var points: [(Int, Int)] = []
        var y = run[0] + 2 * scale
        while y < run[1] - 2 * scale {
            for x in xs { points.append((x, y)) }
            y += 12 * scale
        }
        return points
    }

    /// The baseline's samples: only points whose 3x3 neighbourhood is flat, so a glyph edge that
    /// happens to reach the padding cannot make a sample fragile.
    static func samples(_ p: Pixels, metrics: [String: [Int]]) -> [String] {
        samplePoints(metrics, width: p.width).compactMap { point in
            guard point.x >= 1, point.y >= 1, point.x < p.width - 1, point.y < p.height - 1 else { return nil }
            for dy in -1...1 { for dx in -1...1 where p.delta(point.x, point.y, point.x + dx, point.y + dy) > 2 {
                return nil
            } }
            return "\(point.x),\(point.y),\(p.hex(point.x, point.y))"
        }
    }

    /// The same points in another picture, in the same order.
    static func resample(_ p: Pixels, like baseline: [String]) -> [String] {
        baseline.map { entry in
            let parts = entry.split(separator: ",")
            guard parts.count == 3, let x = Int(parts[0]), let y = Int(parts[1]) else { return "bad" }
            return "\(x),\(y),\(p.hex(x, y))"
        }
    }
}

// MARK: - Comparator

/// The layered comparison. Pure functions over values, so it can be tested with a synthetic
/// difference and without a window.
enum ParityComparator {
    static let channelTolerance = 16
    static let pixelBudget = 0.001

    /// A marker is present if its count stays within half to double of the baseline's. A baseline of
    /// zero means "nothing here", and something appearing there is drift as well.
    static func markerFailures(baseline: Sidecar, actualPixels: Pixels) -> [String] {
        baseline.markers.compactMap { marker in
            let now = marker.measure(actualPixels)
            let ok = marker.count == 0 ? now == 0 : (now * 2 >= marker.count && now <= marker.count * 2 + 2)
            return ok ? nil : "marker: \(marker.name) (\(marker.kind) at \(marker.rect)) count \(marker.count) -> \(now)"
        }
    }

    /// Empty means the scene matches. Each message starts with the layer that failed.
    static func compare(baseline: Sidecar, baselinePixels: Pixels,
                        actual: Sidecar, actualPixels: Pixels) -> [String] {
        var failures: [String] = []

        // 1. layout
        if baseline.pixelSize != actual.pixelSize {
            failures.append("layout: picture size \(baseline.pixelSize) -> \(actual.pixelSize)")
        }
        if baseline.facts != actual.facts {
            failures.append("layout: scene switches \(baseline.facts) -> \(actual.facts)")
        }
        for key in Set(baseline.metrics.keys).union(actual.metrics.keys).sorted() {
            let old = baseline.metrics[key], new = actual.metrics[key]
            if old != new {
                failures.append("layout: \(key) \(old.map(String.init(describing:)) ?? "absent") -> "
                                + "\(new.map(String.init(describing:)) ?? "absent")")
            }
        }

        // 2. colour, at the baseline's points in the new picture
        if !baseline.samples.isEmpty {
            let now = GridScan.resample(actualPixels, like: baseline.samples)
            let bad = zip(baseline.samples, now).enumerated().filter { $0.element.0 != $0.element.1 }
            if let firstBad = bad.first {
                failures.append("colour: \(bad.count) of \(now.count) sample points differ; first "
                                + "\(firstBad.element.0) -> \(firstBad.element.1)")
            }
        }

        // 3. markers
        failures += markerFailures(baseline: baseline, actualPixels: actualPixels)

        // 4. pixels
        if baselinePixels.width == actualPixels.width, baselinePixels.height == actualPixels.height {
            var over = 0
            for y in 0..<baselinePixels.height {
                for x in 0..<baselinePixels.width {
                    var worst = 0
                    for c in 0..<4 {
                        worst = max(worst, abs(baselinePixels.channel(x, y, c) - actualPixels.channel(x, y, c)))
                    }
                    if worst > channelTolerance { over += 1 }
                }
            }
            let total = baselinePixels.width * baselinePixels.height
            if Double(over) > Double(total) * pixelBudget {
                failures.append(String(format: "pixels: %d of %d pixels (%.3f%%) differ by more than %d/255; "
                                       + "the budget is %.1f%%", over, total,
                                       Double(over) / Double(total) * 100, channelTolerance,
                                       pixelBudget * 100))
            }
        } else {
            failures.append("pixels: sizes differ, not compared")
        }

        // 5. attribute runs
        let oldRuns = baseline.resolvedRuns, newRuns = actual.resolvedRuns
        if oldRuns != newRuns {
            if oldRuns.count != newRuns.count {
                failures.append("attributes: \(oldRuns.count) runs -> \(newRuns.count)")
            }
            if let index = (0..<min(oldRuns.count, newRuns.count)).first(where: { oldRuns[$0] != newRuns[$0] }) {
                failures.append("attributes: run \(index) at \(oldRuns[index].range): "
                                + "\(oldRuns[index].attrs) -> \(newRuns[index].attrs)")
            }
        }
        return failures
    }
}

// MARK: - The gate

@MainActor
final class VisualParityTests: XCTestCase {
    private enum Look: String, CaseIterable {
        case dark, light
        var theme: AppTheme { self == .dark ? .midnight : .daylight }
        var mode: AppearanceMode { self == .dark ? .dark : .light }
        var scheme: ColorScheme { self == .dark ? .dark : .light }
        var appearance: NSAppearance { NSAppearance(named: self == .dark ? .darkAqua : .aqua)! }
    }

    private struct GridPrefs {
        var alternateRows = true
        var rowNumbers = true
        var rowHeight = DataPreferences.RowHeight.normal
        var facts: [String: String] {
            ["alternateRows": "\(alternateRows)", "rowNumbers": "\(rowNumbers)",
             "rowHeight": rowHeight.rawValue]
        }
    }

    private struct Capture {
        var png: Data
        var sidecar: Sidecar
    }

    private static var recording: Bool {
        let value = ProcessInfo.processInfo.environment["QH_RECORD_BASELINES"] ?? ""
        return !value.isEmpty && value != "0"
    }

    /// The scenes a recording is limited to, by name without the look (`grid-selection` is both
    /// `grid-selection-dark` and `grid-selection-light`); empty records every scene. A re-record is
    /// for a declared V-n change, and writing the scenes that did not move would hide whether they
    /// did.
    private static let recordOnly: Set<String> = {
        let value = ProcessInfo.processInfo.environment["QH_RECORD_BASELINES"] ?? ""
        return value == "1" ? [] : Set(value.split(separator: ",").map(String.init))
    }()

    private static func isRecorded(_ name: String) -> Bool {
        guard !recordOnly.isEmpty else { return true }
        let base = ["-dark", "-light"].first(where: name.hasSuffix).map { String(name.dropLast($0.count)) } ?? name
        return recordOnly.contains(base) || recordOnly.contains(name)
    }

    /// `__Baselines__` beside this file, so the path is the repository's and not the build's.
    private static let baselineDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().appendingPathComponent("__Baselines__", isDirectory: true)

    // What the run found, restored afterwards: these are singletons the rest of the suite reads.
    private var savedTheme: (dark: AppTheme, light: AppTheme, accent: AccentChoice, tone: SurfaceTone,
                             mode: AppearanceMode, systemIsDark: Bool, ui: String, code: String)!
    private var savedData: (rows: DataPreferences.RowHeight, alt: Bool, numbers: Bool, inspector: Bool,
                            null: String)!
    private var savedEditor: EditorLayout!
    private var windows: [NSWindow] = []

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        let store = ThemeStore.shared
        savedTheme = (store.darkTheme, store.lightTheme, store.accent, store.tone, store.mode,
                      store.systemIsDark, store.uiFontFamily, store.codeFontFamily)
        let data = DataPreferences.shared
        savedData = (data.rowHeight, data.alternateRows, data.showRowNumbers, data.autoShowInspector,
                     data.nullDisplay)
        savedEditor = EditorLayout.current
    }

    override func tearDown() {
        closeWindows()
        ThemeStore.shared.unpinSurface()
        let s = savedTheme!
        ThemeStore.shared.pin(theme: s.dark, accent: s.accent, tone: s.tone, mode: s.mode,
                              systemIsDark: s.systemIsDark, uiFont: s.ui, codeFont: s.code)
        ThemeStore.shared.pin(theme: s.light)
        let d = savedData!
        DataPreferences.shared.pin(rowHeight: d.rows, showRowNumbers: d.numbers,
                                   autoShowInspector: d.inspector)
        DataPreferences.shared.alternateRows = d.alt
        DataPreferences.shared.nullDisplay = d.null
        setEditor(savedEditor)
        super.tearDown()
    }

    private func closeWindows() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
    }

    // MARK: Scene setup

    /// Fixed appearance, fixed fonts. `pin` never writes the user's preferences.
    private func applyAppearance(_ look: Look) {
        // The Accessibility > Display switches of the machine must not reach a baseline.
        ThemeStore.shared.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: look.theme, accent: .ice, tone: .glow, mode: look.mode,
                              systemIsDark: look == .dark, uiFont: "", codeFont: "")
    }

    private func applyGrid(_ prefs: GridPrefs) {
        let data = DataPreferences.shared
        // `pin` first: it is what stops the setters below from persisting.
        data.pin(rowHeight: prefs.rowHeight, showRowNumbers: prefs.rowNumbers, autoShowInspector: false)
        data.alternateRows = prefs.alternateRows
        data.nullDisplay = "null"
    }

    private func setEditor(_ layout: EditorLayout) {
        let prefs = EditorPreferences.shared
        prefs.showLineNumbers = layout.showLineNumbers
        prefs.highlightCurrentLine = layout.highlightCurrentLine
        prefs.highlightCurrentStatement = layout.highlightCurrentStatement
        prefs.wordWrap = layout.wordWrap
        prefs.codeFolding = layout.codeFolding
        prefs.showInvisibles = layout.showInvisibles
        prefs.autoUppercaseKeywords = layout.autoUppercaseKeywords
        prefs.runButtonPerStatement = layout.runButtonPerStatement
        prefs.tabWidth = layout.tabWidth
        prefs.fontSize = layout.fontSize
    }

    // MARK: Rendering

    /// Host a view in a window of the given appearance and let it settle. No animation is running in
    /// any scene that reaches here.
    private func host(_ content: some View, size: CGSize, look: Look) -> (NSHostingView<AnyView>, NSWindow) {
        let root = AnyView(content
            .frame(width: size.width, height: size.height)
            .background(Tone.canvas)
            .preferredColorScheme(look.scheme))
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered,
                              defer: false)
        window.appearance = look.appearance
        window.contentView = host
        windows.append(window)
        settle(host)
        return (host, window)
    }

    /// The scroller style is a per-machine setting (System Settings > Appearance > Show scroll bars),
    /// and a legacy one takes width out of the layout. Overlay is the same everywhere.
    private static func forceOverlayScrollers(_ view: NSView) {
        if let scroll = view as? NSScrollView, scroll.scrollerStyle != .overlay { scroll.scrollerStyle = .overlay }
        for subview in view.subviews { forceOverlayScrollers(subview) }
    }

    /// Wait until the view has stopped changing: the drawn picture is identical for six turns in a
    /// row (0.18 s, longer than the editor's 80 ms highlight debounce). This is the idle condition;
    /// there is no fixed sleep to outrun on a slow machine.
    private func settle(_ host: NSView, file: StaticString = #filePath, line: UInt = #line) {
        var previous: Data?
        var stable = 0
        let deadline = Date().addingTimeInterval(10)
        while stable < 6, Date() < deadline {
            Self.forceOverlayScrollers(host)
            RunLoop.current.run(until: Date().addingTimeInterval(0.03))
            host.layoutSubtreeIfNeeded()
            guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { break }
            host.cacheDisplay(in: host.bounds, to: rep)
            let bytes = rep.bitmapData.map { Data(bytes: $0, count: rep.bytesPerRow * rep.pixelsHigh) }
            stable = bytes == previous ? stable + 1 : 0
            previous = bytes
        }
        if stable < 6 { XCTFail("the view did not stop changing within 10 s", file: file, line: line) }
    }

    /// An sRGB bitmap of the view at `GridScan.scale`, whatever the machine's display is.
    private func png(of host: NSView) throws -> Data {
        let scale = GridScan.scale
        let w = Int(host.bounds.width), h = Int(host.bounds.height)
        let device = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: w * scale, pixelsHigh: h * scale, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: 0, bitsPerPixel: 0))
        // Tagged before anything is drawn, so AppKit converts into sRGB instead of leaving the
        // machine's device space in the file.
        let rep = try XCTUnwrap(device.retagging(with: .sRGB))
        XCTAssertEqual(rep.colorSpace, NSColorSpace.sRGB)
        rep.size = host.bounds.size
        host.cacheDisplay(in: host.bounds, to: rep)
        return try XCTUnwrap(rep.representation(using: .png, properties: [:]))
    }

    // MARK: Grid scenes

    private static let gridSize = CGSize(width: 1000, height: 520)

    private static func isNumeric(_ type: String) -> Bool {
        ["int", "double", "float", "numeric", "decimal", "real"].contains { type.lowercased().contains($0) }
    }

    private func captureGrid(_ scene: String, name: String, look: Look,
                             prefs: GridPrefs = GridPrefs()) throws -> Capture {
        applyAppearance(look)
        applyGrid(prefs)
        let model = Snapshot.seeded(scene: scene)
        let tab = try XCTUnwrap(model.selectedTab, "\(name): the scene has no tab")
        // A scene that set a sort or a filter has asked the store for a view; the render waits for it.
        let landed = Date().addingTimeInterval(5)
        while tab.viewBusy, Date() < landed { RunLoop.main.run(until: Date().addingTimeInterval(0.005)) }
        let (view, _) = host(ResultGrid(tab: tab).environment(model), size: Self.gridSize, look: look)
        let data = try png(of: view)
        let pixels = try XCTUnwrap(Pixels(png: data))
        let s = GridScan.scale
        // The inspector panel beside the grid is 340pt and a rule; it has none of the grid's rules.
        let content = scene == "grid-inspector" ? pixels.width - 341 * s : pixels.width
        var metrics = GridScan.metrics(pixels, contentWidth: content)

        // Row height, measured: the rows' extent divided by how many there are. It has to be what the
        // Data pane says it is, and it is stored so the baseline says it too.
        let displayedRows = tab.result.rows(in: 0..<tab.result.count, columns: Array(0..<tab.result.columns.count))
        let rows = displayedRows.count
        metrics["rows"] = [rows]
        if rows > 0 {
            let body = metrics["bodyHeight"]![0]
            let expected = rows * Int(prefs.rowHeight.points) * s
            // A cell is a little shorter than its row, so the separators end within a few pixels of
            // the rows; the exact figure is stored, and the stripes give the pitch to the pixel.
            XCTAssertLessThanOrEqual(abs(body - expected), 4 * s,
                                     "[\(name)] the separators' extent \(body) is not \(rows) rows of "
                                     + "\(prefs.rowHeight.points)pt (\(expected))")
            if prefs.alternateRows, let edges = metrics["stripeEdges"], edges.count >= 3 {
                let pitches = Set(zip(edges, edges.dropFirst()).map { $1 - $0 })
                XCTAssertEqual(pitches, [Int(prefs.rowHeight.points) * s],
                               "[\(name)] stripe pitch is not the row height")
                metrics["rowHeight"] = [pitches.first ?? 0]
            }
        }
        var facts = prefs.facts
        facts["scene"] = scene
        facts["look"] = look.rawValue
        let markers = gridMarkers(tab: tab, metrics: metrics, prefs: prefs, pixels: pixels, name: name)
        return Capture(png: data, sidecar: Sidecar(
            scene: name, pixelSize: [pixels.width, pixels.height], facts: facts, metrics: metrics,
            samples: GridScan.samples(pixels, metrics: metrics), markers: markers))
    }

    /// The small things a 0.1% pixel budget (460 pixels here) cannot see. Positions come from the
    /// measured layout and the scene's own state; a probe that finds nothing where the scene says
    /// something must be is a misplaced probe and fails the recording.
    private func gridMarkers(tab: QueryTab, metrics: [String: [Int]], prefs: GridPrefs, pixels: Pixels,
                             name: String) -> [Marker] {
        let s = GridScan.scale
        guard let edges = metrics["columnEdges"], let run = metrics["separatorRun"], run[1] > run[0],
              let bodyTop = metrics["bodyTop"]?.first, let preview = tab.preview else { return [] }
        let gutter = prefs.rowNumbers ? 1 : 0
        let rowH = Int(prefs.rowHeight.points) * s
        let visible = tab.visibleColumnSources
        func trailing(_ column: Int) -> Int? { column + gutter < edges.count ? edges[column + gutter] : nil }
        func leading(_ column: Int) -> Int {
            column == 0 ? (gutter == 1 ? edges[0] + s : 0) : edges[column - 1 + gutter] + s
        }
        var found: [(Marker, mustExist: Bool)] = []
        func add(_ name: String, _ kind: String, _ rect: [Int], mustExist: Bool = false) {
            var marker = Marker(name: name, kind: kind, rect: rect, count: 0)
            marker.count = marker.measure(pixels)
            found.append((marker, mustExist))
        }

        // Header: the label line (sort chevron, active funnel, rename dot) and the type chip.
        for column in visible.indices {
            guard let right = trailing(column) else { continue }
            let left = leading(column)
            // Anchored to the header's bottom rule, not to the top of the run: a sort banner sits above.
            let top = bodyTop - 50 * s
            add("header[\(column)].label", "sat", [left, top + 5 * s, right - left, 17 * s])
            add("header[\(column)].chip", "sat", [left, top + 22 * s, right - left, 18 * s])
        }
        // Staged edits: the amber dot at the cell's top right.
        for key in tab.cellEdits.values.keys.sorted(by: { ($0.row, $0.column) < ($1.row, $1.column) }) {
            guard let column = visible.firstIndex(of: key.column), let right = trailing(column) else { continue }
            add("staged[\(key.row),\(column)].dot", "sat",
                [right - 10 * s, bodyTop + key.row * rowH, 10 * s, 10 * s], mustExist: true)
        }
        // The first NULL, empty string and truncated value in a text column.
        var seen: Set<String> = []
        let displayedRows = tab.result.rows(in: 0..<tab.result.count, columns: Array(0..<tab.result.columns.count))
        for (row, values) in displayedRows.enumerated() where bodyTop + (row + 1) * rowH <= run[1] {
            for (column, source) in visible.enumerated() {
                guard source < values.count, source < preview.columns.count,
                      !Self.isNumeric(preview.columns[source].type),
                      column == 0 || column - 1 + gutter < edges.count,
                      !(column == 0 && gutter == 1 && edges.isEmpty) else { continue }
                let centre = bodyTop + row * rowH + rowH / 2
                let value = values[source]
                let kind = value == nil ? "null" : value!.isEmpty ? "empty" : value!.count > 40 ? "long" : nil
                guard let kind, seen.contains(kind) == false else { continue }
                if kind == "long", trailing(column) == nil { continue }
                seen.insert(kind)
                let rect = kind == "long"
                    ? [trailing(column)! - 24 * s, centre - 6 * s, 16 * s, 12 * s]
                    : [leading(column) + 8 * s, centre - 6 * s, 26 * s, 12 * s]
                add("cell[\(row),\(column)].\(kind)", "ink", rect, mustExist: true)
            }
        }
        for (marker, mustExist) in found where mustExist && marker.count == 0 {
            XCTFail("[\(name)] probe \(marker.name) at \(marker.rect) found nothing: the probe is misplaced")
        }
        return found.map(\.0)
    }

    /// The reader the grid opens over a JSON cell. It is a popover in the app, which offscreen
    /// capture cannot reach, so it is drawn directly, the way the `grid-json` scene does.
    private func captureViewer(name: String, look: Look) throws -> Capture {
        applyAppearance(look)
        let size = CGSize(width: 592, height: 460)
        let (view, _) = host(CellValueViewer(
            value: #"{"kabupaten":"Bandung","jumlah_jiwa":48320,"kecamatan":["Sukamaju","Cibadak","Mekarsari"]}"#,
            column: "detail_wilayah", type: "map(varchar,json)"), size: size, look: look)
        let data = try png(of: view)
        let pixels = try XCTUnwrap(Pixels(png: data))
        return Capture(png: data, sidecar: Sidecar(
            scene: name, pixelSize: [pixels.width, pixels.height],
            facts: ["scene": "grid-json", "look": look.rawValue], metrics: [:], samples: []))
    }

    private func gridScenes() -> [(name: String, make: () throws -> Capture)] {
        var scenes: [(String, () throws -> Capture)] = []
        for look in Look.allCases {
            let l = look.rawValue
            // The one scene that carries every cell kind, header state and footer state, under each
            // of the Data pane's switches.
            let variants: [(String, GridPrefs)] = [
                ("", GridPrefs()),
                ("-altoff", GridPrefs(alternateRows: false)),
                ("-nonum", GridPrefs(rowNumbers: false)),
                ("-tall", GridPrefs(rowHeight: .tall)),
                ("-compact", GridPrefs(rowHeight: .compact)),
            ]
            for (suffix, prefs) in variants {
                let name = "grid-kinds\(suffix)-\(l)"
                scenes.append((name, { [unowned self] in
                    try self.captureGrid("grid-kinds", name: name, look: look, prefs: prefs)
                }))
            }
            // The existing `--snapshot` scenes that draw the grid.
            for scene in ["grid", "grid-selection", "grid-edits", "grid-sorted", "grid-columns",
                          "grid-inspector", "grid-filtered", "grid-counted", "grid-empty",
                          "grid-filtered-out"] {
                let name = "\(scene)-\(l)"
                scenes.append((name, { [unowned self] in
                    try self.captureGrid(scene, name: name, look: look)
                }))
            }
            let viewerName = "grid-json-\(l)"
            scenes.append((viewerName, { [unowned self] in
                try self.captureViewer(name: viewerName, look: look)
            }))
        }
        return scenes
    }

    // MARK: Editor scenes

    private static let editorSize = CGSize(width: 1000, height: 420)

    /// Long enough to scroll, and regular enough that the middle is recognisable: line N says N.
    private static let longDocument: String = (1...120).map { n in
        n % 8 == 0 ? "-- block \(n / 8)\nSELECT \(n) AS line_no, kode FROM hive.analytics.t_\(n / 8) WHERE tahun = 2026;"
                   : "SELECT \(n) AS line_no, nama FROM hive.analytics.t_\(n / 8) WHERE aktif = true"
    }.joined(separator: "\n")

    private static let wrapDocument = """
    SELECT kode_wilayah, nama, jumlah_jiwa, bobot, aktif, diperbarui, catatan, kode_wilayah AS kode_lagi, nama AS nama_lagi, jumlah_jiwa AS jiwa_lagi, bobot AS bobot_lagi FROM hive.analytics.penerima_manfaat WHERE kode_wilayah IN ('32.01.01.2001', '32.01.01.2002', '32.01.01.2003', '32.01.02.1004', '32.01.02.1005', '32.01.03.2010', '32.01.03.2011', '32.01.04.3001');
    SELECT 1
    """

    private static let invisiblesDocument = "SELECT\tkode_wilayah,   nama  \n\tFROM hive.analytics.penerima_manfaat  \nWHERE tahun = 2026\t;\n"

    private struct EditorSpec {
        var layout = EditorLayout.standard
        var sql: String? = nil
        var caret: Int? = nil          // nil: the `syntax` scene's own caret, in its second statement
        var fold = false
        var find: String? = nil
        var scrollToLine: Int? = nil
        var facts: [String: String] = [:]
    }

    private func captureEditor(name: String, look: Look, spec: EditorSpec) throws -> Capture {
        applyAppearance(look)
        setEditor(spec.layout)
        let model = Snapshot.seeded(scene: "syntax")
        let tab = try XCTUnwrap(model.selectedTab)
        if let sql = spec.sql { tab.sql = sql }
        if let caret = spec.caret {
            tab.caret = caret
            tab.selection = NSRange(location: caret, length: 0)
        }
        let (view, window) = host(EditorPane(tab: tab).environment(model), size: Self.editorSize, look: look)
        let textView = try XCTUnwrap(Self.findTextView(in: view), "\(name): no SQL text view")
        let coordinator = try XCTUnwrap(textView.delegate as? SQLEditor.Coordinator,
                                        "\(name): no editor coordinator")
        // One region, chosen: the CTE. Folding a second region moves the caret into the first one's
        // body, and the editor opens a fold the caret lands in, so "fold everything" folds one.
        var chosen: FoldRegion?
        if spec.fold {
            chosen = SQLFolding.regions(in: textView.string).first { $0.kind == .cte }
            let region = try XCTUnwrap(chosen, "\(name): the document has no CTE region to fold")
            coordinator.toggleFold(headerOffset: region.header)
        }
        if let query = spec.find {
            coordinator.findBar?.setQuery(query)
            coordinator.showFind(replacing: false)
            // The search field took first responder; nothing here may draw a caret.
            window.makeFirstResponder(nil)
        }
        settle(view)
        // What is folded is what the gutter says is folded once everything has settled, not what was
        // asked for: only those regions' bodies are left out of the attribute check.
        var folded: [FoldRegion] = []
        if let chosen {
            let closed = Set((coordinator.ruler?.foldMarks ?? []).filter(\.folded).map(\.headerLine))
            folded = SQLFolding.regions(in: textView.string).filter { closed.contains($0.headerLine) }
            XCTAssertTrue(folded.contains(chosen), "[\(name)] the CTE fold did not stay closed "
                          + "(gutter reports folded lines \(closed.sorted()))")
        }
        if let line = spec.scrollToLine, let layoutManager = textView.layoutManager,
           let container = textView.textContainer, let scroll = textView.enclosingScrollView {
            layoutManager.ensureLayout(for: container)
            let starts = SQLFolding.lineStarts(in: textView.string as NSString)
            let glyph = layoutManager.glyphIndexForCharacter(at: starts[min(line, starts.count - 1)])
            let rect = layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
            // Keep the clip view's own x: with a ruler it is negative, by the gutter's width.
            scroll.contentView.scroll(to: NSPoint(x: scroll.contentView.bounds.origin.x, y: rect.minY))
            scroll.reflectScrolledClipView(scroll.contentView)
            settle(view)
        }

        let data = try png(of: view)
        let pixels = try XCTUnwrap(Pixels(png: data))
        var facts = spec.facts
        facts["look"] = look.rawValue
        let markers = editorMarkers(textView: textView, coordinator: coordinator, host: view,
                                    spec: spec, pixels: pixels, name: name)
        return Capture(png: data, sidecar: Sidecar(
            scene: name, pixelSize: [pixels.width, pixels.height], facts: facts,
            metrics: editorMetrics(textView: textView, coordinator: coordinator, folded: folded),
            samples: [], markers: markers,
            attributeRuns: attributeRuns(of: textView, appearance: look.appearance,
                                         excluding: folded.map(\.body))))
    }

    private static func findTextView(in view: NSView) -> SQLTextView? {
        if let match = view as? SQLTextView { return match }
        for subview in view.subviews { if let found = findTextView(in: subview) { return found } }
        return nil
    }

    private func hundredths(_ values: CGFloat...) -> [Int] { values.map { Int(($0 * 100).rounded()) } }

    /// The editor's numbers, read from AppKit rather than measured: they are exact there.
    private func editorMetrics(textView: SQLTextView, coordinator: SQLEditor.Coordinator,
                               folded: [FoldRegion]) -> [String: [Int]] {
        var metrics: [String: [Int]] = [:]
        let inset = textView.textContainerInset
        metrics["textInset"] = hundredths(inset.width, inset.height)
        let frame = textView.frame
        metrics["textViewFrame"] = hundredths(frame.minX, frame.minY, frame.width, frame.height)
        if let scroll = textView.enclosingScrollView {
            metrics["scrollOffset"] = hundredths(scroll.contentView.bounds.origin.x,
                                                 scroll.contentView.bounds.origin.y)
            metrics["scrollFrame"] = hundredths(scroll.frame.minX, scroll.frame.minY,
                                                scroll.frame.width, scroll.frame.height)
            metrics["gutterWidth"] = hundredths(scroll.hasVerticalRuler && scroll.rulersVisible
                                                ? (scroll.verticalRulerView?.ruleThickness ?? 0) : 0)
        }
        if let ruler = coordinator.ruler {
            metrics["runMarkLines"] = ruler.runMarks.map(\.headerLine)
            metrics["foldMarks"] = ruler.foldMarks.flatMap { [$0.headerLine, $0.folded ? 1 : 0] }
        }
        if let bar = coordinator.findBar {
            metrics["findBarFrame"] = bar.isHidden ? [] : hundredths(bar.frame.minX, bar.frame.minY,
                                                                      bar.frame.width, bar.frame.height)
        }
        if let layoutManager = textView.layoutManager, let container = textView.textContainer {
            layoutManager.ensureLayout(for: container)
            metrics["containerWidth"] = hundredths(min(container.containerSize.width, 1_000_000))
            metrics["linePadding"] = hundredths(container.lineFragmentPadding)
            let length = (textView.string as NSString).length
            if folded.isEmpty {
                let used = layoutManager.usedRect(for: container)
                metrics["usedRect"] = hundredths(used.minX, used.minY, used.width, used.height)
            } else {
                // How a fold squashes its lines is the mechanism 4A/4B replace, so the whole text's
                // extent is not compared. What is: where the first line after each fold starts, to
                // the pixel.
                metrics["afterFoldY"] = folded.compactMap { region -> Int? in
                    guard region.bodyEnd < length else { return nil }
                    let glyph = layoutManager.glyphIndexForCharacter(at: region.bodyEnd)
                    let rect = layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
                    return Int((rect.minY + textView.textContainerOrigin.y).rounded()) * GridScan.scale
                }
            }
            if layoutManager.numberOfGlyphs > 0 {
                let line = layoutManager.lineFragmentRect(forGlyphAt: 0, effectiveRange: nil)
                metrics["firstLineFragment"] = hundredths(line.minX, line.minY, line.width, line.height)
            }
        }
        return metrics
    }

    /// Probes for the editor's small marks: every invisible character's glyph rectangle, and the
    /// gutter's run triangles.
    private func editorMarkers(textView: SQLTextView, coordinator: SQLEditor.Coordinator, host: NSView,
                               spec: EditorSpec, pixels: Pixels, name: String) -> [Marker] {
        let s = GridScan.scale
        guard let layoutManager = textView.layoutManager, let container = textView.textContainer else { return [] }
        layoutManager.ensureLayout(for: container)
        var found: [(Marker, mustExist: Bool)] = []
        func rect(inTextView r: NSRect) -> [Int] {
            let r = textView.convert(r, to: host)
            return [Int(r.minX.rounded()) * s, Int(r.minY.rounded()) * s,
                    max(1, Int(r.width.rounded())) * s, max(1, Int(r.height.rounded())) * s]
        }
        func add(_ name: String, _ kind: String, _ rect: [Int], mustExist: Bool) {
            var marker = Marker(name: name, kind: kind, rect: rect, count: 0)
            marker.count = marker.measure(pixels)
            found.append((marker, mustExist))
        }
        let origin = textView.textContainerOrigin
        if spec.layout.showInvisibles {
            let text = textView.string as NSString
            for index in 0..<text.length {
                let character = text.character(at: index)
                guard character == 0x20 || character == 0x09 || character == 0x0A else { continue }
                let glyph = layoutManager.glyphIndexForCharacter(at: index)
                var r = layoutManager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container)
                r.origin.x += origin.x
                r.origin.y += origin.y
                let label = character == 0x20 ? "space" : character == 0x09 ? "tab" : "newline"
                // A tab is drawn as nothing by this text view, and the baseline says so (count 0): an
                // arrow appearing later is drift like any other.
                add("invisible[\(index)].\(label)", "ink", rect(inTextView: r), mustExist: character != 0x09)
            }
        }
        if let ruler = coordinator.ruler, spec.layout.runButtonPerStatement {
            let gutter = ruler.convert(ruler.bounds, to: host)
            let text = textView.string as NSString
            // The gutter draws a triangle on each mark's *line*; two statements that begin on one line
            // (the second starts at the whitespace after the first's `;`) share it.
            let starts = SQLFolding.lineStarts(in: text)
            var lines: [Int] = []
            for mark in ruler.runMarks where !lines.contains(mark.headerLine) { lines.append(mark.headerLine) }
            for line in lines.prefix(6) where line < starts.count && starts[line] < text.length {
                let glyph = layoutManager.glyphIndexForCharacter(at: starts[line])
                var r = layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
                r.origin.y += origin.y
                let inHost = textView.convert(NSRect(x: 0, y: r.minY, width: 1, height: r.height), to: host)
                guard inHost.minY >= 0, inHost.maxY <= host.bounds.height else { continue }
                add("runMark[\(line)]", "sat",
                    [Int(gutter.minX.rounded()) * s, Int(inHost.minY.rounded()) * s,
                     20 * s, Int(inHost.height.rounded()) * s], mustExist: true)
            }
        }
        for (marker, mustExist) in found where mustExist && marker.count == 0 {
            XCTFail("[\(name)] probe \(marker.name) at \(marker.rect) found nothing: the probe is misplaced")
        }
        return found.map(\.0)
    }

    /// The attribute runs of what is on screen.
    ///
    /// Only the visible characters, and not the bodies of folded regions: what a fold does to text
    /// that is not shown is how the fold is implemented, which is not what the user sees. Adjacent
    /// runs that describe to the same attributes are one run, so how the highlighter happens to
    /// split a range does not matter either.
    private func attributeRuns(of textView: NSTextView, appearance: NSAppearance,
                               excluding hidden: [NSRange]) -> [AttrRun] {
        var runs: [AttrRun] = []
        appearance.performAsCurrentDrawingAppearance {
            guard let storage = textView.textStorage, let layoutManager = textView.layoutManager,
                  let container = textView.textContainer else { return }
            let origin = textView.textContainerOrigin
            let visible = textView.visibleRect.offsetBy(dx: -origin.x, dy: -origin.y)
            let glyphs = layoutManager.glyphRange(forBoundingRect: visible, in: container)
            let characters = layoutManager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
            var allowed = [characters]
            for body in hidden {
                allowed = allowed.flatMap { segment -> [NSRange] in
                    let cut = NSIntersectionRange(segment, body)
                    guard cut.length > 0 else { return [segment] }
                    return [NSRange(location: segment.location, length: cut.location - segment.location),
                            NSRange(location: NSMaxRange(cut), length: NSMaxRange(segment) - NSMaxRange(cut))]
                        .filter { $0.length > 0 }
                }
            }
            for segment in allowed {
                storage.enumerateAttributes(in: segment, options: []) { attrs, range, _ in
                    runs.append(AttrRun(range: [range.location, range.length], temporary: false,
                                        attrs: Self.describe(attrs)))
                }
                // Find highlights are temporary attributes on the layout manager, not text.
                var location = segment.location
                while location < NSMaxRange(segment) {
                    var effective = NSRange()
                    let attrs = layoutManager.temporaryAttributes(
                        atCharacterIndex: location, longestEffectiveRange: &effective,
                        in: NSRange(location: location, length: NSMaxRange(segment) - location))
                    if !attrs.isEmpty {
                        runs.append(AttrRun(range: [effective.location, effective.length], temporary: true,
                                            attrs: Self.describe(attrs)))
                    }
                    location = max(NSMaxRange(effective), location + 1)
                }
            }
        }
        // Merge neighbours that are the same to the eye. Storage runs first, temporary after, each in
        // location order already, and a merge never crosses from one kind to the other.
        var merged: [AttrRun] = []
        for run in runs {
            if let last = merged.last, last.temporary == run.temporary, last.attrs == run.attrs,
               last.range[0] + last.range[1] == run.range[0] {
                merged[merged.count - 1].range[1] += run.range[1]
            } else {
                merged.append(run)
            }
        }
        return merged
    }

    private static func describe(_ attrs: [NSAttributedString.Key: Any]) -> String {
        var out: [String: String] = [:]
        for (key, value) in attrs {
            switch value {
            case let color as NSColor:
                let c = color.usingColorSpace(.sRGB) ?? color
                out[key.rawValue] = String(format: "rgba(%.3f,%.3f,%.3f,%.3f)", c.redComponent,
                                           c.greenComponent, c.blueComponent, c.alphaComponent)
            case let font as NSFont:
                out[key.rawValue] = "\(font.fontName)@\(font.pointSize)"
            case let style as NSParagraphStyle:
                out[key.rawValue] = "align=\(style.alignment.rawValue) "
                    + "tabs=\(style.tabStops.map { $0.location }) interval=\(style.defaultTabInterval) "
                    + "lineBreak=\(style.lineBreakMode.rawValue) lineSpacing=\(style.lineSpacing) "
                    + "min=\(style.minimumLineHeight) max=\(style.maximumLineHeight) "
                    + "multiple=\(style.lineHeightMultiple) head=\(style.headIndent) "
                    + "firstHead=\(style.firstLineHeadIndent) tail=\(style.tailIndent) "
                    + "paragraphSpacing=\(style.paragraphSpacing) before=\(style.paragraphSpacingBefore) "
                    + "direction=\(style.baseWritingDirection.rawValue)"
            default:
                out[key.rawValue] = "\(value)"
            }
        }
        return out.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: " | ")
    }

    private func editorScenes() -> [(name: String, make: () throws -> Capture)] {
        var scenes: [(String, () throws -> Capture)] = []
        for look in Look.allCases {
            let l = look.rawValue
            var plain = EditorLayout.standard
            plain.highlightCurrentStatement = false
            plain.runButtonPerStatement = false
            var noWrap = EditorLayout.standard
            noWrap.wordWrap = false
            var invisibles = EditorLayout.standard
            invisibles.showInvisibles = true
            let specs: [(String, EditorSpec)] = [
                // Unfolded, run buttons on, the statement band on the second statement.
                ("editor-syntax", EditorSpec(facts: ["scene": "syntax"])),
                ("editor-folded", EditorSpec(fold: true, facts: ["scene": "syntax", "folded": "true"])),
                ("editor-plain", EditorSpec(layout: plain, facts: ["scene": "syntax", "runButtons": "false",
                                                                    "statementBand": "false"])),
                ("editor-find", EditorSpec(find: "jiwa", facts: ["scene": "syntax", "find": "jiwa"])),
                ("editor-invisibles", EditorSpec(layout: invisibles, sql: Self.invisiblesDocument, caret: 0,
                                                 facts: ["scene": "invisibles"])),
                ("editor-wrap-on", EditorSpec(sql: Self.wrapDocument, caret: 0,
                                              facts: ["scene": "wrap", "wrap": "true"])),
                ("editor-wrap-off", EditorSpec(layout: noWrap, sql: Self.wrapDocument, caret: 0,
                                               facts: ["scene": "wrap", "wrap": "false"])),
                ("editor-long-middle", EditorSpec(sql: Self.longDocument, caret: 0, scrollToLine: 60,
                                                  facts: ["scene": "long", "scrollToLine": "60"])),
            ]
            for (base, spec) in specs {
                let name = "\(base)-\(l)"
                scenes.append((name, { [unowned self] in
                    try self.captureEditor(name: name, look: look, spec: spec)
                }))
            }
        }
        return scenes
    }

    // MARK: Record / compare

    private func check(_ scenes: [(name: String, make: () throws -> Capture)]) throws {
        let directory = Self.baselineDirectory
        if Self.recording { try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true) }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        var recorded = 0
        for (name, make) in scenes {
            if Self.recording, !Self.isRecorded(name) { continue }
            let capture = try make()
            closeWindows()
            let pngURL = directory.appendingPathComponent("\(name).png")
            let jsonURL = directory.appendingPathComponent("\(name).json")
            if Self.recording {
                try capture.png.write(to: pngURL)
                try encoder.encode(capture.sidecar).write(to: jsonURL)
                recorded += 1
                continue
            }
            guard let basePNG = try? Data(contentsOf: pngURL), let baseJSON = try? Data(contentsOf: jsonURL),
                  let baseline = try? JSONDecoder().decode(Sidecar.self, from: baseJSON),
                  let basePixels = Pixels(png: basePNG) else {
                XCTFail("[\(name)] no baseline. Record with QH_RECORD_BASELINES=1 (needs the owner's approval)")
                continue
            }
            guard let actualPixels = Pixels(png: capture.png) else {
                XCTFail("[\(name)] the fresh render could not be decoded")
                continue
            }
            let failures = ParityComparator.compare(baseline: baseline, baselinePixels: basePixels,
                                                    actual: capture.sidecar, actualPixels: actualPixels)
            for failure in failures {
                XCTFail("[\(name)] \(failure)")
            }
            if !failures.isEmpty, let dump = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !dump.isEmpty {
                // The picture that failed, so it can be looked at.
                let out = URL(fileURLWithPath: dump, isDirectory: true)
                try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
                try? capture.png.write(to: out.appendingPathComponent("\(name).actual.png"))
            }
        }
        if Self.recording {
            print("visual parity: recorded \(recorded) scenes into \(directory.path)")
            XCTFail("recorded \(recorded) scenes; rerun without QH_RECORD_BASELINES")
        }
    }

    func testGridScenesMatchTheirBaselines() throws {
        try check(gridScenes())
    }

    func testEditorScenesMatchTheirBaselines() throws {
        try check(editorScenes())
    }

    /// A machine with Increase Contrast on must still match the baselines: the scene setup pins
    /// the surface, and the pin wins over what the system says.
    func testBaselinesIgnoreTheMachinesAccessibilitySettings() throws {
        let plain = editorScenes().filter { $0.name.hasPrefix("editor-plain") }
        XCTAssertFalse(plain.isEmpty)
        let scenes = plain.map { scene in
            (name: scene.name, make: { () throws -> Capture in
                ThemeStore.shared.pin(reduceMotion: true, reduceTransparency: true, increaseContrast: true)
                return try scene.make()
            })
        }
        try check(scenes)
    }

    // MARK: Baselines against each other

    private func baseline(_ name: String) throws -> (Sidecar, Pixels) {
        let png = try Data(contentsOf: Self.baselineDirectory.appendingPathComponent("\(name).png"))
        let json = try Data(contentsOf: Self.baselineDirectory.appendingPathComponent("\(name).json"))
        return (try JSONDecoder().decode(Sidecar.self, from: json), try XCTUnwrap(Pixels(png: png)))
    }

    /// The same scene in dark and in light is the same layout. If the scan ever reports a different
    /// geometry for the two, it is measuring the colours, not the lines.
    func testGridGeometryIsTheSameInDarkAndLight() throws {
        try XCTSkipIf(Self.recording, "recording")
        for (name, _) in gridScenes() where name.hasSuffix("-dark") && !name.hasPrefix("grid-json") {
            let light = String(name.dropLast(4)) + "light"
            let (dark, _) = try baseline(name), (bright, _) = try baseline(light)
            for key in ["columnEdges", "separators", "separatorRun", "horizontalRules", "bodyTop",
                        "bodyHeight", "rows", "rowHeight", "stripeEdges"] {
                XCTAssertEqual(dark.metrics[key], bright.metrics[key], "\(name) vs \(light): \(key)")
            }
        }
    }

    /// Every marker of a real baseline, erased in turn, must be reported. This is what the pixel
    /// budget cannot do: a whole 4-point dot is 13 pixels of 460,000.
    func testEveryRecordedMarkerBitesWhenErased() throws {
        try XCTSkipIf(Self.recording, "recording")
        var bitten = 0
        var byPrefix: [String: Int] = [:]
        for name in ["grid-kinds-dark", "grid-kinds-light", "grid-edits-dark", "editor-invisibles-dark",
                     "editor-syntax-dark", "editor-syntax-light"] {
            let (sidecar, pixels) = try baseline(name)
            XCTAssertEqual(ParityComparator.markerFailures(baseline: sidecar, actualPixels: pixels), [],
                           "\(name): the baseline must match itself")
            for marker in sidecar.markers where marker.count > 0 {
                var erased = pixels
                erased.erase(marker.rect)
                let failures = ParityComparator.markerFailures(baseline: sidecar, actualPixels: erased)
                XCTAssertTrue(failures.contains { $0.contains(marker.name) },
                              "\(name): erasing \(marker.name) at \(marker.rect) went unnoticed")
                bitten += 1
                byPrefix[String(marker.name.prefix { $0 != "[" && $0 != "." }), default: 0] += 1
            }
        }
        // The kinds of thing the review asked to be covered, each seen at least once.
        for kind in ["staged", "cell", "header", "invisible", "runMark"] {
            XCTAssertGreaterThan(byPrefix[kind] ?? 0, 0, "no \(kind) marker was recorded with anything in it")
        }
        print("markers erased and caught: \(bitten) \(byPrefix)")
    }

    // MARK: The comparator bites

    /// A picture with two "columns" (separator lines at x = 40 and 90) inside a run from y = 10 to
    /// y = 170, on a flat background, and one marker: a 4x4 amber square at (100, 50).
    private func synthetic() -> (Sidecar, Pixels) {
        let w = 200, h = 200
        var rgba = [UInt8](repeating: 0, count: w * h * 4)
        for i in 0..<(w * h) {
            rgba[i * 4] = 20; rgba[i * 4 + 1] = 24; rgba[i * 4 + 2] = 40; rgba[i * 4 + 3] = 255
        }
        var pixels = Pixels(width: w, height: h, rgba: rgba)
        for x in [40, 90] {
            for y in 10..<170 { for dx in 0..<GridScan.scale { Self.set(&pixels, x + dx, y, 32, 36, 52) } }
        }
        for x in 100..<104 { for y in 50..<54 { Self.set(&pixels, x, y, 245, 166, 35) } }
        let metrics: [String: [Int]] = ["columnEdges": [40, 90], "separatorRun": [10, 170],
                                        "horizontalRules": []]
        var marker = Marker(name: "dot", kind: "sat", rect: [96, 46, 12, 12], count: 0)
        marker.count = marker.measure(pixels)
        let sidecar = Sidecar(scene: "synthetic", pixelSize: [w, h], facts: ["k": "v"], metrics: metrics,
                              samples: GridScan.samples(pixels, metrics: metrics), markers: [marker],
                              attributeRuns: [AttrRun(range: [0, 6], temporary: false, attrs: "color=a")])
        return (sidecar, pixels)
    }

    private static func set(_ p: inout Pixels, _ x: Int, _ y: Int, _ r: UInt8, _ g: UInt8, _ b: UInt8) {
        let i = (y * p.width + x) * 4
        p.rgba[i] = r; p.rgba[i + 1] = g; p.rgba[i + 2] = b
    }

    func testTheComparatorAcceptsAnIdenticalScene() {
        let (sidecar, pixels) = synthetic()
        XCTAssertEqual(ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: sidecar, actualPixels: pixels), [])
    }

    func testOneMovedMetricFailsTheLayoutLayer() {
        let (sidecar, pixels) = synthetic()
        var moved = sidecar
        moved.metrics["columnEdges"] = [40, 91]
        let failures = ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: moved, actualPixels: pixels)
        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(failures.first?.hasPrefix("layout: columnEdges") == true, "\(failures)")
    }

    func testOneMovedRowHeightFailsTheLayoutLayer() {
        let (sidecar, pixels) = synthetic()
        var base = sidecar
        base.metrics["rowHeight"] = [25]
        var moved = base
        moved.metrics["rowHeight"] = [26]
        let failures = ParityComparator.compare(baseline: base, baselinePixels: pixels,
                                                actual: moved, actualPixels: pixels)
        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(failures.first?.hasPrefix("layout: rowHeight") == true, "\(failures)")
    }

    func testOneWrongPixelAtASamplePointFailsTheColourLayerOnly() throws {
        let (sidecar, pixels) = synthetic()
        XCTAssertFalse(sidecar.samples.isEmpty)
        let first = sidecar.samples[0].split(separator: ",")
        let x = try XCTUnwrap(Int(first[0])), y = try XCTUnwrap(Int(first[1]))
        var changed = pixels
        // 4 levels: below the pixel layer's 16/255 tolerance, so only the exact colour layer sees it.
        Self.set(&changed, x, y, 24, 28, 44)
        let failures = ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: sidecar, actualPixels: changed)
        XCTAssertEqual(failures.count, 1, "\(failures)")
        XCTAssertTrue(failures.first?.hasPrefix("colour:") == true, "\(failures)")
    }

    func testASamplePointBesideAnEdgeIsNotSampled() {
        // A point whose neighbourhood is not flat is dropped from the baseline: a glyph edge that
        // reaches the padding must not make a sample fragile.
        let (_, pixels) = synthetic()
        var busy = pixels
        Self.set(&busy, 4, 13, 255, 255, 255)   // beside the first sample column (x = 3)
        let metrics: [String: [Int]] = ["columnEdges": [40, 90], "separatorRun": [10, 170]]
        let all = GridScan.samples(pixels, metrics: metrics)
        let fewer = GridScan.samples(busy, metrics: metrics)
        XCTAssertEqual(fewer.count, all.count - 1)
    }

    func testOneWildPixelOffTheSamplePointsIsWithinTheBudget() {
        // 1 of 40,000 pixels is 0.0025%, under the 0.1% the pixel layer allows; that tolerance is
        // what lets a text renderer change through, and it is stated here so nobody has to guess.
        let (sidecar, pixels) = synthetic()
        var changed = pixels
        Self.set(&changed, 3, 3, 255, 255, 255)
        XCTAssertEqual(ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: sidecar, actualPixels: changed), [])
    }

    func testAnErasedMarkerFailsEvenThoughThePixelBudgetAllowsIt() {
        // 16 pixels of 40,000: invisible to the pixel layer, and exactly what the marker layer is for.
        let (sidecar, pixels) = synthetic()
        var erased = pixels
        erased.erase([96, 46, 12, 12])   // the marker's own rectangle: its background is what remains
        let failures = ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: sidecar, actualPixels: erased)
        XCTAssertEqual(failures.count, 1, "\(failures)")
        XCTAssertTrue(failures.first?.hasPrefix("marker: dot") == true, "\(failures)")
    }

    func testAMarkerThatAppearsWhereThereWasNoneFails() {
        let (sidecar, pixels) = synthetic()
        var empty = sidecar
        empty.markers = [Marker(name: "quiet", kind: "sat", rect: [96, 46, 12, 12], count: 0)]
        var plain = pixels
        plain.erase([96, 46, 12, 12])
        XCTAssertEqual(ParityComparator.markerFailures(baseline: empty, actualPixels: plain), [])
        XCTAssertEqual(ParityComparator.markerFailures(baseline: empty, actualPixels: pixels).count, 1)
    }

    func testAPatchOverTheBudgetFailsThePixelLayer() {
        // 5 x 10 = 50 pixels of 40,000 is 0.125%, over the budget, and away from every sample point.
        let (sidecar, pixels) = synthetic()
        var changed = pixels
        for x in 150..<155 { for y in 180..<190 { Self.set(&changed, x, y, 255, 255, 255) } }
        let failures = ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: sidecar, actualPixels: changed)
        XCTAssertEqual(failures.count, 1, "\(failures)")
        XCTAssertTrue(failures.first?.hasPrefix("pixels:") == true, "\(failures)")
    }

    func testTheChannelToleranceIsSixteenAndNotMore() {
        let (sidecar, pixels) = synthetic()
        func failures(delta: Int) -> [String] {
            var changed = pixels
            // 100 pixels (0.25%), away from the sample points, so only the pixel layer can object.
            for x in 150..<160 { for y in 180..<190 { Self.set(&changed, x, y, UInt8(20 + delta), 24, 40) } }
            return ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                            actual: sidecar, actualPixels: changed)
        }
        XCTAssertEqual(failures(delta: 16), [])
        XCTAssertEqual(failures(delta: 17).count, 1)
    }

    func testOneChangedAttributeRunFailsTheAttributeLayer() {
        let (sidecar, pixels) = synthetic()
        let changed = Sidecar(scene: "synthetic", pixelSize: sidecar.pixelSize, facts: sidecar.facts,
                              metrics: sidecar.metrics, samples: sidecar.samples, markers: sidecar.markers,
                              attributeRuns: [AttrRun(range: [0, 6], temporary: false, attrs: "color=b")])
        let failures = ParityComparator.compare(baseline: sidecar, baselinePixels: pixels,
                                                actual: changed, actualPixels: pixels)
        XCTAssertEqual(failures.count, 1)
        XCTAssertTrue(failures.first?.hasPrefix("attributes:") == true, "\(failures)")
    }

    func testTheGridScanMeasuresLinesAndNotGlyphs() {
        let (sidecar, pixels) = synthetic()
        // Text-like specks along the way: many short edges, none of them a separator.
        var busy = pixels
        for y in stride(from: 12, to: 168, by: 3) { Self.set(&busy, 60, y, 200, 200, 200) }
        for x in stride(from: 5, to: 195, by: 2) { Self.set(&busy, x, 100, 200, 200, 200) }
        for p in [pixels, busy] {
            let metrics = GridScan.metrics(p, contentWidth: p.width)
            XCTAssertEqual(metrics["columnEdges"], sidecar.metrics["columnEdges"])
            XCTAssertEqual(metrics["separatorRun"], sidecar.metrics["separatorRun"])
        }
        // A separator has its own extent: a glyph elsewhere in the same pixel column does not lengthen it.
        var tall = pixels
        for y in 185..<195 { Self.set(&tall, 40, y, 240, 240, 240) }
        XCTAssertEqual(GridScan.metrics(tall, contentWidth: tall.width)["separatorRun"], [10, 170])
    }

    func testARuleNeedsNinetyPercentOfTheWidth() {
        let (_, pixels) = synthetic()
        var full = pixels, most = pixels, half = pixels
        for x in 0..<200 { Self.set(&full, x, 120, 60, 64, 80) }
        for x in 0..<170 { Self.set(&most, x, 120, 60, 64, 80) }   // 85%
        for x in 0..<100 { Self.set(&half, x, 120, 60, 64, 80) }
        XCTAssertEqual(GridScan.metrics(full, contentWidth: 200)["horizontalRules"], [120])
        XCTAssertEqual(GridScan.metrics(most, contentWidth: 200)["horizontalRules"], [])
        XCTAssertEqual(GridScan.metrics(half, contentWidth: 200)["horizontalRules"], [])
    }
}
