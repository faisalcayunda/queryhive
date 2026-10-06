import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The Record panel (W10-T5, blueprint w10 §7): which fields a row has and in what order, what the
/// search and "Edited only" keep, how staged and added rows are marked, where a long value is cut,
/// and that "Show in grid" puts the cursor on the field's cell.
@MainActor
final class RecordPanelTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let columns = [
        Event.Column(name: "kode_wilayah", type: "varchar"),
        Event.Column(name: "nama", type: "varchar"),
        Event.Column(name: "jumlah_jiwa", type: "bigint"),
        Event.Column(name: "aktif", type: "boolean"),
        Event.Column(name: "catatan", type: "json"),
    ]

    private let rows: [[String?]] = [
        ["32.01.01.2001", "KPM Sukamaju", "4", "true", nil],
        ["32.01.01.2002", "KPM Cibadak", "2", "true", #"{"a":1}"#],
        ["32.01.01.2003", "KPM Mekarsari", "7", "false", nil],
    ]

    private func tab(columns: [Event.Column]? = nil, rows: [[String?]]? = nil) -> QueryTab {
        TestStores.ensureConfigured()
        let tab = QueryTab(title: "Record")
        tab.showRows(columns: columns ?? self.columns, rows: rows ?? self.rows)
        return tab
    }

    private func fields(_ tab: QueryTab, row: Int) -> [RecordField] {
        RecordFields.fields(row: row, columns: tab.preview?.columns ?? [], layout: tab.columnLayout,
                            edits: tab.cellEdits)
    }

    // MARK: Which fields, in what order

    func testOneFieldPerColumnInTheOrderTheGridDraws() {
        let tab = tab()
        XCTAssertEqual(fields(tab, row: 1).map(\.name),
                       ["kode_wilayah", "nama", "jumlah_jiwa", "aktif", "catatan"])
        XCTAssertEqual(fields(tab, row: 1).map(\.type), ["varchar", "varchar", "bigint", "boolean", "json"])
    }

    func testAHiddenColumnHasNoFieldAndAMovedOneIsWhereItWasMoved() {
        let tab = tab()
        tab.setColumnHidden(3, true)
        tab.moveColumn(from: 0, to: 3)
        tab.renameColumn(1, to: "Nama KPM")

        let shown = fields(tab, row: 0)
        XCTAssertEqual(shown.map(\.name), ["Nama KPM", "jumlah_jiwa", "catatan", "kode_wilayah"])
        XCTAssertEqual(shown.map(\.source), [1, 2, 4, 0], "the source column is what the value is read by")
        XCTAssertEqual(shown.map(\.position), [0, 1, 2, 3], "the position is what the grid's cursor counts")
        XCTAssertEqual(shown[0].columnName, "nama", "a display format is filed under the server's name")
    }

    func testTheValuesComeInDrawnOrderToo() throws {
        let tab = tab()
        tab.setColumnHidden(3, true)
        tab.moveColumn(from: 0, to: 3)
        let drawn = fields(tab, row: 2)

        let fetched = try RecordFields.readRow(tab.result, row: 2, sources: tab.visibleColumnSources).get()
        XCTAssertEqual(fetched, ["KPM Mekarsari", "7", nil, "32.01.01.2003"])
        let values = drawn.map { RecordFields.value(of: $0, row: 2, fetched: fetched, edits: tab.cellEdits) }
        XCTAssertEqual(values, [.text("KPM Mekarsari"), .text("7"), .null, .text("32.01.01.2003")])
    }

    // MARK: Find field and Edited only

    func testFindFieldMatchesTheNameWithoutCaseAndWithoutReadingAValue() {
        let tab = tab()
        let all = fields(tab, row: 0)
        XCTAssertEqual(RecordFields.filtered(all, matching: "JIWA", editedOnly: false).map(\.name), ["jumlah_jiwa"])
        XCTAssertEqual(RecordFields.filtered(all, matching: " a", editedOnly: false).map(\.name),
                       ["kode_wilayah", "nama", "jumlah_jiwa", "aktif", "catatan"].filter { $0.contains("a") })
        XCTAssertEqual(RecordFields.filtered(all, matching: "", editedOnly: false), all)
        XCTAssertEqual(RecordFields.filtered(all, matching: "tidak ada", editedOnly: false), [])
        // The search is over the label the grid draws, so a renamed column is found by its new name.
        tab.renameColumn(1, to: "Nama KPM")
        XCTAssertEqual(RecordFields.filtered(fields(tab, row: 0), matching: "kpm", editedOnly: false).map(\.source), [1])
    }

    func testEditedOnlyKeepsTheStagedFieldsAndComposesWithTheSearch() {
        let tab = tab()
        tab.cellEdits.edit("KPM Cibadak Baru", at: CellKey(row: 1, column: 1), original: "KPM Cibadak")
        tab.cellEdits.edit("9", at: CellKey(row: 1, column: 2), original: "2")

        let all = fields(tab, row: 1)
        XCTAssertEqual(RecordFields.filtered(all, matching: "", editedOnly: true).map(\.name), ["nama", "jumlah_jiwa"])
        XCTAssertEqual(RecordFields.filtered(all, matching: "jiwa", editedOnly: true).map(\.name), ["jumlah_jiwa"])
        XCTAssertEqual(RecordFields.filtered(fields(tab, row: 0), matching: "", editedOnly: true), [],
                       "another row has no staged field")
    }

    // MARK: Staged marks, added rows

    func testAStagedFieldIsModifiedAndShowsTheStagedText() throws {
        let tab = tab()
        tab.cellEdits.edit("KPM Cibadak Baru", at: CellKey(row: 1, column: 1), original: "KPM Cibadak")

        let all = fields(tab, row: 1)
        XCTAssertEqual(all.map(\.state), [.unchanged, .modified, .unchanged, .unchanged, .unchanged])
        let fetched = try RecordFields.readRow(tab.result, row: 1, sources: tab.visibleColumnSources).get()
        XCTAssertEqual(RecordFields.value(of: all[1], row: 1, fetched: fetched, edits: tab.cellEdits),
                       .text("KPM Cibadak Baru"))
        XCTAssertEqual(RecordFields.value(of: all[0], row: 1, fetched: fetched, edits: tab.cellEdits),
                       .text("32.01.01.2002"))
    }

    func testADeletedRowMarksEveryField() {
        let tab = tab()
        tab.cellEdits.deleteRow(2)
        XCTAssertEqual(Set(fields(tab, row: 2).map(\.state)), [.deleted])
        XCTAssertEqual(Set(fields(tab, row: 0).map(\.state)), [.unchanged])
    }

    func testAnAddedRowShowsWhatWasTypedAndDEFAULTForTheRest() {
        let tab = tab()
        let id = tab.cellEdits.insertRow()
        tab.cellEdits.setInserted("KPM Baru", row: id, column: 1)

        let all = fields(tab, row: id)
        XCTAssertEqual(Set(all.map(\.state)), [.inserted])
        let values = all.map { RecordFields.value(of: $0, row: id, fetched: [], edits: tab.cellEdits) }
        XCTAssertEqual(values, [.defaulted, .text("KPM Baru"), .defaulted, .defaulted, .defaulted])
        let shown = RecordFields.shown(.defaulted, type: "varchar", format: .raw, nullDisplay: "null", expanded: false)
        XCTAssertEqual(shown, .init(text: "DEFAULT", kind: .defaulted, cut: false))
    }

    // MARK: What a value looks like

    func testNullShowsTheUsersNullTextAndEmptyIsNamed() {
        XCTAssertEqual(RecordFields.shown(.null, type: "text", format: .raw, nullDisplay: "∅", expanded: false),
                       .init(text: "∅", kind: .null, cut: false))
        XCTAssertEqual(RecordFields.shown(.null, type: "text", format: .raw, nullDisplay: "", expanded: false).text,
                       "NULL", "a blank setting would draw nothing, and nothing reads as an empty string")
        XCTAssertEqual(RecordFields.shown(.text(""), type: "text", format: .raw, nullDisplay: "null", expanded: false).kind,
                       .empty)
    }

    func testNullAndEmptyAreNeverTheStringsThatSpellThem() {
        let null = RecordFields.shown(.null, type: "text", format: .raw, nullDisplay: "null", expanded: false)
        let word = RecordFields.shown(.text("null"), type: "text", format: .raw, nullDisplay: "null", expanded: false)
        XCTAssertEqual(null.kind, .null)
        XCTAssertEqual(word.kind, .value)
        XCTAssertTrue(null.kind.italic, "NULL is italic, as in the grid")
        XCTAssertFalse(word.kind.italic, "the string 'null' is upright")

        let empty = RecordFields.shown(.text(""), type: "text", format: .raw, nullDisplay: "null", expanded: false)
        let spelled = RecordFields.shown(.text("empty"), type: "text", format: .raw, nullDisplay: "null", expanded: false)
        XCTAssertEqual(empty, .init(text: "\u{2205}", kind: .empty, cut: false), "an empty string is the grid's ∅")
        XCTAssertEqual(spelled, .init(text: "empty", kind: .value, cut: false))
        XCTAssertNotEqual(empty.text, spelled.text)
        XCTAssertTrue(RecordFields.Shown.Kind.defaulted.italic)
        XCTAssertFalse(RecordFields.Shown.Kind.empty.italic)

        let field = RecordField(source: 0, position: 0, name: "c", columnName: "c", type: "text", state: .unchanged)
        XCTAssertEqual(RecordFields.accessibilityLabel(field, shown: empty), "c, text: empty", "VoiceOver keeps the word")
        XCTAssertEqual(RecordFields.accessibilityLabel(field, shown: null), "c, text: null")
    }

    func testTheColumnsDisplayFormatApplies() {
        let shown = RecordFields.shown(.text("550e8400e29b41d4a716446655440000"), type: "uuid", format: .uuid,
                                       nullDisplay: "null", expanded: false)
        XCTAssertEqual(shown.text, "550e8400-e29b-41d4-a716-446655440000")
    }

    // MARK: The six-row cut

    func testAValueOfSixRowsIsShownWholeAndASeventhIsCut() {
        let six = (1...6).map { "line \($0)" }.joined(separator: "\n")
        XCTAssertEqual(RecordFields.clip(six).text, six)
        XCTAssertFalse(RecordFields.clip(six).cut)

        let seven = six + "\nline 7"
        let clipped = RecordFields.clip(seven)
        XCTAssertEqual(clipped.text, six)
        XCTAssertTrue(clipped.cut)
    }

    func testALongSingleLineCountsTheRowsItWrapsOnto() {
        let width = RecordFields.charsPerLine
        let fits = String(repeating: "x", count: width * 6)
        XCTAssertEqual(RecordFields.clip(fits).text, fits)
        XCTAssertFalse(RecordFields.clip(fits).cut)

        let over = String(repeating: "x", count: width * 6 + 1)
        let clipped = RecordFields.clip(over)
        XCTAssertTrue(clipped.cut)
        XCTAssertEqual(clipped.text.count, width * 6)
    }

    func testAHugeValueIsCutWithoutBeingScanned() {
        let huge = String(repeating: "y", count: 20_000_000)
        let started = Date()
        let clipped = RecordFields.clip(huge)
        XCTAssertTrue(clipped.cut)
        XCTAssertLessThanOrEqual(clipped.text.count, RecordFields.maxLines * RecordFields.charsPerLine)
        XCTAssertLessThan(Date().timeIntervalSince(started), 0.5)
    }

    /// Draws one field at the panel's real width and reports whether the row limit hid the end of it.
    private func overflow(of text: String, rowWidth: CGFloat = 316) -> (estimateCuts: Bool, hidden: Bool) {
        let field = RecordField(source: 0, position: 0, name: "alamat", columnName: "alamat", type: "text",
                                state: .unchanged)
        var hidden = false
        // 340 pt panel, 12 pt list padding and 6 pt row padding on each side: a 316 pt row, 15 pt
        // narrower with legacy scrollbars.
        let host = NSHostingView(rootView: RecordFieldRow(
            field: field, value: .text(text), format: .raw, expanded: false, showInGrid: {}, more: {},
            overflowChanged: { hidden = $0 })
            .frame(width: rowWidth))
        host.frame = CGRect(x: 0, y: 0, width: rowWidth, height: 400)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.contentView = nil; window.close() }
        for _ in 0..<4 {
            host.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        }
        let shown = RecordFields.shown(.text(text), type: "text", format: .raw, nullDisplay: "null", expanded: false)
        return (shown.cut, hidden)
    }

    func testAWordWrappedValueTheEstimateMissesStillOffersShowMore() {
        // Words of 8 to 12 letters leave 3 to 8 columns unused at the end of most rows, so 235
        // characters, which the estimate puts on 6 rows, need 7 at every width from 289 to 304 pt.
        let address = "Mekarwangi sebelah Pemerintah Kelurahan Sukamaju sebelah Kelurahan Sukamaju Kebonpedes "
            + "Perumahan Sukabumi Sukabumi Sukabumi sebelah sebelah Kebonpedes Mekarwangi Residence Pemerintah "
            + "Sukamaju Tanjungsari Tanjungsari Bojongloa Perumahan"
        XCTAssertEqual(address.count, 235)
        for width in [301, 316] as [CGFloat] {
            let result = overflow(of: address, rowWidth: width)
            XCTAssertFalse(result.estimateCuts, "the character estimate alone calls this value whole")
            XCTAssertTrue(result.hidden, "but at \(width) pt the layout hides its end, so the row owes a Show more")
        }
    }

    func testAnUnbrokenValueTheEstimateMissesStillOffersShowMore() {
        // With legacy scrollbars a row holds 39 characters, so 240 take 7 rows, not the estimated 6.
        let result = overflow(of: String(repeating: "x", count: 240), rowWidth: 301)
        XCTAssertFalse(result.estimateCuts)
        XCTAssertTrue(result.hidden)
    }

    func testAValueThatFitsOffersNoShowMore() {
        XCTAssertFalse(overflow(of: "KPM Sukamaju").hidden)
        XCTAssertFalse(overflow(of: (1...6).map { "line \($0)" }.joined(separator: "\n")).hidden)
    }

    func testShowMoreOpensTheValueInPlaceUnlessItBelongsToTheReader() {
        let long = (1...20).map { "line \($0)" }.joined(separator: "\n")
        let cut = RecordFields.shown(.text(long), type: "text", format: .raw, nullDisplay: "null", expanded: false)
        XCTAssertTrue(cut.cut)
        XCTAssertFalse(RecordFields.wantsReader(long, type: "text"))
        let open = RecordFields.shown(.text(long), type: "text", format: .raw, nullDisplay: "null", expanded: true)
        XCTAssertEqual(open.text, long, "expanded is the whole value, and there is still a Show less")
        XCTAssertTrue(open.cut)

        XCTAssertTrue(RecordFields.wantsReader(#"{"a":1}"#, type: "json"), "structured values open the reader")
        XCTAssertTrue(RecordFields.wantsReader("00ff", type: "bytea"), "so do bytes")
        XCTAssertTrue(RecordFields.wantsReader(String(repeating: "z", count: RecordFields.expandLimit + 1), type: "text"),
                      "and so does text too long for a list of fields")
        XCTAssertFalse(RecordFields.wantsReader("plain", type: "varchar"))
    }

    // MARK: Reading a row

    func testFiveHundredColumnsMakeFiveHundredFieldsAndOneReadOfTheRow() throws {
        let wide = (0..<500).map { Event.Column(name: "col_\($0)", type: "text") }
        let row = (0..<500).map { "v\($0)" as String? }
        let tab = tab(columns: wide, rows: [row])

        let all = fields(tab, row: 0)
        XCTAssertEqual(all.count, 500)
        XCTAssertEqual(RecordFields.filtered(all, matching: "col_49", editedOnly: false).count, 11,
                       "col_49 and col_490…col_499: the search runs over names that are already here")
        let fetched = try RecordFields.readRow(tab.result, row: 0, sources: tab.visibleColumnSources).get()
        XCTAssertEqual(fetched.count, 500)
        XCTAssertEqual(fetched[499], "v499")
    }

    func testARowThatCannotBeReadIsAFailureAndNeverABlankField() {
        let tab = tab()
        XCTAssertNotNil(failure(RecordFields.readRow(tab.result, row: 9, sources: [0, 1])), "past the last row")
        XCTAssertNotNil(failure(RecordFields.readRow(tab.result, row: -1, sources: [0])))

        let store = tab.activeResult
        store?.release()
        let released = RecordFields.readRow(tab.result, row: 0, sources: [0, 1])
        XCTAssertNotNil(failure(released), "a released store throws, and that is not a row of NULLs")
    }

    private func failure(_ result: Result<[String?], RecordReadFailure>) -> RecordReadFailure? {
        if case .failure(let failure) = result { return failure }
        return nil
    }

    func testTheRowCacheReadsOnceUntilTheKeyMoves() {
        let cache = RecordRowCache()
        var reads = 0
        func read() -> Result<[String?], RecordReadFailure> { reads += 1; return .success(["a"]) }
        let key = RecordRowCache.Key(row: 1, sources: [0], revision: 0, fetched: 3)
        _ = cache.values(for: key, read: read)
        _ = cache.values(for: key, read: read)
        XCTAssertEqual(reads, 1, "typing in Find field does not read the row again")
        _ = cache.values(for: .init(row: 2, sources: [0], revision: 0, fetched: 3), read: read)
        _ = cache.values(for: .init(row: 2, sources: [0], revision: 1, fetched: 3), read: read)
        XCTAssertEqual(reads, 3, "another row, or the same row after the grid's rows changed, is read again")
    }

    // MARK: Accessibility

    func testEachFieldReadsAsOneSentenceWithItsStatus() {
        let field = RecordField(source: 1, position: 1, name: "nama", columnName: "nama", type: "varchar", state: .modified)
        let shown = RecordFields.Shown(text: "KPM Cibadak Baru", kind: .value, cut: false)
        XCTAssertEqual(RecordFields.accessibilityLabel(field, shown: shown), "nama, varchar: KPM Cibadak Baru, edited")

        let plain = RecordField(source: 1, position: 1, name: "nama", columnName: "nama", type: "varchar", state: .unchanged)
        XCTAssertEqual(RecordFields.accessibilityLabel(plain, shown: .init(text: "x", kind: .null, cut: false)),
                       "nama, varchar: null")
        let added = RecordField(source: 1, position: 1, name: "nama", columnName: "nama", type: "varchar", state: .inserted)
        XCTAssertEqual(RecordFields.accessibilityLabel(added, shown: .init(text: "DEFAULT", kind: .defaulted, cut: false)),
                       "nama, varchar: default, added")
    }

    func testTheAnnouncementCountsRowsFromOne() {
        XCTAssertEqual(RecordFields.announcement(row: 11), "Record: row 12")
        XCTAssertEqual(RecordFields.announcement(row: -1), "Record: new row")
    }

    // MARK: Show in grid

    func testShowInGridPutsTheCursorOnTheFieldsDrawnCell() throws {
        let tab = tab()
        tab.setColumnHidden(1, true)
        tab.moveColumn(from: 0, to: 2)   // drawn: jumlah_jiwa, aktif, kode_wilayah, catatan
        _ = tab.cellSelection
        tab.selectCells(anchor: CellPos(row: 2, column: 0), focus: CellPos(row: 2, column: 0))

        let field = try XCTUnwrap(fields(tab, row: 2).first { $0.name == "kode_wilayah" })
        let key = tab.placeCursor(on: field)

        XCTAssertEqual(key, CellKey(row: 2, column: 0), "keyed by the source column")
        XCTAssertEqual(tab.cellCursor?.focus, CellPos(row: 2, column: field.position), "counted in drawn positions")
        XCTAssertEqual(tab.cellSelection, CellRange(from: (2, field.position), to: (2, field.position)))
    }

    func testShowInGridHasNoPlaceForAnAddedRowYet() throws {
        let tab = tab()
        let id = tab.cellEdits.insertRow()
        tab.cellCursor = GridCursor(anchor: CellPos(row: id, column: 0), focus: CellPos(row: id, column: 0))
        let field = try XCTUnwrap(fields(tab, row: id).first)
        XCTAssertNil(tab.placeCursor(on: field))
        XCTAssertEqual(tab.cellCursor?.focus.row, id, "and the cursor stays where it was")
    }

    // MARK: Mode and menu

    func testRecordModeIsPerTabAndStartsOff() {
        let first = tab()
        let second = tab()
        XCTAssertFalse(first.recordMode)
        first.recordMode = true
        XCTAssertTrue(first.recordMode)
        XCTAssertFalse(second.recordMode)
    }

    func testTheViewMenuToggleRecordHasItsKeyInBothSchemes() throws {
        for scheme in ShortcutScheme.allCases {
            let spec = try XCTUnwrap(AppMenu.specs(for: scheme).first { $0.id == ShortcutAction.toggleRecord.rawValue })
            XCTAssertEqual(spec.shortcut?.display, "⌥⌘I")
            XCTAssertEqual(spec.group, .view)
        }
    }

    func testTheMenuItemTogglesTheSelectedTabsRecordMode() throws {
        let model = AppModel(persistsSession: false)
        model.tabs = []
        model.newTab()
        let tab = try XCTUnwrap(model.selectedTab)
        let spec = try XCTUnwrap(AppMenu.specs(for: .queryhive).first { $0.id == ShortcutAction.toggleRecord.rawValue })

        XCTAssertTrue(AppMenu.isEnabled(spec, in: model))
        AppMenu.perform(spec, in: model)
        XCTAssertTrue(tab.recordMode)
        XCTAssertEqual(Announcer.last, "Record view shown")
        AppMenu.perform(spec, in: model)
        XCTAssertFalse(tab.recordMode)
        XCTAssertEqual(Announcer.last, "Record view hidden")
    }

    // MARK: The panel on screen

    /// The grid's width in a window, with the panel up or not. The panel is 340 and a rule: the grid
    /// gives up 341 points to it, and does so in Record mode with the Data pane's setting off and
    /// nothing chosen, which is the case the Cell reader never covers.
    func testRecordModeStandsTheWholePanelUpWithoutTheDataPanesSettingOrASelection() throws {
        let before = DataPreferences.shared.autoShowInspector
        DataPreferences.shared.pin(autoShowInspector: false)
        defer { DataPreferences.shared.pin(autoShowInspector: before) }
        let model = AppModel(persistsSession: false)
        let tab = tab()
        model.tabs = [tab]
        model.selectedTabID = tab.id

        let host = NSHostingView(rootView: AnyView(ResultGrid(tab: tab).environment(model)
            .frame(width: 1_000, height: 500)))
        host.frame = CGRect(x: 0, y: 0, width: 1_000, height: 500)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.contentView = nil; window.close() }

        func tableWidth() -> CGFloat? {
            host.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            host.layoutSubtreeIfNeeded()
            func find(_ view: NSView) -> GridTableView? {
                if let hit = view as? GridTableView { return hit }
                for sub in view.subviews { if let hit = find(sub) { return hit } }
                return nil
            }
            return find(host).map { $0.enclosingScrollView?.frame.width ?? $0.frame.width }
        }

        let without = try XCTUnwrap(tableWidth())
        XCTAssertNil(tab.cellSelection)
        tab.recordMode = true
        let with = try XCTUnwrap(tableWidth())
        XCTAssertEqual(without - with, 341, accuracy: 1)
        tab.recordMode = false
        XCTAssertEqual(try XCTUnwrap(tableWidth()), without, accuracy: 1, "leaving Record gives the room back")
    }
}
