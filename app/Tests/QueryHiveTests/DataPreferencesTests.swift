import XCTest

@testable import QueryHive

/// The grid's switches: what an untouched install gets, and that a change is written down.
///
/// Each test builds its own store on a scratch `UserDefaults` suite, so nothing here reads or writes
/// the preferences the user actually has.
final class DataPreferencesTests: XCTestCase {
    private var suite: UserDefaults!

    override func setUp() {
        super.setUp()
        let suiteName = "qh-data-prefs-\(UUID().uuidString)"
        suite = UserDefaults(suiteName: suiteName)
        addTeardownBlock {
            UserDefaults.standard.removePersistentDomain(forName: suiteName)
        }
    }

    private func store() -> DataPreferences {
        DataPreferences(defaults: suite)
    }

    func testAnUntouchedInstallGetsWhatTheGridAlreadyDid() {
        let prefs = store()
        // The middle row height is the one the grid has always used, the band on every other row was
        // always drawn, the gutter was always there, and a NULL has always read `null`.
        XCTAssertEqual(prefs.rowHeight, .normal)
        XCTAssertEqual(prefs.rowHeight.points, 25)
        XCTAssertEqual(prefs.nullDisplay, "null")
        XCTAssertTrue(prefs.alternateRows)
        XCTAssertTrue(prefs.showRowNumbers)
        XCTAssertEqual(prefs.viewerMode, .automatic)
        XCTAssertEqual(prefs.firstSortDirection, .ascending)
        // 12 is the size the cells were hard-coded to, and at 12 the row is exactly its preset.
        XCTAssertEqual(prefs.gridFontSize, 12)
        XCTAssertEqual(prefs.rowPoints, 25)
    }

    func testAChangeIsWrittenDownAndReadBack() {
        let prefs = store()
        prefs.rowHeight = .tall
        prefs.nullDisplay = "NULL"
        prefs.alternateRows = false
        prefs.showRowNumbers = false
        prefs.viewerMode = .text
        prefs.firstSortDirection = .descending
        prefs.gridFontSize = 14

        let reopened = DataPreferences(defaults: suite)
        XCTAssertEqual(reopened.rowHeight, .tall)
        XCTAssertEqual(reopened.nullDisplay, "NULL")
        XCTAssertFalse(reopened.alternateRows)
        XCTAssertFalse(reopened.showRowNumbers)
        XCTAssertEqual(reopened.viewerMode, .text)
        XCTAssertEqual(reopened.firstSortDirection, .descending)
        XCTAssertEqual(reopened.gridFontSize, 14)
    }

    func testAGridFontSizeIsClampedToWhatTheRowsCanHold() {
        let prefs = store()
        prefs.gridFontSize = 3
        XCTAssertEqual(prefs.gridFontSize, 11)
        prefs.gridFontSize = 40
        XCTAssertEqual(prefs.gridFontSize, 16)
        prefs.gridFontSize = 16
        XCTAssertEqual(prefs.gridFontSize, 16, "the ends of the range are sizes")
        prefs.gridFontSize = 11
        XCTAssertEqual(prefs.gridFontSize, 11)
    }

    func testAStoredGridFontSizeOutsideTheRangeIsHeldInsideItOnLoad() {
        // The absent key reads 0 and must mean "the default", not "the smallest size".
        XCTAssertEqual(store().gridFontSize, 12)
        suite.set(99, forKey: "gridFontSize")
        XCTAssertEqual(store().gridFontSize, 16)
        suite.set(-2, forKey: "gridFontSize")
        XCTAssertEqual(store().gridFontSize, 11)
    }

    func testTheRowFollowsALargerFontByTwoPointsForEachPoint() {
        // The standard size is the preset exactly, whatever the preset, which is what keeps the
        // baselines where they are; above it the row grows with the font, and below it the row
        // stays, because a smaller line does not shrink as fast as two points a size.
        for height in DataPreferences.RowHeight.allCases {
            XCTAssertEqual(height.points(atFontSize: 12), height.points)
            XCTAssertEqual(height.points(atFontSize: 14), height.points + 4)
            XCTAssertEqual(height.points(atFontSize: 11), height.points)
        }
        let prefs = store()
        prefs.rowHeight = .tall
        prefs.gridFontSize = 16
        XCTAssertEqual(prefs.rowPoints, 38)
    }

    func testAnEmptyNullDisplayIsAChoiceRatherThanAMissingSetting() {
        // Some people want the cell blank, and blank is a different thing from "nobody chose".
        let prefs = store()
        prefs.nullDisplay = ""
        XCTAssertEqual(DataPreferences(defaults: suite).nullDisplay, "")
    }

    func testTheThreeRowHeightsAreOrdered() {
        XCTAssertLessThan(DataPreferences.RowHeight.compact.points,
                          DataPreferences.RowHeight.normal.points)
        XCTAssertLessThan(DataPreferences.RowHeight.normal.points,
                          DataPreferences.RowHeight.tall.points)
    }

    func testTheReaderPanelIsOffUntilItIsAskedFor() {
        // The one switch on the pane that changes the result pane's shape rather than the grid's
        // drawing, so the only one that defaults off: an untouched install keeps the whole width for
        // the rows, which is what it has always had.
        XCTAssertFalse(store().autoShowInspector)
    }

    func testPinningChangesWhatIsDrawnWithoutWritingItDown() {
        let prefs = store()
        prefs.pin(showRowNumbers: false, autoShowInspector: true)
        XCTAssertTrue(prefs.autoShowInspector)
        XCTAssertFalse(prefs.showRowNumbers)

        // And the file is left alone, because a render is not allowed to change what the user finds
        // next time. This is the bug `ThemeStore.pin` exists to prevent, stated as a test.
        let reopened = DataPreferences(defaults: suite)
        XCTAssertFalse(reopened.autoShowInspector)
        XCTAssertTrue(reopened.showRowNumbers)
    }

    func testTheFirstSortDirectionOnlyDecidesWhereTheCycleStarts() {
        // Ascending as the first direction is what the grid has always done: ascending, descending,
        // off.
        XCTAssertEqual(GridSort.next(GridSort(column: 0, direction: .ascending), clickedColumn: 0),
                       GridSort(column: 0, direction: .descending))
        XCTAssertNil(GridSort.next(GridSort(column: 0, direction: .descending), clickedColumn: 0))

        // Started descending, the cycle is descending, ascending, off — so both directions stay
        // reachable whichever way it starts, which a fixed cycle would not manage.
        let started = GridSort.next(nil, clickedColumn: 2, firstDirection: .descending)
        XCTAssertEqual(started, GridSort(column: 2, direction: .descending))
        XCTAssertEqual(GridSort.next(started, clickedColumn: 2, firstDirection: .descending),
                       GridSort(column: 2, direction: .ascending))
        XCTAssertNil(GridSort.next(GridSort(column: 2, direction: .ascending), clickedColumn: 2,
                                   firstDirection: .descending))

        // A different column starts the cycle over rather than continuing the old one.
        XCTAssertEqual(GridSort.next(GridSort(column: 0, direction: .descending), clickedColumn: 1),
                       GridSort(column: 1, direction: .ascending))
    }
}
