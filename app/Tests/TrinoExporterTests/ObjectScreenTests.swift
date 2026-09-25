import XCTest

@testable import QueryHive

/// The object screen's click behaviour, without a server or a window.
///
/// What this file can check is the part that is pure: which cell is read as a table's name, what
/// statement the inspector asks with, and that the selection is dropped when the rows under it
/// change. The pane's appearance is checked by `--snapshot --scene objects`, which renders the real
/// view, and the round trip itself is checked by the engine's own suite.
final class ObjectScreenTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json, and the actions
        // these tests drive write it.
        isolateConnectionStore()
    }


    private func tab(columns: [String], rows: [[String?]]) -> QueryTab {
        let tab = QueryTab(title: "analytics")
        tab.objectScope = ObjectScope(connectionID: UUID(), catalog: "hive", schema: "analytics")
        tab.objectColumns = columns
        tab.objectRows = rows
        return tab
    }

    func testTheNameComesFromTheColumnTheDriverLabelledName() {
        // Found by header, not by position. The three drivers agree the first column is the name
        // today, but only the header is a contract: Postgres answers Name/OID/Owner/ACL, Trino
        // Name/Type and MySQL Name/Engine/Rows/Comment, and a driver that put its own column first
        // would otherwise have this read the wrong cell.
        let tab = tab(columns: ["Type", "Name"], rows: [["BASE TABLE", "wilayah"]])
        XCTAssertEqual(tab.objectName(at: 0), "wilayah")
    }

    func testTheNameIsReadCaseInsensitively() {
        let tab = tab(columns: ["name"], rows: [["wilayah"]])
        XCTAssertEqual(tab.objectName(at: 0), "wilayah")
    }

    func testAnEmptyNameIsNotATable() {
        // A blank cell is not a table called "". The inspector would otherwise run
        // `SELECT * FROM ""` and report the server's parse error as if it described a table.
        let tab = tab(columns: ["Name"], rows: [[""], ["   "]])
        XCTAssertNil(tab.objectName(at: 0))
        XCTAssertNil(tab.objectName(at: 1))
    }

    func testARowWithNoNameColumnOrNoSuchRowIsNilRatherThanACrash() {
        // Both of these are reachable: a driver whose listing has no Name column at all, and an
        // index left over from a list that has since been replaced.
        let noName = tab(columns: ["OID"], rows: [["24601"]])
        XCTAssertNil(noName.objectName(at: 0))
        let short = tab(columns: ["Name"], rows: [["a", "b"]])
        XCTAssertNil(short.objectName(at: 5))
        let ragged = tab(columns: ["Name", "OID"], rows: [["wilayah"]])
        XCTAssertEqual(ragged.objectName(at: 0), "wilayah")
    }

    func testTheInspectorStatementIsQualifiedTheWayTheDriverWants() {
        // The same `qualifiedName` the tree's double-click uses, so the two entry points cannot
        // produce different SQL for the same table.
        XCTAssertEqual(objectColumnsSQL(database: "hive", schema: "analytics", table: "wilayah",
                                        for: .trino),
                       "SELECT * FROM \"hive\".\"analytics\".\"wilayah\"")
        XCTAssertEqual(objectColumnsSQL(database: nil, schema: "public", table: "wilayah",
                                        for: .postgres),
                       "SELECT * FROM \"public\".\"wilayah\"")
        XCTAssertEqual(objectColumnsSQL(database: "qh", schema: nil, table: "wilayah", for: .mysql),
                       "SELECT * FROM `qh`.`wilayah`")
    }

    func testAMissingScopeFallsBackToTheBareNameRatherThanRefusing() {
        // A Trino scope with no catalog cannot be qualified, and the server will resolve the bare
        // name against the connection's own default. Refusing here would turn a working connection
        // into an inspector that never loads.
        XCTAssertEqual(objectColumnsSQL(database: nil, schema: "analytics", table: "wilayah",
                                        for: .trino),
                       "SELECT * FROM wilayah")
    }

    func testReloadingTheListingDropsTheSelection() {
        // The selection is an index, so a new list gives it a different meaning: the same number
        // would point at whichever table now sits at that row. Clearing is the only honest option,
        // and the detail fields go with it because they describe the table that was selected.
        let model = AppModel()
        let tab = tab(columns: ["Name"], rows: [["wilayah"]])
        tab.objectSelection = 0
        tab.objectDetailTable = "wilayah"
        tab.objectDetailColumns = [Event.Column(name: "kode", type: "text")]

        model.clearObjectSelection(tab)

        XCTAssertNil(tab.objectSelection)
        XCTAssertNil(tab.objectDetailTable)
        XCTAssertTrue(tab.objectDetailColumns.isEmpty)
        XCTAssertNil(tab.objectDetailToken, "a detail fetch still in flight must not land on a cleared pane")
    }

    func testTheObjectScreenIsATabOfItsOwn() {
        // `isObjects` is what routes the workspace to `ObjectsPane` instead of the editor, so a tab
        // that set `objectScope` without this being true would open an editor over a listing.
        let tab = tab(columns: ["Name"], rows: [])
        XCTAssertTrue(tab.isObjects)
        XCTAssertFalse(QueryTab(title: "Query 1").isObjects)
    }

    func testSelectingARowReasonsAboutAMissingConnectionRatherThanSayingNothing() {
        // The connection this scope names is not in the model -- reachable by deleting the
        // connection while its object tab is open. The row still becomes selected, because the
        // click did happen, and the inspector gets a reason instead of "No columns reported",
        // which would be a claim about the table rather than about this app never asking.
        let model = AppModel()
        let tab = tab(columns: ["Name"], rows: [["wilayah"]])

        model.selectObject(tab, row: 0)

        XCTAssertEqual(tab.objectSelection, 0)
        XCTAssertEqual(tab.objectDetailTable, "wilayah")
        XCTAssertNotNil(tab.objectDetailError)
        XCTAssertFalse(tab.objectDetailLoading, "a failed lookup must not leave the pane spinning")
        XCTAssertNil(tab.objectDetailProcess)
    }

    func testSelectingARowWithNoNameDoesNothingAtAll() {
        // A blank name has no table to describe, so the click is dropped rather than turning into
        // a statement about nothing.
        let model = AppModel()
        let tab = tab(columns: ["Name"], rows: [[""]])

        model.selectObject(tab, row: 0)

        XCTAssertNil(tab.objectSelection)
        XCTAssertNil(tab.objectDetailTable)
    }

    func testAReloadClearsTheSelectionBeforeTheFetchRatherThanAfterIt() {
        // The race this pins: the rows are painted by the `objects` event, which arrives *before*
        // the run exits. Clearing on exit therefore wiped a click that landed in between -- the row
        // stayed highlighted and the inspector never appeared. Clearing at the start cannot be
        // raced, because no row is on screen yet.
        //
        // Checked through the ordering rather than through timing: the selection is gone the moment
        // the reload is asked for, and the run it starts is what will fill the list back in.
        let model = AppModel()
        let tab = tab(columns: ["Name"], rows: [["wilayah"]])
        tab.objectSelection = 0
        tab.objectDetailTable = "wilayah"
        // No connection is registered, so the reload returns before starting anything -- which is
        // exactly the first half of the ordering: the clear happens before the guard that would
        // stop it. A version that cleared in `onExit` would leave these set here.
        model.loadObjects(tab)

        XCTAssertNil(tab.objectSelection)
        XCTAssertNil(tab.objectDetailTable)
    }
}
