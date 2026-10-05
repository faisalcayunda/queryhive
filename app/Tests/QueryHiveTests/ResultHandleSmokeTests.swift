import XCTest
import QueryHiveFFI

@testable import QueryHive

/// The data plane's FFI surface, seen from Swift: a `ResultHandle` made from rows, read back through
/// `window`, `cellText` and `setView`, then released (W5-T2, blueprint fase-6 sections 9.2 and 12).
///
/// What it guards is the boundary and not the store: the `QHW1` layout decodes here the way the grid
/// will decode it, every call after `release` is a thrown `StaleHandle` instead of a trap, and a bad
/// request is a thrown `InvalidArgument`. The store's own semantics are the Rust tests' business.
final class ResultHandleSmokeTests: XCTestCase {
    private func configuredHost() throws -> EngineHost {
        let host = EngineHost()
        let sweep = try host.configureResultStores(spillDir: nil, budgetBytes: 64 << 20)
        XCTAssertFalse(sweep.spillEnabled, "no spill directory was given")
        return host
    }

    private func store(_ host: EngineHost, rows: [[String?]]) throws -> ResultHandle {
        let columns = (0..<(rows.first?.count ?? 0)).map { ColumnWire(name: "c\($0)", typeName: "text") }
        return try host.storeFromRows(columns: columns, rows: rows)
    }

    /// One decoded `QHW1` buffer. Reads with `loadUnaligned`, as the grid does.
    private struct Window {
        var flags: UInt16
        var firstRow: UInt32
        var rowCount: Int
        var columnCount: Int
        var visibleTotal: UInt32
        var sourceRows: [UInt32]
        var cells: [(text: String?, flags: UInt8)]

        init(_ data: Data) {
            func u32(_ at: Int) -> UInt32 {
                data.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: at, as: UInt32.self) }
            }
            func u16(_ at: Int) -> UInt16 {
                data.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: at, as: UInt16.self) }
            }
            precondition(Array(data[0..<4]) == Array("QHW1".utf8), "magic")
            precondition(u16(4) == 1, "version")
            flags = u16(6)
            firstRow = u32(8)
            rowCount = Int(u32(12))
            columnCount = Int(u32(16))
            visibleTotal = u32(20)
            let heapLength = Int(u32(24))
            let s0 = 32 + 4 * rowCount
            let s1 = s0 + 4 * (rowCount * columnCount + 1)
            let s2 = s1 + rowCount * columnCount
            precondition(data.count == s2 + heapLength, "the length is S2 + H")
            sourceRows = (0..<rowCount).map { u32(32 + 4 * $0) }
            var previous = Int(u32(s0))
            var decoded: [(text: String?, flags: UInt8)] = []
            for k in 0..<(rowCount * columnCount) {
                let end = Int(u32(s0 + 4 * (k + 1)))
                precondition(end >= previous && end <= heapLength, "offsets are monotone and inside the heap")
                let flag = data[s1 + k]
                let text = String(decoding: data[(s2 + previous)..<(s2 + end)], as: UTF8.self)
                decoded.append((flag & 1 == 0 ? text : nil, flag))
                previous = end
            }
            cells = decoded
        }

        func text(_ row: Int, _ column: Int) -> String? { cells[row * columnCount + column].text }
    }

    func testAWindowDecodesTheWayTheGridWillRead() throws {
        let host = try configuredHost()
        let handle = try store(host, rows: [
            ["a0", "x", nil],
            ["a1", "", "tab\tand\nnewline"],
            ["a2", "NULL", "unicode: é中😀"],
            ["a3", "y", nil],
            ["a4", "z", "last"],
        ])
        let count = try handle.rowCount()
        XCTAssertEqual(count.fetched, 5)
        XCTAssertEqual(count.visible, 5)
        XCTAssertEqual(count.viewId, 0, "no view yet: the identity view")
        XCTAssertEqual(count.phase, .complete)
        XCTAssertEqual(try handle.columns().map(\.name), ["c0", "c1", "c2"])

        let data = try handle.window(viewId: 0, firstRow: 1, rowCount: 3, columns: [0, 2], formats: [.raw, .raw])
        let window = Window(data)
        XCTAssertEqual(window.rowCount, 3)
        XCTAssertEqual(window.columnCount, 2)
        XCTAssertEqual(window.firstRow, 1)
        XCTAssertEqual(window.visibleTotal, 5)
        XCTAssertEqual(window.sourceRows, [1, 2, 3])
        XCTAssertEqual(window.flags & 0b11, 0b11, "CUT and COMPLETE")
        XCTAssertEqual(window.text(0, 0), "a1")
        XCTAssertEqual(window.text(0, 1), "tab\tand\nnewline")
        XCTAssertEqual(window.text(1, 1), "unicode: é中😀")
        XCTAssertNil(window.text(2, 1), "a NULL is flagged, not an empty string")

        // Rows past the end are clamped, not an error.
        let tail = Window(try handle.window(viewId: 0, firstRow: 4, rowCount: 50, columns: [0], formats: [.raw]))
        XCTAssertEqual(tail.sourceRows, [4])
        let beyond = Window(try handle.window(viewId: 0, firstRow: 99, rowCount: 5, columns: [0], formats: [.raw]))
        XCTAssertEqual(beyond.rowCount, 0)
    }

    func testCellTextIsTheFullTextAndNilIsNull() throws {
        // A second row keeps every column mixed: a chunk-column that is NULL in all of its rows is a
        // known W4-T3 render defect (see `store_sink.rs`, `an_all_null_chunk_column_is_flagged_null`).
        let handle = try store(try configuredHost(), rows: [["", nil, "NULL"], ["x", "y", "z"]])
        XCTAssertEqual(try handle.cellText(viewId: 0, row: 0, column: 0, format: .raw), "")
        XCTAssertNil(try handle.cellText(viewId: 0, row: 0, column: 1, format: .raw))
        XCTAssertEqual(try handle.cellText(viewId: 0, row: 0, column: 2, format: .raw), "NULL")
        XCTAssertThrowsError(try handle.cellText(viewId: 0, row: 7, column: 0, format: .raw)) { error in
            guard case .InvalidArgument = error as? StoreFfiError else {
                return XCTFail("expected InvalidArgument, got \(error)")
            }
        }
    }

    func testAViewReordersTheRowsAndTheOldViewIdGoesStale() throws {
        let handle = try store(try configuredHost(), rows: [["10"], ["9"], ["2"], [nil]])
        let info = try handle.setView(spec: ViewSpec(
            sort: SortSpec(column: 0, descending: false), filters: [], search: nil))
        XCTAssertNotEqual(info.viewId, 0)
        XCTAssertEqual(info.visible, 4)
        XCTAssertEqual(info.fetched, 4)

        let sorted = Window(try handle.window(
            viewId: info.viewId, firstRow: 0, rowCount: 4, columns: [0], formats: [.raw]))
        XCTAssertEqual(sorted.flags & 0b100, 0b100, "VIEWED")
        XCTAssertEqual(sorted.sourceRows.count, 4)
        XCTAssertEqual(Set(sorted.sourceRows), [0, 1, 2, 3])
        // Numbers sort as numbers: 2, 9, 10 (NULL wherever the store puts it, but never between).
        let numbers = (0..<4).compactMap { sorted.text($0, 0) }
        XCTAssertEqual(numbers, ["2", "9", "10"])

        // The id of the view before is no longer current, and the error says which one is.
        XCTAssertThrowsError(try handle.window(viewId: 0, firstRow: 0, rowCount: 1, columns: [0], formats: [.raw])) {
            XCTAssertEqual($0 as? StoreFfiError, .StaleView(current: info.viewId))
        }

        // A filter narrows it; the picker still reads the whole column.
        let filtered = try handle.setView(spec: ViewSpec(
            sort: nil, filters: [.text(column: 0, needle: ">= 9")], search: nil))
        XCTAssertEqual(filtered.visible, 2)
        let distinct = try handle.distinctValues(column: 0, limit: 10)
        XCTAssertEqual(Set(distinct.values.compactMap { $0 }), ["10", "9", "2"])
        XCTAssertFalse(distinct.more)
    }

    func testAReleasedHandleThrowsStaleHandleInsteadOfCrashing() throws {
        let host = try configuredHost()
        let handle = try store(host, rows: [["a"], ["b"]])
        XCTAssertEqual(try host.storeStats().stores, 1)
        try handle.release()
        try handle.release()  // idempotent
        XCTAssertEqual(try host.storeStats().stores, 0)

        func assertStale<T>(_ call: () throws -> T, _ name: String) {
            XCTAssertThrowsError(try call(), name) {
                XCTAssertEqual($0 as? StoreFfiError, .StaleHandle, name)
            }
        }
        assertStale({ try handle.rowCount() }, "rowCount")
        assertStale({ try handle.columns() }, "columns")
        assertStale({ try handle.window(viewId: 0, firstRow: 0, rowCount: 1, columns: [0], formats: [.raw]) }, "window")
        assertStale({ try handle.rowsText(viewId: 0, firstRow: 0, rowCount: 1, columns: [0]) }, "rowsText")
        assertStale({ try handle.cellText(viewId: 0, row: 0, column: 0, format: .raw) }, "cellText")
        assertStale({ try handle.columnWidths() }, "columnWidths")
        assertStale({ try handle.distinctValues(column: 0, limit: 5) }, "distinctValues")
        assertStale({ try handle.setView(spec: ViewSpec(sort: nil, filters: [], search: nil)) }, "setView")
    }

    func testABadRequestThrowsInvalidArgument() throws {
        let handle = try store(try configuredHost(), rows: [["a", "b"]])
        func assertInvalid<T>(_ call: () throws -> T, _ name: String) {
            XCTAssertThrowsError(try call(), name) { error in
                guard case .InvalidArgument = error as? StoreFfiError else {
                    return XCTFail("\(name): expected InvalidArgument, got \(error)")
                }
            }
        }
        assertInvalid({ try handle.window(viewId: 0, firstRow: 0, rowCount: 1, columns: [2], formats: [.raw]) }, "column past the end")
        assertInvalid({ try handle.window(viewId: 0, firstRow: 0, rowCount: 1, columns: [0, 1], formats: [.raw]) }, "formats mismatch")
        assertInvalid({ try handle.window(viewId: 0, firstRow: 0, rowCount: 5_000, columns: [0], formats: [.raw]) }, "too many rows")
        assertInvalid({
            try handle.setView(spec: ViewSpec(sort: SortSpec(column: 9, descending: false), filters: [], search: nil))
        }, "sort column past the end")
    }

    func testTheRegistryIsConfiguredOnceAndNotImplicitly() throws {
        let host = EngineHost()
        XCTAssertThrowsError(try host.createResultStore()) { error in
            guard case .InvalidArgument(let message) = error as? StoreFfiError else {
                return XCTFail("expected InvalidArgument, got \(error)")
            }
            XCTAssertEqual(message, "result stores are not configured")
        }
        _ = try host.configureResultStores(spillDir: nil, budgetBytes: 1 << 20)
        XCTAssertThrowsError(try host.configureResultStores(spillDir: nil, budgetBytes: 1 << 20)) { error in
            guard case .InvalidArgument = error as? StoreFfiError else {
                return XCTFail("expected InvalidArgument, got \(error)")
            }
        }
        XCTAssertNoThrow(try host.createResultStore())
    }
}
