import AppKit

/// What the Dock tile shows, behind a seam so progress can be checked without a Dock.
@MainActor
protocol DockTileDisplaying: AnyObject {
    /// A badge, a progress bar (0...1), or both nil to show the plain icon.
    func show(badge: String?, fraction: Double?)
    func clear()
}

/// The one Dock tile, shared by every run that reports progress. A run only clears it when it is
/// the last one still reporting, so a short export finishing cannot wipe a long one's badge.
@MainActor
final class DockProgress {
    static let shared = DockProgress(tile: SystemDockTile())

    private let tile: any DockTileDisplaying
    private var live = 0

    init(tile: any DockTileDisplaying) { self.tile = tile }

    func begin() { live += 1 }

    func show(badge: String?, fraction: Double?) { tile.show(badge: badge, fraction: fraction) }

    func end() {
        live = max(0, live - 1)
        if live == 0 { tile.clear() }
    }
}

/// Progress for one export or `to_table` run, in the two places the system draws it: a `Progress`
/// that Finder shows on the file being written, and the Dock tile (FR-RUN-05).
///
/// Finder gets a progress only for a single file whose final path is known up front. A numbered
/// part, a zip, or a format that splits itself renames or replaces the file mid-run, so Finder
/// would be pointed at a name that goes away (F-23). The Dock always gets one.
@MainActor
final class FileProgress {
    private let dock: DockProgress
    private let finderTarget: URL?
    private let total: Int?
    private let publishes: Bool

    private(set) var progress: Progress?
    private var published = false
    private var dockLive = false
    private var lastShown: String?

    /// - Parameters:
    ///   - finderTarget: the file the run writes, or nil when Finder should not show one.
    ///   - total: the row count when it is known for this very statement; nil shows an
    ///     indeterminate bar and a running count instead.
    ///   - publishes: false in tests, so no progress is announced to the system.
    init(finderTarget: URL?, total: Int?, dock: DockProgress? = nil, publishes: Bool = true) {
        self.finderTarget = finderTarget
        self.total = total.flatMap { $0 > 0 ? $0 : nil }
        self.dock = dock ?? .shared
        self.publishes = publishes
    }

    /// Called with the running row count, which the engine sends every so often.
    func update(rows: Int) {
        if !dockLive {
            dockLive = true
            dock.begin()
        }
        publishWhenTheFileExists()
        progress?.completedUnitCount = Int64(total.map { min(rows, $0) } ?? rows)

        let fraction = total.map { min(1, Double(rows) / Double($0)) }
        let badge = total == nil ? Self.badge(rows: rows) : nil
        // The Dock redraws only when what it shows changes: whole percents, or the compact label.
        let key = badge ?? "\(Int((fraction ?? 0) * 100))"
        guard key != lastShown else { return }
        lastShown = key
        dock.show(badge: badge, fraction: fraction)
    }

    /// Ends both displays. Safe to call twice, and on every way a run can end.
    func finish() {
        if let progress {
            // An indeterminate progress has no end to reach; give it one so it reads as finished.
            if progress.totalUnitCount <= 0 { progress.totalUnitCount = 1 }
            progress.completedUnitCount = progress.totalUnitCount
            if published { progress.unpublish() }
        }
        progress = nil
        published = false
        if dockLive {
            dockLive = false
            dock.end()
        }
    }

    /// The engine emits `start` just before it opens the file, so publishing there would point
    /// Finder at a name that does not exist yet. The first progress event is after the first
    /// rows were written, and a run too short to reach one needs no progress bar.
    private func publishWhenTheFileExists() {
        guard progress == nil, let target = finderTarget,
              FileManager.default.fileExists(atPath: target.path) else { return }
        let progress = Progress(totalUnitCount: Int64(total ?? -1))
        progress.kind = .file
        progress.fileOperationKind = .downloading
        progress.fileURL = target
        progress.isCancellable = false
        progress.isPausable = false
        self.progress = progress
        if publishes {
            progress.publish()
            published = true
        }
    }

    // MARK: What a run reports

    /// The file an export will write, when Finder can follow it: one file, no zip, no split.
    static func finderTarget(for tab: QueryTab) -> URL? {
        guard tab.destination == .file, !tab.zip, tab.splitRows == 0, !tab.format.splitsItself,
              let directory = tab.outputDirectory, !tab.trimmedName.isEmpty else { return nil }
        return directory.appendingPathComponent("\(tab.trimmedName).\(tab.format.rawValue)")
    }

    /// The total the tab counted, but only for the statement being exported: a count of some
    /// other statement would draw a confident, wrong bar.
    static func knownTotal(for tab: QueryTab, sql: String) -> Int? {
        guard let total = tab.totalRows, tab.previewedSQL == sql else { return nil }
        return total
    }

    /// "1.2M": the Dock badge is a few characters wide.
    static func badge(rows: Int, locale: Locale = .current) -> String {
        rows.formatted(.number.notation(.compactName).locale(locale))
    }
}

/// `NSApp.dockTile`, with a content view only while a bar is showing: setting one replaces the
/// application icon, so the view draws the icon itself.
@MainActor
final class SystemDockTile: DockTileDisplaying {
    private var bar: DockBarView?

    func show(badge: String?, fraction: Double?) {
        guard let tile = NSApp?.dockTile else { return }
        tile.badgeLabel = badge
        if let fraction {
            if bar == nil {
                let view = DockBarView(frame: NSRect(origin: .zero, size: tile.size))
                bar = view
                tile.contentView = view
            }
            bar?.fraction = fraction
        } else if bar != nil {
            bar = nil
            tile.contentView = nil
        }
        tile.display()
    }

    func clear() {
        guard let tile = NSApp?.dockTile else { return }
        bar = nil
        tile.badgeLabel = nil
        tile.contentView = nil
        tile.display()
    }
}

private final class DockBarView: NSView {
    var fraction = 0.0 { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        NSApp?.applicationIconImage?.draw(in: bounds)
        let height = bounds.height * 0.1
        let track = NSRect(x: bounds.width * 0.1, y: bounds.height * 0.06,
                           width: bounds.width * 0.8, height: height)
        NSColor.black.withAlphaComponent(0.55).setFill()
        NSBezierPath(roundedRect: track, xRadius: height / 2, yRadius: height / 2).fill()
        var filled = track
        filled.size.width *= min(1, max(0, fraction))
        NSColor.controlAccentColor.setFill()
        NSBezierPath(roundedRect: filled, xRadius: height / 2, yRadius: height / 2).fill()
    }
}
