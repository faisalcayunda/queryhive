import Foundation
import XCTest

@testable import QueryHive

/// Base class for the Swift microbenchmarks (performance-plan §4, item 0.3).
///
/// Every bench is skipped unless `QH_BENCH=1`, so a plain `swift test` stays fast. Run them with
/// `QH_BENCH=1 swift test -c release --filter Bench`: a debug build measures the wrong thing.
/// Each bench prints one JSON line in the input contract of `deploy/dev/bench_app.py`, so
/// `swift test ... | python3 deploy/dev/bench_app.py --source app` can record them.
@MainActor
class BenchCase: XCTestCase {
    override func setUpWithError() throws {
        try XCTSkipUnless(ProcessInfo.processInfo.environment["QH_BENCH"] == "1",
                          "set QH_BENCH=1 to run the microbenchmarks")
        isolateConnectionStore()
    }

    /// Run `body` `iterations` times after `warmup` untimed runs; milliseconds per run.
    /// `setup` runs untimed before each timed run.
    func measureMs(iterations: Int, warmup: Int = 2, setup: () -> Void = {},
                   _ body: () -> Void) -> [Double] {
        for _ in 0..<warmup { setup(); body() }
        var samples: [Double] = []
        let clock = ContinuousClock()
        for _ in 0..<iterations {
            setup()
            let took = clock.measure { body() }
            let parts = took.components
            samples.append(Double(parts.seconds) * 1000 + Double(parts.attoseconds) / 1e15)
        }
        return samples
    }

    /// Print one record and return (median, p95).
    @discardableResult
    func report(_ scenario: String, axis: Any? = nil, metric: String = "iter_ms", samples: [Double], notes: String)
        -> (median: Double, p95: Double) {
        let sorted = samples.sorted()
        let median = sorted[sorted.count / 2]
        let p95 = sorted[min(sorted.count - 1, Int((Double(sorted.count) * 0.95).rounded(.up)) - 1)]
        var record: [String: Any] = [
            "source": "swift-bench",
            "scenario": scenario,
            "app": "queryhive",
            "metrics": [metric: samples],
            "median_ms": median,
            "p95_ms": p95,
            "n": samples.count,
            "notes": notes,
        ]
        if let axis { record["axis"] = axis }
        let data = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
        print(String(decoding: data, as: UTF8.self))
        return (median, p95)
    }

    /// A deterministic pseudo-random stream, so two runs sort the same data.
    struct Lcg {
        var state: UInt64 = 0x9E37_79B9_7F4A_7C15
        mutating func next() -> UInt64 {
            state = state &* 6364136223846793005 &+ 1442695040888963407
            return state >> 33
        }
    }
}
