import XCTest

@testable import QueryHive

@MainActor
private final class FakeTile: DockTileDisplaying {
    private(set) var shown: [(badge: String?, fraction: Double?)] = []
    private(set) var clears = 0
    func show(badge: String?, fraction: Double?) { shown.append((badge, fraction)) }
    func clear() { clears += 1 }
}

@MainActor
final class FileProgressTests: XCTestCase {
    private var directory: URL!

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-progress-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { [directory] in try? FileManager.default.removeItem(at: directory!) }
    }

    private func tab(_ configure: (QueryTab) -> Void = { _ in }) -> QueryTab {
        let tab = QueryTab(title: "Query 1")
        tab.destination = .file
        tab.format = .csv
        tab.outputName = "report"
        tab.outputDirectory = directory
        tab.zip = false
        tab.splitRows = 0
        configure(tab)
        return tab
    }

    private func make(target: URL?, total: Int?) -> (FileProgress, FakeTile, DockProgress) {
        let tile = FakeTile()
        let dock = DockProgress(tile: tile)
        return (FileProgress(finderTarget: target, total: total, dock: dock, publishes: false), tile, dock)
    }

    // MARK: Which runs get a Finder progress

    func testOneCSVFileGetsAFinderTarget() {
        XCTAssertEqual(FileProgress.finderTarget(for: tab()),
                       directory.appendingPathComponent("report.csv"))
    }

    func testRunsThatRenameOrReplaceTheFileGetNone() {
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.zip = true }), "zip")
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.splitRows = 1_000 }), "numbered parts")
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.format = .xlsx }), "xlsx splits itself")
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.format = .xls }), "xls splits itself")
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.destination = .table }), "a table has no file")
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.outputDirectory = nil }))
        XCTAssertNil(FileProgress.finderTarget(for: tab { $0.outputName = "  " }))
    }

    func testTheExtensionFollowsTheFormat() {
        XCTAssertEqual(FileProgress.finderTarget(for: tab { $0.format = .json })?.lastPathComponent,
                       "report.json")
    }

    func testATotalCountsOnlyForTheStatementItWasCountedFor() {
        let counted = tab { $0.totalRows = 5_000; $0.previewedSQL = "SELECT 1" }
        XCTAssertEqual(FileProgress.knownTotal(for: counted, sql: "SELECT 1"), 5_000)
        XCTAssertNil(FileProgress.knownTotal(for: counted, sql: "SELECT 2"))
        XCTAssertNil(FileProgress.knownTotal(for: tab { $0.previewedSQL = "SELECT 1" }, sql: "SELECT 1"))
    }

    // MARK: Finder

    func testNoProgressIsCreatedBeforeTheFileExists() {
        let target = directory.appendingPathComponent("report.csv")
        let (progress, _, _) = make(target: target, total: nil)
        progress.update(rows: 1_000)
        XCTAssertNil(progress.progress, "start comes before the file is opened; a short run never shows")
        try? Data("a\n".utf8).write(to: target)
        progress.update(rows: 2_000)
        XCTAssertNotNil(progress.progress)
    }

    func testTheFinderProgressNamesTheFileAndIsIndeterminateWithoutATotal() throws {
        let target = directory.appendingPathComponent("report.csv")
        try Data("a\n".utf8).write(to: target)
        let (progress, _, _) = make(target: target, total: nil)
        progress.update(rows: 1_000)
        let published = try XCTUnwrap(progress.progress)
        XCTAssertEqual(published.fileURL, target)
        XCTAssertEqual(published.kind, .file)
        XCTAssertTrue(published.isIndeterminate)
        XCTAssertFalse(published.isCancellable, "Stop lives in the app")
    }

    func testAKnownTotalGivesAFraction() throws {
        let target = directory.appendingPathComponent("report.csv")
        try Data("a\n".utf8).write(to: target)
        let (progress, _, _) = make(target: target, total: 200)
        progress.update(rows: 50)
        XCTAssertEqual(try XCTUnwrap(progress.progress).fractionCompleted, 0.25, accuracy: 0.001)
        progress.update(rows: 500)
        XCTAssertEqual(try XCTUnwrap(progress.progress).fractionCompleted, 1, accuracy: 0.001,
                       "rows past the counted total do not overshoot")
    }

    func testFinishCompletesAndDropsTheProgress() throws {
        let target = directory.appendingPathComponent("report.csv")
        try Data("a\n".utf8).write(to: target)
        let (progress, _, _) = make(target: target, total: nil)
        progress.update(rows: 10)
        let published = try XCTUnwrap(progress.progress)
        progress.finish()
        XCTAssertTrue(published.isFinished)
        XCTAssertNil(progress.progress)
        progress.finish()
    }

    func testNoTargetMeansNoFinderProgress() {
        let (progress, _, _) = make(target: nil, total: nil)
        progress.update(rows: 10_000)
        XCTAssertNil(progress.progress)
    }

    // MARK: Dock

    func testWithoutATotalTheDockShowsACompactCount() {
        let (progress, tile, _) = make(target: nil, total: nil)
        progress.update(rows: 1_234_567)
        XCTAssertEqual(tile.shown.count, 1)
        XCTAssertEqual(tile.shown[0].badge, FileProgress.badge(rows: 1_234_567))
        XCTAssertNil(tile.shown[0].fraction)
    }

    func testWithATotalTheDockShowsABarAndNoBadge() {
        let (progress, tile, _) = make(target: nil, total: 400)
        progress.update(rows: 100)
        XCTAssertNil(tile.shown[0].badge)
        XCTAssertEqual(try XCTUnwrap(tile.shown[0].fraction), 0.25, accuracy: 0.001)
    }

    func testTheDockIsOnlyRedrawnWhenWhatItShowsChanges() {
        let (progress, tile, _) = make(target: nil, total: 1_000)
        progress.update(rows: 100)
        progress.update(rows: 100)
        progress.update(rows: 104)
        XCTAssertEqual(tile.shown.count, 1, "same whole percent")
        progress.update(rows: 200)
        XCTAssertEqual(tile.shown.count, 2)
    }

    func testFinishClearsTheDock() {
        let (progress, tile, _) = make(target: nil, total: nil)
        progress.update(rows: 5_000)
        progress.finish()
        XCTAssertEqual(tile.clears, 1)
        progress.finish()
        XCTAssertEqual(tile.clears, 1, "ending twice clears once")
    }

    func testARunThatNeverReportedLeavesTheDockAlone() {
        let (progress, tile, _) = make(target: nil, total: nil)
        progress.finish()
        XCTAssertEqual(tile.clears, 0)
        XCTAssertTrue(tile.shown.isEmpty)
    }

    func testTheDockClearsWhenTheLastRunEnds() {
        let tile = FakeTile()
        let dock = DockProgress(tile: tile)
        let first = FileProgress(finderTarget: nil, total: nil, dock: dock, publishes: false)
        let second = FileProgress(finderTarget: nil, total: nil, dock: dock, publishes: false)
        first.update(rows: 1_000)
        second.update(rows: 2_000)
        first.finish()
        XCTAssertEqual(tile.clears, 0, "the longer run is still going")
        second.finish()
        XCTAssertEqual(tile.clears, 1)
    }

    func testTheBadgeIsCompact() {
        let english = Locale(identifier: "en_US")
        XCTAssertEqual(FileProgress.badge(rows: 999, locale: english), "999")
        XCTAssertEqual(FileProgress.badge(rows: 1_234_567, locale: english), "1.2M")
        XCTAssertEqual(FileProgress.badge(rows: 12_000, locale: english), "12K")
    }
}
