import XCTest

@testable import QueryHive

/// The row limit a new tab starts with.
///
/// The limit itself lives on the tab, because it is a property of looking rather than of the
/// query: the grid's own field changes one tab and leaves the others alone, and a restored session
/// keeps each tab's number. This preference is only the starting value, which is exactly the part
/// that is easy to get wrong — a tab that ignored it, or a default that read as zero.
@MainActor
final class RowLimitSettingTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json.
        isolateConnectionStore()
    }

    /// Runs `body` with `defaultRowLimit` set to `value`, and puts the real value back after.
    ///
    /// The property is read from `UserDefaults.standard` at construction, so this is the path under
    /// test and the original has to be restored rather than left for the next test.
    private func withStoredRowLimit(_ value: Int?, _ body: () -> Void) {
        let key = "defaultRowLimit"
        let original = UserDefaults.standard.object(forKey: key)
        addTeardownBlock {
            if let original {
                UserDefaults.standard.set(original, forKey: key)
            } else {
                UserDefaults.standard.removeObject(forKey: key)
            }
        }
        if let value {
            UserDefaults.standard.set(value, forKey: key)
        } else {
            UserDefaults.standard.removeObject(forKey: key)
        }
        body()
    }

    func testAnUnsetPreferenceStartsANewTabAtAThousand() {
        withStoredRowLimit(nil) {
            let model = AppModel()
            XCTAssertEqual(model.defaultRowLimit, 1000)
            XCTAssertEqual(model.selectedTab?.rowLimit, 1000)
        }
    }

    func testANewTabTakesTheStoredPreference() {
        withStoredRowLimit(250) {
            let model = AppModel()
            XCTAssertEqual(model.defaultRowLimit, 250)
            XCTAssertEqual(model.selectedTab?.rowLimit, 250,
                           "a new tab ignored the default row limit")
        }
    }

    /// The default applies when a tab is made, not when it is read: a tab already open keeps its
    /// own number, which is what makes the grid's field per-tab rather than global.
    func testChangingTheDefaultLeavesOpenTabsAlone() {
        withStoredRowLimit(1000) {
            let model = AppModel()
            let first = try? XCTUnwrap(model.selectedTab)
            XCTAssertEqual(first?.rowLimit, 1000)

            model.defaultRowLimit = 250
            XCTAssertEqual(model.selectedTab?.rowLimit, 1000,
                           "an open tab followed a change meant for the next one")

            model.newTab()
            XCTAssertEqual(model.selectedTab?.rowLimit, 250)
        }
    }
}
