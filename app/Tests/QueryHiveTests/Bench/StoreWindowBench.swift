import Foundation
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// The cost of one page read through UniFFI (blueprint D-22, §21.4): a `window` of 64 x 32 and of
/// 128 x 32 cells, resident on the shared host and spilled on a host of its own, plus `rows_text`
/// and the price of the `Data` the buffer arrives in. The numbers decide `StoreRows.pageRows`:
/// 128 rows if p99 of 128 x 32 is within 0.4 ms including UniFFI, 32 rows if p99 of 64 x 32 is
/// over 0.4 ms.
///
/// Run with `QH_BENCH=1 swift test -c release --filter StoreWindowBench`.
final class StoreWindowBench: BenchCase {
    private let columnCount = 32
    private let rowCount = 100_000

    private func columns() -> [Event.Column] {
        (0..<columnCount).map { Event.Column(name: "c\($0)", type: ["varchar", "bigint", "double", "timestamp(6)"][$0 % 4]) }
    }

    private func rows() -> [[String?]] {
        (0..<rowCount).map { r in
            (0..<columnCount).map { c -> String? in
                switch c % 4 {
                case 0: return "name \(r % 977) \(c)"
                case 1: return c % 7 == 1 && r % 5 == 0 ? nil : String(r &+ c)
                case 2: return String(Double(r) * 0.37 + Double(c))
                default: return "2026-07-25 15:30:06"
                }
            }
        }
    }

    private func percentile(_ values: [Double], _ p: Double) -> Double {
        let sorted = values.sorted()
        return sorted[min(sorted.count - 1, Int((Double(sorted.count) * p).rounded(.up)) - 1)]
    }

    /// Reads windows of `height` rows from random page-aligned positions and reports the distribution.
    private func read(_ store: StoreRows, height: Int, name: String, notes: String) throws {
        var rng = Lcg()
        let all = Array(0..<UInt32(columnCount))
        let formats = [CellFormat](repeating: .raw, count: columnCount)
        let pages = rowCount / height
        let samples = measureMs(iterations: 600, warmup: 50) {
            let first = Int(rng.next() % UInt64(pages)) * height
            let data = try? store.handle.window(viewId: store.viewID, firstRow: UInt32(first), rowCount: UInt32(height),
                                                columns: all, formats: formats)
            precondition(data != nil)
        }
        let (median, p95) = report(name, axis: "window", samples: samples, notes: notes)
        let p99 = percentile(samples, 0.99)
        print("\(name): p50 \(median) ms, p95 \(p95) ms, p99 \(p99) ms")
        XCTAssertFalse(samples.isEmpty)
    }

    func testWindowResident64And128() throws {
        let store = try TestStores.makeStore(columns: columns(), rows: rows())
        defer { store.release() }
        try read(store, height: 64, name: "bench-window-64x32-resident", notes: "window 64 x 32 through UniFFI, resident, shared host")
        try read(store, height: 128, name: "bench-window-128x32-resident", notes: "window 128 x 32 through UniFFI, resident, shared host")
    }

    /// A host of its own with a real spill directory and a budget a tenth of the data, so most pages
    /// are read back from disk (the shared host has spill off, which `TestStores` explains).
    func testWindowSpilled64And128() throws {
        let (host, directory) = try TestStores.spillingHost(budgetBytes: 4 << 20)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        let store = try TestStores.makeStore(on: host, columns: columns(), rows: rows())
        defer { store.release() }
        let stats = try host.storeStats()
        print("spilled \(stats.spilledBytes) bytes, resident \(stats.residentBytes)")
        try read(store, height: 64, name: "bench-window-64x32-spilled", notes: "window 64 x 32 through UniFFI, spilled (own host, 4 MiB budget)")
        try read(store, height: 128, name: "bench-window-128x32-spilled", notes: "window 128 x 32 through UniFFI, spilled (own host, 4 MiB budget)")
    }

    /// `rows_text` of 64 x 32, and what the copy out of the FFI buffer into `Data` and into strings costs.
    func testRowsTextAndDecode() throws {
        let store = try TestStores.makeStore(columns: columns(), rows: rows())
        defer { store.release() }
        let all = Array(0..<UInt32(columnCount))
        var data = Data()
        var rng = Lcg()
        let text = measureMs(iterations: 400, warmup: 40) {
            let first = Int(rng.next() % UInt64(rowCount / 64)) * 64
            data = try! store.handle.rowsText(viewId: store.viewID, firstRow: UInt32(first), rowCount: 64, columns: all)
        }
        report("bench-rows-text-64x32", axis: "window", samples: text, notes: "rows_text 64 x 32 through UniFFI, raw, uncut")
        let decode = measureMs(iterations: 400, warmup: 40) {
            let page = try! WindowPage(data)
            _ = page.values()
        }
        report("bench-window-decode-64x32", axis: "window", samples: decode,
               notes: "WindowPage decode and 2,048 String(decoding:) of one rows_text buffer (\(data.count) bytes)")
    }
}
