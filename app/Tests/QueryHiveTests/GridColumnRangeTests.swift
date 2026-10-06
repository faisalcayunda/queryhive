import AppKit
import XCTest

@testable import QueryHive

/// `ArrayRows` with a counter on `cell`, so a test can say how many reads a draw cost.
private final class CountingRows: ResultRows, @unchecked Sendable {
    let inner: ArrayRows
    private(set) var cellReads = 0
    init(_ inner: ArrayRows) { self.inner = inner }

    var count: Int { inner.count }
    var fetched: Int { inner.fetched }
    var columns: [Event.Column] { inner.columns }
    func row(at index: Int) -> [String?]? { inner.row(at: index) }
    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText {
        cellReads += 1
        return inner.cell(row: row, column: column, format: format)
    }
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String? {
        inner.fullValue(row: row, column: column, format: format)
    }
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]] { inner.rows(in: range, columns: columns) }
    func naturalCharCounts() -> [Int] { inner.naturalCharCounts() }
    func distinctValues(column: Int) async -> DistinctSample { await inner.distinctValues(column: column) }
}

/// D-23: the table builds and paints the columns on screen, not all of them (TM-2).
@MainActor
final class GridColumnRangeTests: XCTestCase {

    private static let width = 500
    private let columns = (0..<GridColumnRangeTests.width).map { Event.Column(name: "c\($0)", type: "text") }
    private lazy var data: [[String?]] = (0..<20).map { row in
        (0..<GridColumnRangeTests.width).map { column in column % 7 == 0 ? nil : "r\(row)c\(column)" }
    }

    private func fixture() -> (GridFixture, CountingRows) {
        let fixture = GridFixture(columns: columns, rows: data)
        fixture.apply()
        let counting = CountingRows(ArrayRows(rows: data, sizing: data, columns: columns))
        fixture.coordinator.rows = counting
        fixture.coordinator.textCache.removeAll()
        return (fixture, counting)
    }

    func testARowReadsOnlyTheColumnsOnScreenAndNotAllFiveHundred() {
        let (fixture, counting) = fixture()
        let text = fixture.coordinator.rowText(0, columns: 0..<6)
        XCTAssertGreaterThanOrEqual(counting.cellReads, 6)
        XCTAssertLessThan(counting.cellReads, 80, "500 reads per row is the bug this test pins")
        XCTAssertEqual(text.first, 0)
        XCTAssertEqual(text.cells.count, counting.cellReads)
        XCTAssertEqual(text.cells[1], "r0c1")
        XCTAssertTrue(text.flags[0].contains(.null))
    }

    func testAScrolledRangeIsIndexedFromItsOwnFirstColumn() {
        let (fixture, counting) = fixture()
        let text = fixture.coordinator.rowText(3, columns: 300..<306)
        XCTAssertLessThan(counting.cellReads, 80)
        XCTAssertTrue(text.built.contains(300) && text.built.contains(305))
        XCTAssertEqual(text.cells[300 - text.first], "r3c300")
        XCTAssertEqual(text.cells[302 - text.first], "r3c302")
    }

    func testABuiltRangeIsReusedAndOnlyAMissRebuilds() {
        let (fixture, counting) = fixture()
        _ = fixture.coordinator.rowText(0, columns: 0..<6)
        let after = counting.cellReads
        _ = fixture.coordinator.rowText(0, columns: 2..<5)
        XCTAssertEqual(counting.cellReads, after, "a request inside the built range is a hit")
        _ = fixture.coordinator.rowText(0, columns: 400..<406)
        XCTAssertGreaterThan(counting.cellReads, after, "a request outside it builds the row again")
        let text = fixture.coordinator.rowText(0, columns: 400..<406)
        XCTAssertEqual(text.cells[402 - text.first], "r0c402")
    }

    /// The painter fed a ranged text and the same columns of a full text lays down the same pixels.
    func testPaintingARangeMatchesPaintingTheSameColumnsFromAFullText() throws {
        let (fixture, _) = fixture()
        let coordinator = fixture.coordinator
        let range = 40..<46
        let ranged = coordinator.rowText(0, columns: range)
        var full = GridRowText(first: 0, cells: [], flags: [])
        for source in 0..<GridColumnRangeTests.width {
            let cell = ArrayRows(rows: data, sizing: data, columns: columns)
                .cell(row: 0, column: source, format: .raw)
            full.cells.append(cell.text)
            full.flags.append(cell.flags)
        }

        func render(_ text: GridRowText) throws -> [UInt8] {
            let context = coordinator.paint
            let pixelsWide = Int(ceil(context.geometry.totalWidth))
            let pixelsHigh = Int(ceil(context.rowHeight))
            let bitmap = try XCTUnwrap(CGContext(data: nil, width: pixelsWide, height: pixelsHigh,
                                                 bitsPerComponent: 8, bytesPerRow: 0,
                                                 space: CGColorSpaceCreateDeviceRGB(),
                                                 bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
            bitmap.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
            GridRowPainter.paint(row: 0, columns: range, context: context, text: text, staged: [],
                                 selection: nil, lines: coordinator.lineCache,
                                 width: CGFloat(pixelsWide), into: bitmap)
            let bytes = try XCTUnwrap(bitmap.data)
            return Array(UnsafeBufferPointer(start: bytes.assumingMemoryBound(to: UInt8.self),
                                             count: bitmap.bytesPerRow * pixelsHigh))
        }
        let a = try render(ranged)
        let b = try render(full)
        XCTAssertEqual(a, b)
        XCTAssertTrue(a.contains { $0 != 0 }, "the render has to put something on the bitmap")
    }

    func testRowsDidGrowRepaintsOnlyTheNewRows() {
        let (fixture, counting) = fixture()
        _ = fixture.coordinator.rowText(0, columns: 0..<6)
        let reads = counting.cellReads
        fixture.coordinator.rowsDidGrow(from: 10, to: 12)
        _ = fixture.coordinator.rowText(0, columns: 0..<6)
        XCTAssertEqual(counting.cellReads, reads, "the drawn rows keep their cached text (D-28)")
    }
}
