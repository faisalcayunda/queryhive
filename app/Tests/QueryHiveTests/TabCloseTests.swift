import XCTest

@testable import QueryHive

/// Closing tabs in bulk, and the one startup preference behind them.
///
/// A tab strip with one `x` per tab is fine until someone has eight of them and wants the last two.
/// The three bulk closes are small, but two things about them are easy to get wrong and are what
/// these tests actually pin: the tab the user was looking at decides what *stays*, and every close
/// still goes through `closeTab` — so a running tab's process is terminated exactly as the single
/// close does it, rather than by an array edit that would leave it running.
@MainActor
final class TabCloseTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json.
        isolateConnectionStore()
    }

    private func model() -> AppModel {
        AppModel()
    }

    /// A workspace with `count` tabs, returning their ids in strip order.
    private func workspace(_ model: AppModel, tabs count: Int) -> [UUID] {
        // `AppModel()` already made one, so this tops up rather than creating `count` from nothing.
        while model.tabs.count < count {
            model.newTab()
        }
        return model.tabs.map(\.id)
    }

    func testClosingOthersKeepsTheNamedTabAndSelectsIt() {
        let model = model()
        let ids = workspace(model, tabs: 4)

        model.closeOtherTabs(keeping: ids[1])

        XCTAssertEqual(model.tabs.map(\.id), [ids[1]])
        // The tab that stays has to be the one in front, or the tab under the pointer moves.
        XCTAssertEqual(model.selectedTabID, ids[1])
    }

    func testClosingOthersOnTheOnlyTabLeavesItAlone() {
        let model = model()
        let ids = workspace(model, tabs: 1)

        model.closeOtherTabs(keeping: ids[0])

        XCTAssertEqual(model.tabs.map(\.id), ids)
    }

    func testClosingTabsToTheRightRemovesExactlyThose() {
        let model = model()
        let ids = workspace(model, tabs: 5)

        model.closeTabs(after: ids[1])

        XCTAssertEqual(model.tabs.map(\.id), Array(ids.prefix(2)))
    }

    func testClosingTabsToTheRightOfTheLastTabIsANoOp() {
        let model = model()
        let ids = workspace(model, tabs: 3)

        model.closeTabs(after: ids[2])

        XCTAssertEqual(model.tabs.map(\.id), ids)
    }

    func testClosingTheSelectedTabAlsoClosesItsNeighboursToTheRight() {
        // The named tab is the one the menu was opened on, which is not always the selected one: a
        // right-click does not select. The removal is by strip position either way.
        let model = model()
        let ids = workspace(model, tabs: 4)
        model.selectTab(ids[3])

        model.closeTabs(after: ids[0])

        XCTAssertEqual(model.tabs.map(\.id), [ids[0]])
        // The tab that was in front is gone, so the selection has to fall somewhere that exists.
        XCTAssertEqual(model.selectedTabID, ids[0])
    }

    func testClosingAllLeavesTheWorkspaceEmpty() {
        let model = model()
        _ = workspace(model, tabs: 3)

        model.closeAllTabs()

        XCTAssertTrue(model.tabs.isEmpty)
        XCTAssertNil(model.selectedTabID)
        // Nothing is left to show, so the panel closes with it.
        XCTAssertTrue(model.panelCollapsed)
    }

    func testANewTabAfterClosingAllDoesNotReuseAName() {
        // `closeAllTabs` deliberately does not reset the counter it names from: two tabs called
        // "Query 1" in one session would be two tabs nobody can tell apart.
        let model = model()
        _ = workspace(model, tabs: 3)
        let titlesBefore = model.tabs.map(\.title)

        model.closeAllTabs()
        model.newTab()

        let fresh = try? XCTUnwrap(model.selectedTab)
        XCTAssertNotNil(fresh)
        XCTAssertFalse(titlesBefore.contains(fresh?.title ?? ""),
                       "a new tab reused the name of one that just closed")
    }
}

/// The startup preference: whether a launch brings back the tabs it left.
@MainActor
final class StartupSettingTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    /// The reader takes its defaults so this can be checked without writing the real ones.
    func testUnsetMeansOn() {
        let suite = UserDefaults(suiteName: "qh-test-\(UUID().uuidString)")!
        addTeardownBlock { suite.removePersistentDomain(forName: suite.description) }

        // `object(forKey:)`, not `bool(forKey:)`: the latter answers `false` for a key nobody has
        // written, which would read as "off" for every user who has never opened Settings.
        XCTAssertNil(suite.object(forKey: AppModel.restoreTabsKey))
        XCTAssertTrue(AppModel.storedRestoreTabsOnLaunch(in: suite))
    }

    func testAStoredOffIsHonoured() {
        let suite = UserDefaults(suiteName: "qh-test-\(UUID().uuidString)")!
        addTeardownBlock { suite.removePersistentDomain(forName: suite.description) }

        suite.set(false, forKey: AppModel.restoreTabsKey)
        XCTAssertFalse(AppModel.storedRestoreTabsOnLaunch(in: suite))

        suite.set(true, forKey: AppModel.restoreTabsKey)
        XCTAssertTrue(AppModel.storedRestoreTabsOnLaunch(in: suite))
    }

    /// The property writes where the next launch reads, so the two have to be the same place.
    func testThePropertyWritesWhatTheReaderReads() {
        // This one does touch `UserDefaults.standard`, because that is the path under test: the
        // property's storage and the reader's storage agreeing is the whole claim. The original
        // value is put back afterwards.
        let original = UserDefaults.standard.object(forKey: AppModel.restoreTabsKey)
        addTeardownBlock {
            if let original {
                UserDefaults.standard.set(original, forKey: AppModel.restoreTabsKey)
            } else {
                UserDefaults.standard.removeObject(forKey: AppModel.restoreTabsKey)
            }
        }

        let model = AppModel()
        model.restoreTabsOnLaunch = false

        XCTAssertFalse(AppModel.storedRestoreTabsOnLaunch(),
                       "the property wrote somewhere the next launch does not read")
        XCTAssertFalse(AppModel().restoreTabsOnLaunch,
                       "a fresh model did not pick the setting up")
    }
}

/// A tab that closes while its grid is on screen.
///
/// The grid lives through the Runs of a tab (W8-F1), so it can be holding a store at the moment the
/// tab lets go of it. Whatever it does next must not be drawing rows out of a released store.
@MainActor
final class TabCloseGridTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    override func tearDown() {
        PerfSignposts.recording = false
        PerfSignposts.clearStages()
        super.tearDown()
    }

    func testClosingATabLetsGoOfItsStoreAndTheTableDrawsNothingMore() throws {
        let grid = HostedGrid()
        defer { grid.close() }
        grid.run()
        let table = try XCTUnwrap(grid.table)
        let store = try XCTUnwrap(grid.tab.activeResult)
        XCTAssertEqual(table.numberOfRows, 50)

        PerfSignposts.recording = true
        PerfSignposts.clearStages()
        grid.model.closeTab(grid.tab.id)
        // The table is still in the window for a moment, as it is while SwiftUI catches up.
        table.noteNumberOfRowsChanged()
        table.needsDisplay = true
        table.displayIfNeeded()
        grid.spin(0.05)

        XCTAssertTrue(store.isReleased)
        XCTAssertNil(grid.tab.activeResult)
        XCTAssertEqual(table.numberOfRows, 0)
        XCTAssertNil(PerfSignposts.time(of: .firstDraw), "a row was drawn after its store was let go")
    }

    func testClosingATabInTheMiddleOfARunLeavesNoStoreBehind() throws {
        var script = StreamingEngine.Script()
        script.delay = 0.3
        let engine = StreamingEngine()
        engine.script = script
        let grid = HostedGrid(engine: engine)
        defer { grid.close() }
        grid.model.preview(grid.tab)
        grid.spin(0.1)
        XCTAssertNotNil(grid.table, "the table is up while the engine works")
        let store = try XCTUnwrap(engine.stores.first)

        grid.model.closeTab(grid.tab.id)
        // The engine's late events find a tab whose token has gone and are dropped.
        grid.spin(0.5)

        XCTAssertTrue(store.isReleased)
        XCTAssertEqual(store.count, 0)
        XCTAssertNil(grid.tab.activeResult)
    }
}
