import Foundation
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// The one-process host the tests share (blueprint §17.1, §21.4).
///
/// `RustEngine.host` is a single instance whose budget and spill directory are set once, so in the
/// test process it is configured here as `(nil, 64 MiB)`: spill off, and a corpus of a few dozen
/// rows never reaches the budget. **That host cannot spill**, whoever configures it first. A test
/// that needs a real spill, a chunk that really went to disk or a miss that reads one back builds
/// an `EngineHost()` of its own with `spillingHost(budgetBytes:)`.
enum TestStores {
    /// Configure the shared host, once. Safe to call from every test: the first caller wins.
    @discardableResult
    static func ensureConfigured() -> Bool {
        RustEngine.ensureStoresConfigured(spillDir: nil, budgetBytes: 64 << 20)
        return true
    }

    /// Stores the shared host holds right now. A test that opens tabs asserts this returns to the
    /// value it had before, which is what proves a tab let go of its store.
    static var liveStores: Int {
        ensureConfigured()
        return Int(RustEngine.storeStats()?.stores ?? 0)
    }

    /// A store of `rows` under `columns`, on the shared host.
    static func makeStore(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        ensureConfigured()
        return try RustEngine().storeFromRows(columns: columns, rows: rows)
    }

    /// `n` text columns `c0…`, for tests that do not care what the columns are called.
    static func textColumns(_ n: Int) -> [Event.Column] {
        (0..<n).map { Event.Column(name: "c\($0)", type: "text") }
    }

    /// A host of its own with a real spill directory and a small budget, and the directory, which
    /// the caller removes in `tearDown` (`addTeardownBlock { try? FileManager.default.removeItem(at: dir) }`).
    static func spillingHost(budgetBytes: UInt64) throws -> (host: EngineHost, directory: URL) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-test-spill-\(UUID().uuidString)")
        let host = EngineHost()
        let sweep = try host.configureResultStores(spillDir: directory.path, budgetBytes: budgetBytes)
        guard sweep.spillEnabled else {
            throw StoreFailure(operation: "spillingHost", detail: sweep.reason ?? "spill is off")
        }
        return (host, directory)
    }

    /// A `StoreRows` over a handle from `host`, with `columns` as the event would have set them.
    static func makeStore(on host: EngineHost, columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        let wire = columns.map { ColumnWire(name: $0.name, typeName: $0.type) }
        return StoreRows(handle: try host.storeFromRows(columns: wire, rows: rows), columns: columns)
    }
}

/// A `ResultHandleProtocol` that answers from an array, so the Swift side of the store can be driven
/// into states the Rust one reaches only by timing: a page read while the result is short, a view
/// installed under a poll, a window that fails. It writes `QHW1` itself, which also makes it the
/// independent encoder the decoder is held against.
final class FakeResultHandle: ResultHandleProtocol, @unchecked Sendable {
    struct WindowCall: Equatable { var view: UInt64; var first: Int; var rows: Int; var columns: [UInt32] }

    private let lock = NSLock()
    private var _rows: [[String?]]
    private var _visible: Int?
    private var _viewId: UInt64 = 0
    private var _phase: StorePhase
    private var _calls: [WindowCall] = []
    private var _released = false
    var columnTypes: [String]
    /// What `window` throws instead of answering, when set.
    var windowError: StoreFfiError?
    /// What `rows_text` throws instead of answering, if anything.
    var textError: StoreFfiError?
    private(set) var rowsTextCalls = 0
    /// What `set_view` answers with, when set; otherwise a new view over every row.
    var nextView: ((ViewSpec) throws -> ViewInfo)?
    var widths: [UInt32] = []
    var distinct: DistinctValues?

    init(rows: [[String?]] = [], columns: Int = 1, phase: StorePhase = .streaming) {
        _rows = rows
        _phase = phase
        columnTypes = Array(repeating: "text", count: columns)
    }

    // MARK: Test controls

    func append(_ more: [[String?]]) { lock.lock(); _rows += more; lock.unlock() }
    func setPhase(_ phase: StorePhase) { lock.lock(); _phase = phase; lock.unlock() }
    /// Make `rowCount()` answer a view that `window` does not serve yet, like Rust between
    /// `set_view` and the app's hop.
    func announce(viewId: UInt64, visible: Int) { lock.lock(); _viewId = viewId; _visible = visible; lock.unlock() }
    /// Serve `window` and `rows_text` for this view id.
    private var servedView: UInt64 = 0
    func serve(view: UInt64) { lock.lock(); servedView = view; lock.unlock() }
    var calls: [WindowCall] { lock.lock(); defer { lock.unlock() }; return _calls }
    var released: Bool { lock.lock(); defer { lock.unlock() }; return _released }
    var rowsHeld: [[String?]] { lock.lock(); defer { lock.unlock() }; return _rows }

    // MARK: ResultHandleProtocol

    func rowCount() throws -> RowCount {
        lock.lock(); defer { lock.unlock() }
        if _released { throw StoreFfiError.StaleHandle }
        return RowCount(fetched: UInt32(_rows.count), visible: UInt32(_visible ?? _rows.count),
                        viewId: _viewId, phase: _phase)
    }

    func columns() throws -> [ColumnWire] {
        columnTypes.enumerated().map { ColumnWire(name: "c\($0.offset)", typeName: $0.element) }
    }

    func window(viewId: UInt64, firstRow: UInt32, rowCount: UInt32, columns: [UInt32],
                formats: [CellFormat]) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        if _released { throw StoreFfiError.StaleHandle }
        if let windowError { throw windowError }
        _calls.append(WindowCall(view: viewId, first: Int(firstRow), rows: Int(rowCount), columns: columns))
        guard viewId == servedView else { throw StoreFfiError.StaleView(current: servedView) }
        return Self.encode(rows: _rows, visible: _visible ?? _rows.count, first: Int(firstRow), count: Int(rowCount),
                           columns: columns, cut: true, types: columnTypes)
    }

    func rowsText(viewId: UInt64, firstRow: UInt32, rowCount: UInt32, columns: [UInt32]) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        if _released { throw StoreFfiError.StaleHandle }
        rowsTextCalls += 1
        if let textError { throw textError }
        guard viewId == servedView else { throw StoreFfiError.StaleView(current: servedView) }
        return Self.encode(rows: _rows, visible: _visible ?? _rows.count, first: Int(firstRow), count: Int(rowCount),
                           columns: columns, cut: false, types: columnTypes)
    }

    func cellText(viewId: UInt64, row: UInt32, column: UInt32, format: CellFormat) throws -> String? {
        lock.lock(); defer { lock.unlock() }
        if _released { throw StoreFfiError.StaleHandle }
        guard viewId == servedView else { throw StoreFfiError.StaleView(current: servedView) }
        guard Int(row) < _rows.count else { throw StoreFfiError.InvalidArgument(message: "row") }
        return Int(column) < _rows[Int(row)].count ? _rows[Int(row)][Int(column)] : nil
    }

    func columnWidths() throws -> [UInt32] { widths }

    func setView(spec: ViewSpec) throws -> ViewInfo {
        if let nextView { return try nextView(spec) }
        lock.lock(); defer { lock.unlock() }
        _viewId += 1
        servedView = _viewId
        _visible = nil
        return ViewInfo(viewId: _viewId, visible: UInt32(_rows.count), fetched: UInt32(_rows.count))
    }

    func distinctValues(column: UInt32, limit: UInt32) throws -> DistinctValues {
        distinct ?? DistinctValues(values: [], more: false)
    }

    func release() throws {
        lock.lock(); _released = true; lock.unlock()
    }

    // MARK: QHW1

    static let nullBit: UInt8 = 1, emptyBit: UInt8 = 2, openableBit: UInt8 = 4, numericBit: UInt8 = 8,
               truncatedBit: UInt8 = 16

    /// `QHW1` for `count` rows from `first`, clamped to `visible`.
    static func encode(rows: [[String?]], visible: Int, first: Int, count: Int, columns: [UInt32], cut: Bool,
                       types: [String] = []) -> Data {
        let available = max(0, min(count, min(visible, rows.count) - first))
        var offsets: [UInt32] = [0]
        var flags: [UInt8] = []
        var heap: [UInt8] = []
        for r in 0..<available {
            let row = rows[first + r]
            for column in columns {
                let value = Int(column) < row.count ? row[Int(column)] : nil
                var flag: UInt8 = 0
                if let value {
                    var text = value
                    if value.isEmpty { flag |= emptyBit }
                    if let lead = value.first(where: { !$0.isWhitespace }), lead == "{" || lead == "[" { flag |= openableBit }
                    if Int(column) < types.count, GridMetrics.isNumeric(type: types[Int(column)]) { flag |= numericBit }
                    if cut, value.utf16.count > 256 {
                        var units = 0
                        var end = value.startIndex
                        for index in value.indices {
                            let next = value.index(after: index)
                            let width = value[index..<next].utf16.count
                            if units + width > 256 { break }
                            units += width
                            end = next
                        }
                        text = String(value[..<end])
                        flag |= truncatedBit
                    }
                    heap += Array(text.utf8)
                } else {
                    flag |= nullBit
                }
                flags.append(flag)
                offsets.append(UInt32(heap.count))
            }
        }
        var data = Data("QHW1".utf8)
        func put<T: FixedWidthInteger>(_ value: T) { var v = value.littleEndian; data.append(Data(bytes: &v, count: MemoryLayout<T>.size)) }
        put(UInt16(1))
        put(UInt16(cut ? 1 : 0))
        put(UInt32(first))
        put(UInt32(available))
        put(UInt32(columns.count))
        put(UInt32(visible))
        put(UInt32(heap.count))
        put(UInt32(0))
        for r in 0..<available { put(UInt32(first + r)) }
        for offset in offsets { put(offset) }
        data.append(contentsOf: flags)
        data.append(contentsOf: heap)
        return data
    }
}
