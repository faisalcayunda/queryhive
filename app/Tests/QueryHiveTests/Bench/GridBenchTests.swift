import XCTest

@testable import QueryHive

/// Bench 1: the wire decode of a `rows` line (the CLI, MCP and the `host.run` path still read them).
final class GridBenchTests: BenchCase {
    /// One `rows` event of 200 x 30, decoded the way `RustEngine` does: a fresh decoder per line.
    func testDecodeRowsEvent200x30() {
        let page = (0..<200).map { r in
            (0..<30).map { c -> String? in
                c % 7 == 3 ? nil : "row\(r)-col\(c)-\(String(repeating: "x", count: c % 12))"
            }
        }
        let line = String(decoding: try! JSONSerialization.data(
            withJSONObject: ["event": "rows", "data": page.map { row in row.map { $0 as Any? ?? NSNull() } }]),
            as: UTF8.self)
        var rowsSeen = 0
        let samples = measureMs(iterations: 300, warmup: 20) {
            let event = EngineWire.event(in: Data(line.utf8))
            rowsSeen = event?.data?.count ?? 0
        }
        XCTAssertEqual(rowsSeen, 200)
        report("bench-decode-rows-200x30", samples: samples,
               notes: "EngineWire.event on one 200x30 rows line (\(line.utf8.count) bytes), fresh decoder per line")
    }

    // The streaming paint cycle and the 500k-row header sort that lived here measured the Swift row
    // accumulation and `GridSort.order`, both gone from the app. The window read through UniFFI is
    // `StoreWindowBench`; the sort is the Rust view's, measured on the Rust side (`bench_ffi`).
}
