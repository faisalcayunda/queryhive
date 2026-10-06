import Foundation
import os
import QueryHiveFFI

/// What a store call that did not work came to, with the operation it was part of.
///
/// `ResultRows` does not throw (TM-1): a failed read is a blank cell, and the reason lands in
/// `StoreRows.lastFailure` for the grid's banner. The copy and write-plan paths use the throwing
/// variants instead and refuse a partial answer.
struct StoreFailure: Error, CustomStringConvertible {
    let operation: String
    let underlying: Error?
    let detail: String

    init(operation: String, underlying: Error) {
        self.operation = operation
        self.underlying = underlying
        self.detail = (underlying as? LocalizedError)?.errorDescription ?? "\(underlying)"
    }

    init(operation: String, detail: String) {
        self.operation = operation
        self.underlying = nil
        self.detail = detail
    }

    /// The handle was released or the view moved on: not a problem to show, only to stop on.
    var isStale: Bool {
        guard let error = underlying as? StoreFfiError else { return false }
        switch error {
        case .StaleHandle, .StaleView, .Superseded: return true
        default: return false
        }
    }

    var description: String { "\(operation): \(detail)" }
}

/// One decoded `QHW1` buffer (blueprint §11.1). Holds the `Data` as it came and decodes a cell only
/// when it is asked for, so a page costs one copy out of the FFI and nothing per cell until drawn.
struct WindowPage {
    let data: Data
    let firstRow: Int
    let rowCount: Int
    let columnCount: Int
    private let offsets: Int      // S0: `u32[RC * C + 1]`
    private let flags: Int        // S1: `u8[RC * C]`
    private let heap: Int         // S2: the UTF-8 heap

    /// The bytes a decoded page keeps, for the cache's byte budget.
    var byteCount: Int { data.count }

    init(_ data: Data) throws {
        func fail(_ why: String) -> StoreFailure { StoreFailure(operation: "window", detail: "bad buffer: \(why)") }
        guard data.count >= 32 else { throw fail("shorter than the header") }
        let header: (magic: [UInt8], version: UInt16, rows: Int, columns: Int, first: Int, heapLength: Int) =
            data.withUnsafeBytes { raw in
                (Array(raw[0..<4]),
                 raw.loadUnaligned(fromByteOffset: 4, as: UInt16.self),
                 Int(raw.loadUnaligned(fromByteOffset: 12, as: UInt32.self)),
                 Int(raw.loadUnaligned(fromByteOffset: 16, as: UInt32.self)),
                 Int(raw.loadUnaligned(fromByteOffset: 8, as: UInt32.self)),
                 Int(raw.loadUnaligned(fromByteOffset: 24, as: UInt32.self)))
            }
        guard header.magic == Array("QHW1".utf8) else { throw fail("magic") }
        guard header.version == 1 else { throw fail("version \(header.version)") }
        let cells = header.rows.multipliedReportingOverflow(by: header.columns)
        guard !cells.overflow, cells.partialValue <= 1 << 24 else { throw fail("implausible size") }
        let s0 = 32 + 4 * header.rows
        let s1 = s0 + 4 * (cells.partialValue + 1)
        let s2 = s1 + cells.partialValue
        guard data.count == s2 + header.heapLength else { throw fail("length is not S2 + H") }
        let monotone: Bool = data.withUnsafeBytes { raw in
            var previous = 0
            for k in 0...cells.partialValue {
                let value = Int(raw.loadUnaligned(fromByteOffset: s0 + 4 * k, as: UInt32.self))
                if value < previous || value > header.heapLength { return false }
                previous = value
            }
            return true
        }
        guard monotone else { throw fail("offsets are not monotone inside the heap") }
        self.data = data
        self.firstRow = header.first
        self.rowCount = header.rows
        self.columnCount = header.columns
        self.offsets = s0
        self.flags = s1
        self.heap = s2
    }

    /// The text and the raw QHW1 flag byte of cell (`row`, `column`), both relative to the window.
    func cell(row: Int, column: Int) -> (text: String, flags: UInt8) {
        let k = row * columnCount + column
        return data.withUnsafeBytes { raw in
            let start = Int(raw.loadUnaligned(fromByteOffset: offsets + 4 * k, as: UInt32.self))
            let end = Int(raw.loadUnaligned(fromByteOffset: offsets + 4 * (k + 1), as: UInt32.self))
            let text = String(decoding: UnsafeRawBufferPointer(rebasing: raw[(heap + start)..<(heap + end)]),
                              as: UTF8.self)
            return (text, raw[flags + k])
        }
    }

    /// QHW1 flags are not `CellFlags`: the Rust side numbers OPENABLE 4 and TRUNCATED 16, the Swift
    /// side the other way round (TM-7). Mapped bit by bit so a swap is a failing test, not a drawing.
    static func cellFlags(_ raw: UInt8) -> CellFlags {
        var result = CellFlags()
        if raw & 1 != 0 { result.insert(.null) }
        if raw & 2 != 0 { result.insert(.empty) }
        if raw & 4 != 0 { result.insert(.openable) }
        if raw & 8 != 0 { result.insert(.numeric) }
        if raw & 16 != 0 { result.insert(.truncated) }
        return result
    }

    /// The cells of a `rows_text` answer (`CUT` clear): the raw value, `nil` for NULL.
    func values() -> [[String?]] {
        (0..<rowCount).map { r in
            (0..<columnCount).map { c in
                let cell = cell(row: r, column: c)
                return cell.flags & 1 != 0 ? nil : cell.text
            }
        }
    }
}

extension CellText {
    /// A window cell as the grid shows it: its first line, cut at `prefixLimit` characters.
    static func make(display: String, flags: CellFlags) -> CellText {
        let firstLine = display.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
            .first.map(String.init) ?? display
        var flags = flags
        guard firstLine.count > prefixLimit else { return CellText(text: firstLine, flags: flags) }
        flags.insert(.truncated)
        return CellText(text: String(firstLine.prefix(prefixLimit)), flags: flags)
    }

    /// A raw value shown under a Swift-rendered format: what `ArrayRows` always did.
    static func make(raw value: String?, format: ColumnFormat, type: String) -> CellText {
        guard let value else { return CellText(text: "", flags: [.null]) }
        var flags = CellFlags()
        if value.isEmpty { flags.insert(.empty) }
        if GridMetrics.isNumeric(type: type) { flags.insert(.numeric) }
        if let first = value.first(where: { !$0.isWhitespace }), first == "{" || first == "[" {
            flags.insert(.openable)
        }
        return make(display: format.render(value, type: type), flags: flags)
    }
}

/// The rows of a result that live in the Rust store, read a window at a time (blueprint §17.2).
///
/// A result is a `ResultHandle`; this is the Swift side of it, and the only thing the grid, the
/// copy path and the write-plan builder see. Reads go through pages of `pageRows` x `columnBlock`
/// **source** columns, one `window` call per miss, cached by `(viewID, page, block)`. Two things a
/// page must not do: outlive the rows it was read from (each page records `first` and `rows`, so a
/// page read while the result was still streaming short is read again once rows arrive, B2) and
/// outlive the format it was rendered under (`prepare(formats:)` drops the blocks whose format
/// changed, B2). `viewID` and `count` change in one place, `install`, which the `apply` hop is the
/// only caller of for a view change (B3).
final class StoreRows: ResultRows, @unchecked Sendable {
    /// Provisional, D-22: `StoreWindowBench` confirms or replaces them.
    static let pageRows = 64
    static let columnBlock = 32
    static let maxPages = 24
    static let minPages = 8
    static let maxPageBytes = 8 << 20
    static let maxRowBlocks = 8
    /// Formats the app renders itself from `rows_text` (D-24). `Raw` never joins them.
    static let swiftRenderedFormats: Set<ColumnFormat> = [.json]

    private static let log = Logger(subsystem: Bundle.main.bundleIdentifier ?? "QueryHive", category: "store")
    private static let workQueue = DispatchQueue(label: "queryhive.store.work", qos: .userInitiated,
                                                 attributes: .concurrent)
    /// `set_view` calls of this store, one at a time: Rust installs whichever call finishes last, so
    /// applies must reach it in the order they were asked for.
    private let viewQueue = DispatchQueue(label: "queryhive.store.view", qos: .userInitiated)

    let handle: any ResultHandleProtocol

    /// Recursive: `cell` holds it across a page load, which records its failures through it.
    private let lock = NSRecursiveLock()
    private var _columns: [Event.Column]
    private var _count = 0
    private var _fetched = 0
    private var _phase = StorePhase.empty
    private var _viewID: UInt64 = 0
    private var applying = 0
    private var released = false
    private var _lastFailure: StoreFailure?
    private var formats: [ColumnFormat] = []
    private var widths: [Int]?
    private var signposted = false

    private struct PageKey: Hashable { var view: UInt64; var page: Int; var block: Int }
    private final class Page {
        let window: WindowPage
        var rendered: [Int: [CellText]]
        var tick = 0
        init(window: WindowPage, rendered: [Int: [CellText]]) { self.window = window; self.rendered = rendered }
        var bytes: Int { window.byteCount + rendered.values.reduce(0) { $0 + $1.count * 64 } }
    }
    private var pages: [PageKey: Page] = [:]
    private var pageTick = 0

    private struct RowBlock { var first: Int; var rows: [[String?]]; var tick: Int }
    private var rowBlocks: [PageKey: RowBlock] = [:]

    /// A store seeded from the handle's own counters, so a store built from rows (`store_from_rows`)
    /// is readable before the first poll.
    init(handle: any ResultHandleProtocol, columns: [Event.Column] = []) {
        self.handle = handle
        self._columns = columns
        if let counts = try? handle.rowCount() {
            _viewID = counts.viewId
            _count = Int(counts.visible)
            _fetched = Int(counts.fetched)
            _phase = counts.phase
        }
    }

    deinit { release() }

    // MARK: State

    var columns: [Event.Column] { locked { _columns } }
    /// Zero once released: a table that still holds this store (the tab's next result has not been
    /// applied to it yet) has no row left to ask for, so it draws none of them.
    var count: Int { locked { released ? 0 : _count } }
    var fetched: Int { locked { _fetched } }
    var phase: StorePhase { locked { _phase } }
    var viewID: UInt64 { locked { _viewID } }
    var lastFailure: StoreFailure? { locked { _lastFailure } }

    var isLive: Bool {
        locked { !released && (!Self.isTerminal(_phase) || applying > 0) }
    }

    private static func isTerminal(_ phase: StorePhase) -> Bool {
        switch phase {
        case .empty, .streaming: false
        case .complete, .cancelled, .failed: true
        }
    }

    @discardableResult
    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    /// Main thread, from the `columns` event.
    func setColumns(_ columns: [Event.Column]) {
        locked { _columns = columns; widths = nil }
    }

    // MARK: Polling

    /// Once per display tick while `isLive`. `fetched` and `phase` always follow the store. `count`
    /// follows it only while the store's view is the one being drawn: since `row_count()` reads one
    /// snapshot, a view that `set_view` has just installed answers here before the apply hop has
    /// told this object, and its count over the old `viewID` would be drawn blank (B3).
    func poll() -> PollResult {
        let counts: RowCount
        do { counts = try handle.rowCount() } catch {
            record("row_count", error)
            return PollResult(grewFrom: nil, finished: true)
        }
        return locked {
            _fetched = Int(counts.fetched)
            _phase = counts.phase
            if !signposted, _fetched > 0 {
                signposted = true
                PerfSignposts.firstRowsEvent()
            }
            var grew: Int?
            if counts.viewId == _viewID {
                let visible = Int(counts.visible)
                if visible > _count {
                    grew = _count
                    _count = visible
                } else if visible < _count {
                    Self.log.error("a view shrank under its own id: \(self._count) -> \(visible)")
                }
            }
            return PollResult(grewFrom: grew, finished: Self.isTerminal(_phase) && applying == 0)
        }
    }

    // MARK: Views

    /// Install a view: computed off the main thread, then installed on it, which makes this the one
    /// writer of `viewID` and `count` for a view change. Applies reach Rust in call order (a serial
    /// queue), and `install` ignores a view older than the one already shown, so the latest ask wins
    /// even if the hops come back out of order. Rust itself never answers `Superseded` here.
    func apply(_ spec: ViewSpec) async throws -> ViewInfo {
        locked { applying += 1 }
        let info: ViewInfo
        do {
            info = try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<ViewInfo, Error>) in
                self.viewQueue.async { continuation.resume(with: Result { try self.handle.setView(spec: spec) }) }
            }
        } catch {
            locked { applying -= 1 }
            record("set_view", error)
            throw error
        }
        await MainActor.run { install(info) }
        return info
    }

    /// The synchronous twin, for tests.
    @discardableResult
    func applyBlocking(_ spec: ViewSpec) throws -> ViewInfo {
        locked { applying += 1 }
        do {
            let info = try handle.setView(spec: spec)
            install(info)
            return info
        } catch {
            locked { applying -= 1 }
            record("set_view", error)
            throw error
        }
    }

    private func install(_ info: ViewInfo) {
        locked {
            applying -= 1
            // An older view's hop that arrived late: the store has moved on, and so has `_viewID`.
            guard info.viewId >= _viewID else { return }
            _viewID = info.viewId
            _count = Int(info.visible)
            _fetched = Int(info.fetched)
            pages.removeAll()
            rowBlocks.removeAll()
        }
    }

    // MARK: Formats

    private func format(of column: Int) -> ColumnFormat { formats.indices.contains(column) ? formats[column] : .raw }

    private static func ffi(_ format: ColumnFormat) -> CellFormat {
        switch format {
        case .raw: .raw
        case .text: .text
        case .uuid: .uuid
        case .unixTimestamp: .unixTimestamp
        case .json: .json
        }
    }

    /// The format of every column, in source order. Drops every page of a block one of whose formats
    /// changed (the format is not in the page key) and answers the source columns that changed.
    @discardableResult
    func prepare(formats incoming: [ColumnFormat]) -> IndexSet {
        locked {
            var changed = IndexSet()
            for column in 0..<max(incoming.count, formats.count) {
                let old = formats.indices.contains(column) ? formats[column] : .raw
                let new = incoming.indices.contains(column) ? incoming[column] : .raw
                if old != new { changed.insert(column) }
            }
            formats = incoming
            dropBlocks(of: changed)
            return changed
        }
    }

    private func dropBlocks(of columns: IndexSet) {
        guard !columns.isEmpty else { return }
        let blocks = Set(columns.map { $0 / Self.columnBlock })
        pages = pages.filter { !blocks.contains($0.key.block) }
    }

    // MARK: Cells

    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText {
        lock.lock()
        defer { lock.unlock() }
        guard !released, row >= 0, row < _count, column >= 0, column < _columns.count else {
            return CellText(text: "", flags: [])
        }
        // The grid hands over the format it is drawing. `prepare(formats:)` is how the pages learn
        // it, so a caller that skipped it is brought in line here rather than shown stale text.
        if self.format(of: column) != format {
            if formats.count < _columns.count { formats += Array(repeating: .raw, count: _columns.count - formats.count) }
            formats[column] = format
            dropBlocks(of: IndexSet(integer: column))
        }
        let block = column / Self.columnBlock
        let pageIndex = row / Self.pageRows
        let key = PageKey(view: _viewID, page: pageIndex, block: block)
        var page = pages[key]
        if let hit = page, row - hit.window.firstRow >= hit.window.rowCount { page = nil }
        if page == nil {
            guard let loaded = loadPage(key) else { return CellText(text: "", flags: []) }
            page = loaded
            // Stamped before the eviction: a page with no tick would be the oldest one there is.
            pageTick += 1
            loaded.tick = pageTick
            pages[key] = loaded
            evictPages()
        }
        guard let page, row - page.window.firstRow < page.window.rowCount else {
            Self.log.error("row \(row) is under the count but outside its page: a bug")
            return CellText(text: "", flags: [])
        }
        pageTick += 1
        page.tick = pageTick
        let r = row - page.window.firstRow
        if let rendered = page.rendered[column] { return rendered[r] }
        let raw = page.window.cell(row: r, column: column - block * Self.columnBlock)
        return CellText.make(display: raw.text, flags: WindowPage.cellFlags(raw.flags))
    }

    /// Reads one page. Called with the lock held; the FFI call is a `window` of at most 2,048 cells.
    private func loadPage(_ key: PageKey) -> Page? {
        let sources = Array((key.block * Self.columnBlock)..<min((key.block + 1) * Self.columnBlock, _columns.count))
        let first = key.page * Self.pageRows
        let swiftColumns = sources.filter { Self.swiftRenderedFormats.contains(format(of: $0)) }
        do {
            let data = try handle.window(viewId: key.view, firstRow: UInt32(first), rowCount: UInt32(Self.pageRows),
                                         columns: sources.map(UInt32.init),
                                         formats: sources.map { Self.swiftRenderedFormats.contains(format(of: $0)) ? .raw : Self.ffi(format(of: $0)) })
            let window = try WindowPage(data)
            var rendered: [Int: [CellText]] = [:]
            if !swiftColumns.isEmpty, window.rowCount > 0 {
                // Swift-rendered columns need the whole value, which a window cuts at 256 units.
                let full = try handle.rowsText(viewId: key.view, firstRow: UInt32(window.firstRow),
                                               rowCount: UInt32(window.rowCount), columns: swiftColumns.map(UInt32.init))
                let values = try WindowPage(full).values()
                for (offset, column) in swiftColumns.enumerated() {
                    let type = _columns[column].type
                    rendered[column] = values.map { CellText.make(raw: $0[offset], format: format(of: column), type: type) }
                }
            }
            return Page(window: window, rendered: rendered)
        } catch {
            record("window", error)
            return nil
        }
    }

    private func evictPages() {
        var bytes = pages.values.reduce(0) { $0 + $1.bytes }
        while pages.count > Self.minPages, pages.count > Self.maxPages || bytes > Self.maxPageBytes {
            guard let oldest = pages.min(by: { $0.value.tick < $1.value.tick }) else { break }
            bytes -= oldest.value.bytes
            pages.removeValue(forKey: oldest.key)
        }
    }

    /// Tab to the background: give back the pages and the row blocks, keep the store.
    func dropPages() {
        locked { pages.removeAll(); rowBlocks.removeAll() }
    }

    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String? {
        let (view, live) = locked { (_viewID, !released && row >= 0 && row < _count && column >= 0 && column < _columns.count) }
        guard live else { return nil }
        do {
            if Self.swiftRenderedFormats.contains(format) {
                let raw = try handle.cellText(viewId: view, row: UInt32(row), column: UInt32(column), format: .raw)
                return raw.map { format.render($0, type: columnType(column)) }
            }
            return try handle.cellText(viewId: view, row: UInt32(row), column: UInt32(column), format: Self.ffi(format))
        } catch {
            record("cell_text", error)
            return nil
        }
    }

    private func columnType(_ column: Int) -> String {
        locked { _columns.indices.contains(column) ? _columns[column].type : "" }
    }

    // MARK: Whole rows

    func row(at index: Int) -> [String?]? {
        let (columnCount, view, inRange) = locked { (_columns.count, _viewID, index >= 0 && index < _count && !released) }
        guard inRange, columnCount > 0 else { return nil }
        let size = max(1, 4096 / columnCount)
        let key = PageKey(view: view, page: index / size, block: 0)
        if let hit = locked({ rowBlocks[key] }), index - hit.first < hit.rows.count {
            return hit.rows[index - hit.first]
        }
        do {
            let rows = try fetchText(first: key.page * size, rows: size, columns: Array(0..<columnCount), view: view)
            return locked {
                guard !rows.isEmpty else { return nil }
                pageTick += 1
                rowBlocks[key] = RowBlock(first: key.page * size, rows: rows, tick: pageTick)
                while rowBlocks.count > Self.maxRowBlocks,
                      let oldest = rowBlocks.min(by: { $0.value.tick < $1.value.tick }) {
                    rowBlocks.removeValue(forKey: oldest.key)
                }
                return index - key.page * size < rows.count ? rows[index - key.page * size] : nil
            }
        } catch {
            record("rows_text", error)
            return nil
        }
    }

    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]] {
        do { return try rowsOrThrow(in: range, columns: columns) } catch {
            record("rows_text", error)
            return []
        }
    }

    /// Raw values for copy and write plans, split to the FFI's limits. A failure is thrown, never a
    /// blank cell that could reach a clipboard or a `WHERE`.
    func rowsOrThrow(in range: Range<Int>, columns: [Int]) throws -> [[String?]] {
        let (view, total, released) = locked { (_viewID, _count, self.released) }
        if released { throw StoreFailure(operation: "rows_text", underlying: StoreFfiError.StaleHandle) }
        let clamped = range.clamped(to: 0..<total)
        guard !clamped.isEmpty, !columns.isEmpty else { return [] }
        do {
            let rows = try fetchText(first: clamped.lowerBound, rows: clamped.count, columns: columns, view: view)
            guard rows.count == clamped.count else {
                throw StoreFailure(operation: "rows_text", detail: "read \(rows.count) of \(clamped.count) rows")
            }
            return rows
        } catch let failure as StoreFailure {
            throw failure
        } catch {
            throw StoreFailure(operation: "rows_text", underlying: error)
        }
    }

    /// `rows_text` over any shape: columns in chunks of 1,024, rows in chunks of 4,096 and 262,144
    /// cells, and a `TooLarge` answer halved until one row.
    private func fetchText(first: Int, rows: Int, columns: [Int], view: UInt64) throws -> [[String?]] {
        let columnChunk = 1_024
        var result = [[String?]](repeating: [], count: rows)
        // Rust clamps a window to the rows that have arrived: only the rows every chunk answered are
        // returned, so a block read mid-stream is short and is read again once the count grows (B2).
        var answered = rows
        for start in stride(from: 0, to: columns.count, by: columnChunk) {
            let part = Array(columns[start..<min(start + columnChunk, columns.count)])
            let step = max(1, min(4_096, 262_144 / part.count))
            var done = 0
            var size = step
            while done < rows {
                let take = min(size, rows - done)
                do {
                    let data = try handle.rowsText(viewId: view, firstRow: UInt32(first + done), rowCount: UInt32(take),
                                                   columns: part.map(UInt32.init))
                    let values = try WindowPage(data).values()
                    for (offset, row) in values.enumerated() { result[done + offset].append(contentsOf: row) }
                    done += values.count
                    if values.isEmpty { break }
                    size = step
                } catch StoreFfiError.TooLarge where take > 1 {
                    size = max(1, take / 2)
                }
            }
            answered = min(answered, done)
        }
        return Array(result.prefix(answered))
    }

    // MARK: Widths and distinct values

    /// Per source column, `min(widest, 64)` of the first 200 rows. Rust freezes its statistics at
    /// row 200, so the answer is asked for again only while fewer than 200 rows are known and the
    /// run is still going (TM-8: NULL counts 0 there, which no width reaches).
    func naturalCharCounts() -> [Int] {
        if let cached = locked({ widths }) { return cached }
        do {
            let raw = try handle.columnWidths()
            let (columnCount, settled) = locked { (_columns.count, _fetched >= 200 || Self.isTerminal(_phase)) }
            var counts = raw.prefix(columnCount).map { min(Int($0), 64) }
            if counts.count < columnCount { counts += Array(repeating: 0, count: columnCount - counts.count) }
            if settled { locked { widths = counts } }
            return counts
        } catch {
            record("column_widths", error)
            return Array(repeating: 0, count: columns.count)
        }
    }

    func distinctValues(column: Int) async -> DistinctSample {
        let limit = UInt32(ColumnFilter.valuePickerLimit + 1)
        return await withCheckedContinuation { (continuation: CheckedContinuation<DistinctSample, Never>) in
            Self.workQueue.async {
                do {
                    let answer = try self.handle.distinctValues(column: UInt32(column), limit: limit)
                    continuation.resume(returning: DistinctSample(values: answer.values, more: answer.more))
                } catch {
                    self.record("distinct_values", error)
                    // No list: the filter falls back to the text box, which always works.
                    continuation.resume(returning: DistinctSample(values: [], more: true))
                }
            }
        }
    }

    // MARK: Release

    /// Give the rows back to the engine. Idempotent, from any thread; `deinit` calls it too.
    func release() {
        let first = locked { () -> Bool in
            guard !released else { return false }
            released = true
            pages.removeAll()
            rowBlocks.removeAll()
            return true
        }
        guard first else { return }
        do { try handle.release() } catch { Self.log.error("release: \(String(describing: error))") }
    }

    var isReleased: Bool { locked { released } }

    private func record(_ operation: String, _ error: Error) {
        let failure = (error as? StoreFailure) ?? StoreFailure(operation: operation, underlying: error)
        guard !failure.isStale else { return }
        Self.log.error("\(failure.description)")
        locked { if _lastFailure == nil { _lastFailure = failure } }
    }
}

/// "No result yet": what a tab shows before any Run, and after one that wrote nothing.
final class EmptyRows: ResultRows, @unchecked Sendable {
    var count: Int { 0 }
    var fetched: Int { 0 }
    var columns: [Event.Column] { [] }
    func row(at index: Int) -> [String?]? { nil }
    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText { CellText(text: "", flags: []) }
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String? { nil }
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]] { [] }
    func naturalCharCounts() -> [Int] { [] }
    func distinctValues(column: Int) async -> DistinctSample { DistinctSample(values: [], more: false) }
}

extension QueryTab {
    /// The view this tab's filters, search and in-memory sort ask the store for (§17.2).
    var viewSpec: ViewSpec {
        var filters: [FilterSpec] = []
        for (column, filter) in columnFilters.sorted(by: { $0.key < $1.key }) {
            switch filter {
            case .values(let picked):
                filters.append(.values(column: UInt32(column),
                                       values: picked.sorted().map { $0 == ColumnFilter.nullToken ? nil : $0 }))
            case .text(let needle):
                guard !filter.isEmpty else { continue }
                filters.append(.text(column: UInt32(column), needle: needle))
            }
        }
        let sort = memorySort.map { SortSpec(column: UInt32($0.column), descending: $0.direction == .descending) }
        return ViewSpec(sort: sort, filters: filters, search: effectiveLocalSearch)
    }
}
