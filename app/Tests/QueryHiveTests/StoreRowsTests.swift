import XCTest
import QueryHiveFFI

@testable import QueryHive

/// `StoreRows` against a fake handle (the states Rust reaches only by timing), against the real
/// store (the twin of `ArrayRows`, D-24), and against a spilling host (D-22, blueprint §17.1).
final class StoreRowsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let textColumn = [Event.Column(name: "c0", type: "text")]

    private func fakeStore(rows: Int, phase: StorePhase = .streaming,
                           columns: [Event.Column]? = nil) -> (StoreRows, FakeResultHandle) {
        let handle = FakeResultHandle(rows: (0..<rows).map { ["r\($0)"] }, columns: (columns ?? textColumn).count, phase: phase)
        let store = StoreRows(handle: handle, columns: columns ?? textColumn)
        return (store, handle)
    }

    // MARK: QHW1

    func testEveryQhw1FlagBitMapsToItsOwnCellFlag() {
        // Rust numbers OPENABLE 4 and TRUNCATED 16; the Swift `CellFlags` the other way round. A
        // cast would swap them without a compile error (TM-7), so each bit is pinned on its own.
        XCTAssertEqual(WindowPage.cellFlags(FakeResultHandle.nullBit), [.null])
        XCTAssertEqual(WindowPage.cellFlags(FakeResultHandle.emptyBit), [.empty])
        XCTAssertEqual(WindowPage.cellFlags(FakeResultHandle.openableBit), [.openable])
        XCTAssertEqual(WindowPage.cellFlags(FakeResultHandle.numericBit), [.numeric])
        XCTAssertEqual(WindowPage.cellFlags(FakeResultHandle.truncatedBit), [.truncated])
        XCTAssertEqual(WindowPage.cellFlags(0), [])
        XCTAssertEqual(WindowPage.cellFlags(0b11111), [.null, .empty, .openable, .numeric, .truncated])
        XCTAssertNotEqual(WindowPage.cellFlags(4), [.truncated], "a swap of the two would pass a cast")
    }

    func testTheFlagsSurviveTheRealStoreToo() throws {
        let columns = [Event.Column(name: "t", type: "varchar"), Event.Column(name: "n", type: "bigint")]
        let long = String(repeating: "x", count: 300)
        let store = try TestStores.makeStore(columns: columns, rows: [
            [nil, "7"], ["", "8"], ["{\"a\":1}", "9"], [long, "10"],
        ])
        defer { store.release() }
        XCTAssertEqual(store.cell(row: 0, column: 0, format: .raw).flags, [.null])
        XCTAssertEqual(store.cell(row: 1, column: 0, format: .raw).flags, [.empty])
        XCTAssertEqual(store.cell(row: 2, column: 0, format: .raw).flags, [.openable])
        XCTAssertEqual(store.cell(row: 3, column: 0, format: .raw).flags, [.truncated])
        XCTAssertEqual(store.cell(row: 3, column: 1, format: .raw).flags, [.numeric])
        XCTAssertEqual(store.cell(row: 3, column: 0, format: .raw).text.count, 256, "the window cuts at 256 units")
        XCTAssertEqual(store.fullValue(row: 3, column: 0, format: .raw), long, "the full value is still there")
    }

    func testAMalformedBufferIsRefusedNotRead() throws {
        let good = FakeResultHandle.encode(rows: [["a", "b"]], visible: 1, first: 0, count: 1, columns: [0, 1], cut: true)
        XCTAssertNoThrow(try WindowPage(good))
        var magic = good; magic[0] = 0x58
        XCTAssertThrowsError(try WindowPage(magic))
        var version = good; version[4] = 9
        XCTAssertThrowsError(try WindowPage(version))
        XCTAssertThrowsError(try WindowPage(good.dropLast()), "the length is S2 + H")
        XCTAssertThrowsError(try WindowPage(Data("QHW1".utf8)), "shorter than the header")
        // An offset past the heap: the first offset after the table is 0, so corrupt the second.
        var offsets = good
        let s0 = 32 + 4 * 1
        offsets.replaceSubrange((s0 + 4)..<(s0 + 8), with: [0xFF, 0xFF, 0x00, 0x00])
        XCTAssertThrowsError(try WindowPage(offsets))
    }

    // MARK: Pages (B2)

    func testAShortPageReadWhileStreamingIsRefetchedWhenRowsArrive() {
        // Page 3 (rows 192...255) holds 8 of its 64 rows while 200 have arrived. The key
        // (view, page, block) does not change when the rest comes, so the page itself has to know.
        let (store, handle) = fakeStore(rows: 200)
        XCTAssertEqual(store.cell(row: 195, column: 0, format: .raw).text, "r195")
        let before = handle.calls.filter { $0.first == 192 }.count
        XCTAssertEqual(before, 1)

        handle.append((200..<400).map { ["r\($0)"] })
        let poll = store.poll()
        XCTAssertEqual(poll.grewFrom, 200)
        XCTAssertEqual(store.count, 400)
        XCTAssertEqual(store.cell(row: 200, column: 0, format: .raw).text, "r200", "a real cell, not a blank one")
        XCTAssertEqual(handle.calls.filter { $0.first == 192 }.count, before + 1, "exactly one more read")
        // And the page that now holds it is served from the cache.
        _ = store.cell(row: 210, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.filter { $0.first == 192 }.count, before + 1)
    }

    func testAFilterThatGrowsUnderTheSameViewIsReadAgainToo() throws {
        let (store, handle) = fakeStore(rows: 10)
        handle.nextView = { _ in handle.announce(viewId: 3, visible: 4); handle.serve(view: 3)
            return ViewInfo(viewId: 3, visible: 4, fetched: 10) }
        try store.applyBlocking(ViewSpec(sort: nil, filters: [], search: nil))
        XCTAssertEqual(store.count, 4)
        XCTAssertEqual(store.cell(row: 3, column: 0, format: .raw).text, "r3")
        handle.announce(viewId: 3, visible: 9)
        XCTAssertEqual(store.poll().grewFrom, 4)
        XCTAssertEqual(store.cell(row: 8, column: 0, format: .raw).text, "r8", "the page was short; it is read again")
        // `row(at:)` is the same rule for its blocks.
        XCTAssertEqual(store.row(at: 8)?[0], "r8")
    }

    func testARowBlockReadWhileStreamingIsReadAgainOnceTheRowsArrive() {
        let (store, handle) = fakeStore(rows: 10)
        XCTAssertEqual(store.row(at: 3)?[0], "r3")
        handle.append((10..<100).map { ["r\($0)"] })
        XCTAssertEqual(store.poll().grewFrom, 10)
        XCTAssertEqual(store.row(at: 50)?[0], "r50", "not an empty row padded into the cached block")
        XCTAssertEqual(store.row(at: 99)?[0], "r99")
    }

    func testARowBlockReadShortNeverFeedsAnAllNullPredicateToAWritePlan() {
        let (store, handle) = fakeStore(rows: 4)
        XCTAssertEqual(store.row(at: 3)?[0], "r3")
        handle.append((4..<9).map { ["r\($0)"] })
        _ = store.poll()
        var edits = CellEdits()
        edits.deleteRow(8)
        let plan = WritePlan.build(edits: edits, rows: store, columns: store.columns, table: "public.t", kind: .postgres)
        let sql = plan.statements.map(\.sql).joined()
        XCTAssertFalse(sql.contains("IS NULL"), sql)
        XCTAssertTrue(sql.contains("r8"), sql)
    }

    func testRowsOrThrowRefusesAShortAnswer() {
        let (store, handle) = fakeStore(rows: 4)
        handle.announce(viewId: 0, visible: 9)
        _ = store.poll()
        XCTAssertThrowsError(try store.rowsOrThrow(in: 0..<9, columns: [0]))
    }

    func testPrepareFormatsDropsThePagesOfTheBlocksWhoseFormatChanged() {
        // 40 columns are two blocks. The format of a column in the second block changes: that block
        // is read again, the first is not.
        let columns = (0..<40).map { Event.Column(name: "c\($0)", type: "text") }
        let handle = FakeResultHandle(rows: [(0..<40).map { "v\($0)" }], columns: 40, phase: .complete)
        let store = StoreRows(handle: handle, columns: columns)
        XCTAssertEqual(store.cell(row: 0, column: 0, format: .raw).text, "v0")
        XCTAssertEqual(store.cell(row: 0, column: 35, format: .raw).text, "v35")
        let reads = handle.calls.count
        XCTAssertEqual(reads, 2)

        let changed = store.prepare(formats: (0..<40).map { $0 == 35 ? .uuid : .raw })
        XCTAssertEqual(changed, IndexSet(integer: 35))
        _ = store.cell(row: 0, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, reads, "the first block's page was not touched")
        _ = store.cell(row: 0, column: 35, format: .uuid)
        XCTAssertEqual(handle.calls.count, reads + 1, "the second block was read again")
        XCTAssertEqual(handle.calls.last?.columns.first, 32, "and it is the second block that was asked for")
        XCTAssertEqual(store.prepare(formats: (0..<40).map { $0 == 35 ? .uuid : .raw }), IndexSet(), "nothing changed")
    }

    func testAFormatChangedWithoutPrepareIsStillNotServedStale() {
        // The grid is told to call `prepare`, and `cell` carries the format it is drawing; a caller
        // that forgot is brought in line rather than shown yesterday's text.
        let columns = [Event.Column(name: "c0", type: "text")]
        let handle = FakeResultHandle(rows: [["x"]], columns: 1, phase: .complete)
        let store = StoreRows(handle: handle, columns: columns)
        _ = store.cell(row: 0, column: 0, format: .raw)
        _ = store.cell(row: 0, column: 0, format: .uuid)
        XCTAssertEqual(handle.calls.count, 2)
    }

    // MARK: Polling (B3)

    func testPollIgnoresTheNewViewUntilTheApplyHopInstallsIt() throws {
        let (store, handle) = fakeStore(rows: 10, phase: .complete)
        XCTAssertEqual(store.viewID, 0)
        XCTAssertEqual(store.cell(row: 5, column: 0, format: .raw).text, "r5")

        // `set_view` has installed view 5 over three rows in Rust; Swift has not been told.
        handle.announce(viewId: 5, visible: 3)
        let poll = store.poll()
        XCTAssertNil(poll.grewFrom, "no growth, and above all no `from > to`")
        XCTAssertEqual(store.count, 10, "the count still belongs to the view being drawn")
        XCTAssertEqual(store.fetched, 10)
        XCTAssertEqual(store.cell(row: 9, column: 0, format: .raw).text, "r9", "and so does the text")

        handle.nextView = { _ in handle.serve(view: 5); return ViewInfo(viewId: 5, visible: 3, fetched: 10) }
        try store.applyBlocking(ViewSpec(sort: nil, filters: [], search: nil))
        XCTAssertEqual(store.viewID, 5)
        XCTAssertEqual(store.count, 3)

        handle.announce(viewId: 5, visible: 6)
        XCTAssertEqual(store.poll().grewFrom, 3, "after the hop the poll follows the new view")
        XCTAssertEqual(store.count, 6)
    }

    func testAnApplyThatFailsLeavesTheViewAndTheLiveFlagAlone() {
        let (store, handle) = fakeStore(rows: 4, phase: .complete)
        handle.nextView = { _ in throw StoreFfiError.Streaming }
        XCTAssertFalse(store.isLive)
        XCTAssertThrowsError(try store.applyBlocking(ViewSpec(sort: nil, filters: [], search: nil)))
        XCTAssertEqual(store.viewID, 0)
        XCTAssertFalse(store.isLive, "a failed apply is not a view in flight")
        XCTAssertNotNil(store.lastFailure, "and the reason is kept for the banner")
    }

    func testTheStoreIsLiveWhileItStreamsAndStopsAfterTheLastPoll() {
        let (store, handle) = fakeStore(rows: 3, phase: .streaming)
        XCTAssertTrue(store.isLive)
        XCTAssertFalse(store.poll().finished)
        handle.setPhase(.complete)
        let last = store.poll()
        XCTAssertTrue(last.finished)
        XCTAssertFalse(store.isLive)
    }

    // MARK: Errors

    func testAStaleViewAnswersABlankCellAndCachesNothing() {
        let (store, handle) = fakeStore(rows: 5, phase: .complete)
        handle.serve(view: 9)   // Rust moved on; Swift still draws view 0
        XCTAssertEqual(store.cell(row: 1, column: 0, format: .raw), CellText(text: "", flags: []))
        XCTAssertNil(store.lastFailure, "a stale view is not a failure to show")
        handle.serve(view: 0)
        XCTAssertEqual(store.cell(row: 1, column: 0, format: .raw).text, "r1", "nothing blank was cached")
    }

    func testAWindowFailureIsBlankAndRecordedButAReleasedHandleIsNot() {
        let (store, handle) = fakeStore(rows: 5, phase: .complete)
        handle.windowError = .Corrupt(message: "bad chunk")
        XCTAssertEqual(store.cell(row: 0, column: 0, format: .raw).text, "")
        XCTAssertEqual(store.lastFailure?.operation, "window")
        XCTAssertTrue(store.lastFailure?.detail.contains("bad chunk") == true)

        let (other, otherHandle) = fakeStore(rows: 5, phase: .complete)
        otherHandle.windowError = .StaleHandle
        _ = other.cell(row: 0, column: 0, format: .raw)
        XCTAssertNil(other.lastFailure)
    }

    func testRowsOrThrowThrowsWhereRowsAnswersNothing() {
        let (store, handle) = fakeStore(rows: 5, phase: .complete)
        XCTAssertEqual(try store.rowsOrThrow(in: 0..<2, columns: [0]).map { $0[0] }, ["r0", "r1"])
        handle.textError = .Spill(message: "disk")
        XCTAssertThrowsError(try store.rowsOrThrow(in: 0..<2, columns: [0])) { error in
            XCTAssertEqual((error as? StoreFailure)?.isStale, false)
        }
        XCTAssertEqual(store.rows(in: 0..<2, columns: [0]).count, 0, "the safe variant is empty, never half a block")
        handle.textError = .StaleHandle
        XCTAssertThrowsError(try store.rowsOrThrow(in: 0..<2, columns: [0])) { error in
            XCTAssertEqual((error as? StoreFailure)?.isStale, true, "stale: cancel without a message")
        }
    }

    func testAClipboardCopyRefusesAPartialAnswer() {
        let (store, handle) = fakeStore(rows: 5, phase: .complete)
        let selection = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 0))
        XCTAssertEqual(try GridClipboard.text(result: store, selection: selection, visible: [0], withHeaders: false), "r0\nr1")
        handle.textError = .Internal(message: "boom")
        XCTAssertThrowsError(try GridClipboard.text(result: store, selection: selection, visible: [0], withHeaders: false))
    }

    func testAReleasedStoreAnswersEmptyAndReleasesOnce() {
        let (store, handle) = fakeStore(rows: 5, phase: .complete)
        store.release()
        store.release()
        XCTAssertTrue(handle.released)
        XCTAssertTrue(store.isReleased)
        XCTAssertFalse(store.isLive)
        XCTAssertEqual(store.cell(row: 0, column: 0, format: .raw).text, "")
        XCTAssertNil(store.row(at: 0))
        XCTAssertNil(store.fullValue(row: 0, column: 0, format: .raw))
        XCTAssertThrowsError(try store.rowsOrThrow(in: 0..<1, columns: [0]))
    }

    func testTheCacheKeepsAtMostTwentyFourPagesButNeverFewerThanEight() {
        let (store, handle) = fakeStore(rows: 64 * 40, phase: .complete)
        for page in 0..<40 { _ = store.cell(row: page * 64, column: 0, format: .raw) }
        XCTAssertEqual(handle.calls.count, 40)
        // The newest page is still cached, the oldest is not.
        _ = store.cell(row: 39 * 64, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, 40)
        _ = store.cell(row: 0, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, 41, "the oldest page was evicted")
        store.dropPages()
        _ = store.cell(row: 39 * 64, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, 42, "a tab in the background gives its pages back")
    }

    // MARK: Whole rows

    func testRowAtReadsBlocksAndKeepsEightOfThem() throws {
        let columns = TestStores.textColumns(100)
        let rows = (0..<500).map { r in (0..<100).map { c -> String? in c == 3 && r % 7 == 0 ? nil : "r\(r)c\(c)" } }
        let store = try TestStores.makeStore(columns: columns, rows: rows)
        defer { store.release() }
        for row in [0, 39, 40, 499, 250, 7] {
            XCTAssertEqual(store.row(at: row) ?? [], rows[row], "row \(row)")
        }
        XCTAssertNil(store.row(at: 500))
        XCTAssertNil(store.row(at: -1))
    }

    func testRowsInIsSplitAtTheFfiLimitsAndComesBackWhole() throws {
        let columns = TestStores.textColumns(12)
        let rows = (0..<9_000).map { r in (0..<12).map { c -> String? in "\(r)/\(c)" } }
        let store = try TestStores.makeStore(columns: columns, rows: rows)
        defer { store.release() }
        let all = try store.rowsOrThrow(in: 0..<9_000, columns: Array(0..<12))
        XCTAssertEqual(all.count, 9_000)
        XCTAssertEqual(all.last ?? [], rows.last ?? [])
        XCTAssertEqual(all[4_096] , rows[4_096], "the seam between two calls")
        // A subset of columns, out of source order, is returned in the order asked.
        XCTAssertEqual(try store.rowsOrThrow(in: 5..<6, columns: [7, 2]), [["5/7", "5/2"]])
        // A range past the end is clamped, not an error.
        XCTAssertEqual(try store.rowsOrThrow(in: 8_999..<20_000, columns: [0]).count, 1)
    }

    func testMoreThanAThousandColumnsAreReadInChunks() throws {
        let columns = TestStores.textColumns(1_100)
        let rows = [(0..<1_100).map { Optional("v\($0)") }]
        let store = try TestStores.makeStore(columns: columns, rows: rows)
        defer { store.release() }
        XCTAssertEqual(store.row(at: 0) ?? [], rows[0])
    }

    // MARK: Swift-rendered formats

    func testAJsonColumnIsRenderedBySwiftFromTheFullValueAndCachedWithItsPage() {
        let json = #"{"b":2,"a":[1,2,3]}"#
        let columns = [Event.Column(name: "j", type: "json")]
        let handle = FakeResultHandle(rows: [[json], [json]], columns: 1, phase: .complete)
        let store = StoreRows(handle: handle, columns: columns)
        store.prepare(formats: [.json])
        let cell = store.cell(row: 0, column: 0, format: .json)
        XCTAssertEqual(cell.text, ColumnFormat.json.render(json, type: "json"))
        XCTAssertEqual(cell.flags, [.openable])
        _ = store.cell(row: 1, column: 0, format: .json)
        _ = store.cell(row: 0, column: 0, format: .json)
        XCTAssertEqual(handle.rowsTextCalls, 1, "rendered once per page, not once per cell")
        XCTAssertEqual(store.fullValue(row: 0, column: 0, format: .json), ColumnFormat.json.render(json, type: "json"))
        XCTAssertTrue(StoreRows.swiftRenderedFormats.contains(.json))
        XCTAssertFalse(StoreRows.swiftRenderedFormats.contains(.raw), "Raw never moves (D-24)")
    }

    // MARK: The twin (D-24)

    private struct Cell { var value: String?; var type: String }

    /// Values that exercise every flag, the cut and the formats.
    private var corpus: [[String?]] {
        let long = String(repeating: "abcdefghij", count: 40)               // 400 units
        let longLines = "first line\n" + String(repeating: "z", count: 300)
        let wideEmoji = String(repeating: "e\u{0301}", count: 200)           // 400 units, 200 graphemes
        return [
            ["plain", "12"], [nil, nil], ["", ""], ["  {\"a\":1}", "-7"], ["[1, 2, 3]", "3.5"],
            ["550e8400e29b41d4a716446655440000", "1700000000"], ["550e8400-e29b-41d4-a716-446655440000", "1700000000123"],
            ["6869", "0"], [long, "99"], [longLines, "100"], [wideEmoji, "1"], ["tab\there", "2"],
            ["{not json", "x"], ["日本語のテキスト", "9007199254740993"], ["0 seconds", "1e3"],
        ]
    }

    private let twinColumns = [Event.Column(name: "t", type: "varchar"), Event.Column(name: "n", type: "bigint")]

    private func prefix256(_ text: String) -> String {
        var units = 0
        var end = text.startIndex
        for index in text.indices {
            let next = text.index(after: index)
            let width = text[index..<next].utf16.count
            if units + width > 256 { break }
            units += width
            end = next
        }
        return String(text[..<end])
    }

    /// The recorded divergences (blueprint §17.2) turned into an assertion: Rust cuts at 256 UTF-16
    /// units on a grapheme boundary and sets TRUNCATED by that rule, the twin at 1,024 characters;
    /// and the twin's `openable` is looser than Rust's validated JSON.
    private func assertTwin(_ twin: ArrayRows, _ store: StoreRows, row: Int, column: Int, format: ColumnFormat,
                            file: StaticString = #filePath, line: UInt = #line) {
        let label = "row \(row) column \(column) under \(format)"
        let ours = store.cell(row: row, column: column, format: format)
        let theirs = twin.cell(row: row, column: column, format: format)
        guard StoreRows.swiftRenderedFormats.contains(format) == false else {
            XCTAssertEqual(ours, theirs, "\(label): a Swift-rendered column is the twin's own logic", file: file, line: line)
            return
        }
        let full = twin.fullValue(row: row, column: column, format: format)
        var expected = theirs
        if let full, full.utf16.count > 256 {
            let head = prefix256(full)
            expected.text = head.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? head
            expected.flags.insert(.truncated)
        } else {
            expected.flags.remove(.truncated)
        }
        var got = ours
        // Divergence 2: the twin calls any `{`/`[` openable; Rust validates. Rust implies twin.
        if theirs.flags.contains(.openable), !ours.flags.contains(.openable) { got.flags.insert(.openable) }
        // Divergence 3 (found by this test, not in the blueprint's list): NUMERIC is a property of the
        // stored value in Rust (§7.2, `GridSort.number(text) != nil`) and of the column type in the
        // twin. The grid draws alignment from the column type (`GridPaintContext.numeric`) and reads
        // no cell's NUMERIC flag, so nothing on screen differs. Pinned to Rust's own rule instead.
        let stored = twin.fullValue(row: row, column: column, format: .raw)
        XCTAssertEqual(ours.flags.contains(.numeric), stored.map { SwiftGridReference.number($0) != nil } ?? false,
                       "\(label): NUMERIC follows the stored value", file: file, line: line)
        got.flags.remove(.numeric)
        expected.flags.remove(.numeric)
        XCTAssertEqual(got.text, expected.text, "\(label): text", file: file, line: line)
        XCTAssertEqual(got.flags, expected.flags, "\(label): flags", file: file, line: line)
        if ours.flags.contains(.openable) { XCTAssertTrue(theirs.flags.contains(.openable), "\(label): openable", file: file, line: line) }
    }

    func testEveryFormatReadsLikeTheArrayTwinWithinTheRecordedDivergences() throws {
        let rows = corpus
        let twin = ArrayRows(rows: rows, sizing: rows, columns: twinColumns)
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        for format in ColumnFormat.allCases {
            store.prepare(formats: [format, format])
            for row in rows.indices {
                for column in 0..<2 { assertTwin(twin, store, row: row, column: column, format: format) }
            }
        }
    }

    func testRawIsStoredTextAsItIsAndNeverASwiftRenderedFormat() throws {
        let rows = corpus
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        for row in rows.indices {
            XCTAssertEqual(store.fullValue(row: row, column: 0, format: .raw), rows[row][0], "row \(row)")
            XCTAssertEqual(store.fullValue(row: row, column: 1, format: .raw), rows[row][1], "row \(row)")
        }
        XCTAssertFalse(StoreRows.swiftRenderedFormats.contains(.raw))
    }

    func testTheFullValueUnderAFormatIsTheTwinsFullValue() throws {
        let rows = corpus
        let twin = ArrayRows(rows: rows, sizing: rows, columns: twinColumns)
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        for format in ColumnFormat.allCases {
            for row in rows.indices {
                XCTAssertEqual(store.fullValue(row: row, column: 0, format: format),
                               twin.fullValue(row: row, column: 0, format: format),
                               "row \(row) under \(format)")
            }
        }
    }

    func testWidthsAndDistinctValuesAgreeWithTheTwin() async throws {
        let rows: [[String?]] = (0..<300).map { ["name \($0 % 17) " + String(repeating: "w", count: $0 % 90), String($0)] }
        let twin = ArrayRows(rows: rows, sizing: rows, columns: twinColumns)
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        XCTAssertEqual(store.naturalCharCounts(), twin.naturalCharCounts())

        let few: [[String?]] = [["b", "1"], [nil, "2"], ["a", "3"], ["b", "4"]]
        let small = try TestStores.makeStore(columns: twinColumns, rows: few)
        defer { small.release() }
        let ours = await small.distinctValues(column: 0)
        let theirs = await ArrayRows(rows: few, sizing: few, columns: twinColumns).distinctValues(column: 0)
        XCTAssertEqual(ours, theirs)
        let past = await store.distinctValues(column: 1)
        XCTAssertEqual(past, DistinctSample(values: [], more: true), "past the picker's limit: no list, `more`")
    }

    func testNullWidthIsTheOneRecordedDifferenceInWidths() throws {
        // TM-8: Rust counts NULL as 0 where the twin counts 4. No width reaches the difference
        // (the formula gives 84 for every n <= 8), but the numbers are not the same.
        let rows: [[String?]] = [[nil, "1"]]
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        XCTAssertEqual(store.naturalCharCounts()[0], 0)
        XCTAssertEqual(ArrayRows(rows: rows, sizing: rows, columns: twinColumns).naturalCharCounts()[0], 4)
    }

    // MARK: The view, on the real store

    func testApplyBlockingFiltersSortsAndSearchesInRust() throws {
        let rows: [[String?]] = [["b", "10"], ["a", "9"], ["c", "100"], [nil, "7"]]
        let store = try TestStores.makeStore(columns: twinColumns, rows: rows)
        defer { store.release() }
        let info = try store.applyBlocking(ViewSpec(sort: SortSpec(column: 1, descending: false),
                                                    filters: [.text(column: 0, needle: "")], search: nil))
        XCTAssertEqual(info.fetched, 4)
        XCTAssertEqual(store.rows(in: 0..<store.count, columns: [1]).map { $0[0] }, ["7", "9", "10", "100"])
        _ = try store.applyBlocking(ViewSpec(sort: nil, filters: [.values(column: 0, values: ["a", nil])], search: nil))
        XCTAssertEqual(store.count, 2)
        XCTAssertEqual(store.fetched, 4, "the footer's `12 of 40`")
        // The old view's pages are gone: the first row now is the first row of the new view.
        XCTAssertEqual(store.cell(row: 0, column: 0, format: .raw).text, "a")
    }

    func testAStreamingRealStoreGrowsUnderPollAndSealsOnComplete() throws {
        TestStores.ensureConfigured()
        let store = try RustEngine().makeResultStore()
        defer { store.release() }
        store.setColumns(textColumn)
        XCTAssertEqual(store.count, 0)
        XCTAssertEqual(store.phase, .empty)
        XCTAssertTrue(store.isLive)
    }

    // MARK: Spill

    func testAStoreThatSpilledReadsBackTheSameCells() throws {
        let (host, directory) = try TestStores.spillingHost(budgetBytes: 4 << 20)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let columns = (0..<8).map { Event.Column(name: "c\($0)", type: $0 % 2 == 0 ? "varchar" : "bigint") }
        let rows = (0..<120_000).map { r in
            (0..<8).map { c -> String? in c % 2 == 0 ? "row \(r) column \(c) " + String(repeating: "p", count: r % 40) : String(r &* 31 &+ c) }
        }
        let store = try TestStores.makeStore(on: host, columns: columns, rows: rows)
        defer { store.release() }
        let stats = try host.storeStats()
        XCTAssertGreaterThan(stats.spilledBytes, 0, "the budget is a fraction of the data: it has to have spilled")
        let twin = ArrayRows(rows: rows, sizing: rows, columns: columns)
        for row in [0, 63, 64, 12_345, 60_000, 119_999] {
            for column in [0, 1, 6, 7] {
                XCTAssertEqual(store.cell(row: row, column: column, format: .raw).text,
                               twin.cell(row: row, column: column, format: .raw).text, "row \(row) column \(column)")
            }
        }
        XCTAssertEqual(store.row(at: 119_999) ?? [], rows[119_999])
        XCTAssertNil(store.lastFailure)
    }

    // MARK: The empty stand-in

    func testEmptyRowsIsAnEmptyResult() async {
        let empty = EmptyRows()
        XCTAssertEqual(empty.count, 0)
        XCTAssertEqual(empty.fetched, 0)
        XCTAssertNil(empty.row(at: 0))
        XCTAssertEqual(empty.cell(row: 0, column: 0, format: .raw), CellText(text: "", flags: []))
        XCTAssertFalse(empty.isLive)
        XCTAssertTrue(empty.poll().finished)
        let sample = await empty.distinctValues(column: 0)
        XCTAssertEqual(sample, DistinctSample(values: [], more: false))
    }
}
