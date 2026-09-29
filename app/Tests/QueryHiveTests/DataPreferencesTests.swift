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
    }

    func testAChangeIsWrittenDownAndReadBack() {
        let prefs = store()
        prefs.rowHeight = .tall
        prefs.nullDisplay = "NULL"
        prefs.alternateRows = false
        prefs.showRowNumbers = false
        prefs.viewerMode = .text
        prefs.firstSortDirection = .descending

        let reopened = DataPreferences(defaults: suite)
        XCTAssertEqual(reopened.rowHeight, .tall)
        XCTAssertEqual(reopened.nullDisplay, "NULL")
        XCTAssertFalse(reopened.alternateRows)
        XCTAssertFalse(reopened.showRowNumbers)
        XCTAssertEqual(reopened.viewerMode, .text)
        XCTAssertEqual(reopened.firstSortDirection, .descending)
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
