import XCTest

@testable import QueryHive

/// Bench 1-3: the wire decode, one streaming paint cycle, and the header sort.
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

    /// The body of `AppModel`'s `rows` case while streaming: append a page to the accumulated
    /// buffer, assign `tab.preview` from it, and read `displayedRows` as the grid does.
    /// The buffer stays shared with `tab.preview` between cycles (it is the closure's `rows` in
    /// the app), so the append pays the same copy-on-write the app pays.
    func testPaintCycle() {
        let page = (0..<200).map { r in (0..<30).map { c -> String? in "r\(r)c\(c)" } }
        let columns = (0..<30).map { Event.Column(name: "c\($0)", type: "text") }
        for total in [10_000, 100_000, 500_000] {
            let tab = QueryTab(title: "bench")
            var rows: [[String?]] = []
            rows.reserveCapacity(total)
            // Inner rows are shared between entries: the bench is about the outer buffer.
            while rows.count < total { rows.append(contentsOf: page) }
            tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                        queryID: nil, elapsedMS: 0)
            let samples = measureMs(iterations: total >= 500_000 ? 20 : 40) {
                rows.append(contentsOf: page)
                tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                            queryID: tab.preview?.queryID, elapsedMS: 0)
                XCTAssertGreaterThan(tab.displayedRows.count, total - 1)
            }
            report("bench-paint-\(total / 1000)k", samples: samples,
                   notes: "append 200 rows + assign tab.preview + read displayedRows at ~\(total) accumulated rows x 30 cols")
        }
    }

    /// `GridSort.order` on 500k rows. Rows are 4 columns wide rather than 30: the sort touches one
    /// column, and 500k x 30 distinct strings would only measure the machine's memory.
    func testSortHeader500k() {
        var rng = Lcg()
        let rows: [[String?]] = (0..<500_000).map { i in
            let n = rng.next()
            return ["\(n % 1_000_000)", "name-\(n % 99_991)-\(i % 13)", n % 50 == 0 ? nil : "\(n % 5000).\(n % 100)", "k"]
        }
        for (column, label) in [(0, "sort-numeric-500k"), (2, "bench-sort-500k-numeric-decimal-null"), (1, "sort-text-500k")] {
            var count = 0
            let samples = measureMs(iterations: 5, warmup: 1) {
                count = GridSort(column: column, direction: .ascending).order(rows).count
            }
            XCTAssertEqual(count, rows.count)
            report(label, axis: "sort-search", metric: "sort_ms", samples: samples,
                   notes: "GridSort.order, 500k rows x 4 cols, column \(column)")
        }
    }
}
