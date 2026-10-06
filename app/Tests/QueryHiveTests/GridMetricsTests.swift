import CoreGraphics
import XCTest

@testable import QueryHive

/// The grid's numbers: the width formula, the slack, the boxes the baselines measure, the geometry
/// the header and the body share, and the diff that decides what repaints.
///
/// Pure values, so every case here runs without a window (blueprint §18).
final class GridMetricsTests: XCTestCase {

    // MARK: The width formula

    /// `min(max(n × 7.2 + 20, 84), 320) + 22`, which is the width the grid has always given a
    /// column: 7.2 pt per character at 12 pt monospaced, a two-character floor, a 320 pt ceiling,
    /// and the trailing 22 for the cell's own padding plus the header's funnel.
    func testTheWidthFormulaIsSevenPointTwoNPlusTwentyClampedAndPlusTwentyTwo() throws {
        func width(header: Int, sample: Int) throws -> CGFloat {
            let widths = try XCTUnwrap(GridMetrics.naturalWidths(headerCounts: [header],
                                                                 sampleCounts: [sample]))
            return try XCTUnwrap(widths.first)
        }
        // Under the floor: two characters is 84 + 22, not 34.4 + 22.
        XCTAssertEqual(try width(header: 2, sample: 0), 106, accuracy: 0.001)
        // Between the clamps: 10 × 7.2 + 20 = 92, plus 22.
        XCTAssertEqual(try width(header: 10, sample: 0), 114, accuracy: 0.001)
        // Above the ceiling: 300 characters does not eat the panel.
        XCTAssertEqual(try width(header: 0, sample: 300), 342, accuracy: 0.001)
        // The wider of the header label and the sampled rows wins.
        XCTAssertEqual(try width(header: 40, sample: 3), 330, accuracy: 0.001)
        XCTAssertEqual(try width(header: 3, sample: 40), 330, accuracy: 0.001)
    }

    /// Slack goes out in proportion to what each column already has, and the gutter is subtracted
    /// first: it is not one of these columns, so its width would otherwise always be the amount
    /// they fell short by.
    func testSlackIsSharedInProportionAndTheGutterIsSubtractedFirst() {
        let fitted = GridMetrics.fitted([100, 200], available: 500, gutter: 40)
        XCTAssertEqual(fitted.reduce(0, +), 460, accuracy: 0.001)
        XCTAssertEqual(fitted[0] / fitted[1], 100.0 / 200.0, accuracy: 0.0001)
        // The 160 points of slack go out 100:200, so the column that started wider grows twice as
        // much — proportional sharing keeps the ratio and with it the reading order.
        XCTAssertEqual(fitted[1] - 200, 2 * (fitted[0] - 100), accuracy: 0.0001)

        // Same panel, a wider gutter: the columns get 240 of the 300, not 300.
        let withGutter = GridMetrics.fitted([100, 100], available: 300, gutter: 60)
        XCTAssertEqual(withGutter.reduce(0, +), 240, accuracy: 0.001)
    }

    /// A result that already overflows is returned untouched: there is no slack, and rounding
    /// widths that are about to scroll anyway would only move the separators.
    func testAResultThatAlreadyFillsThePanelIsNotTouched() {
        XCTAssertEqual(GridMetrics.fitted([200, 200], available: 300, gutter: 0), [200, 200])
        XCTAssertEqual(GridMetrics.fitted([], available: 300, gutter: 60), [])
    }

    // MARK: The boxes the baselines measured

    /// 23 for a data cell, 25 for a gutter cell, 50 for the header band — built from SwiftUI's own
    /// line measurement rather than written as constants (risk R-10 says this test goes first).
    func testTheBoxesAreTheSizeTheBaselinesMeasured() {
        XCTAssertEqual(GridMetrics.dataBoxHeight(), 23, accuracy: 0.001)
        XCTAssertEqual(GridMetrics.gutterBoxHeight(), 25, accuracy: 0.001)
        XCTAssertEqual(GridMetrics.headerHeight(), 50, accuracy: 0.001)
    }

    /// The boxes follow the cell font. At the standard size they are the measured ones, and a larger
    /// font never makes a box smaller; the row grows by two points per font point, so no preset
    /// ever overflows its row by more than Compact does at the standard size (23 in a 21 row), which
    /// is the overflow the painter's row order was built to hide.
    func testTheBoxesFollowTheFontAndTheRowsStayAheadOfThem() {
        XCTAssertEqual(GridMetrics.dataBoxHeight(fontSize: 12), 23, accuracy: 0.001)
        XCTAssertEqual(GridMetrics.headerHeight(fontSize: 12), 50, accuracy: 0.001)
        var previousBox: CGFloat = 0
        var previousHeader: CGFloat = 0
        for size in DataPreferences.gridFontSizeRange {
            let box = GridMetrics.dataBoxHeight(fontSize: size)
            let header = GridMetrics.headerHeight(fontSize: size)
            XCTAssertGreaterThanOrEqual(box, previousBox, "box at \(size)")
            XCTAssertGreaterThanOrEqual(header, previousHeader, "header at \(size)")
            previousBox = box
            previousHeader = header
            for preset in DataPreferences.RowHeight.allCases {
                XCTAssertLessThanOrEqual(box - preset.points(atFontSize: size), 2,
                                         "\(preset) at \(size): box \(box) in a row of \(preset.points(atFontSize: size))")
            }
        }
        XCTAssertGreaterThan(GridMetrics.dataBoxHeight(fontSize: 16), GridMetrics.dataBoxHeight(fontSize: 12))
        XCTAssertGreaterThan(GridMetrics.headerHeight(fontSize: 16), GridMetrics.headerHeight(fontSize: 12))
    }

    /// The painter draws the cells and the header labels in the style's size and leaves the gutter's
    /// number and the type chip at their own; the size is part of the style, so a change of it is a
    /// change of the inputs and the grid re-measures.
    func testThePaintContextDrawsTheCellsAndLabelsInTheStyleSize() {
        var style = GridInputs.GridStyle.placeholder
        let standard = GridPaintContext.resolve(style: style, appearance: .currentDrawing())
        XCTAssertEqual(standard.cellFont.pointSize, 12)
        XCTAssertEqual(standard.headerFont.pointSize, 12)

        style.fontSize = 15
        let larger = GridPaintContext.resolve(style: style, appearance: .currentDrawing())
        XCTAssertEqual(larger.cellFont.pointSize, 15)
        XCTAssertEqual(larger.headerFont.pointSize, 15)
        XCTAssertEqual(larger.gutterFont.pointSize, 10.5)
        XCTAssertEqual(larger.chipFont.pointSize, 11)
        XCTAssertGreaterThan(larger.dataBoxHeight, standard.dataBoxHeight)
        XCTAssertGreaterThan(larger.headerLabelLine, standard.headerLabelLine)
        XCTAssertNotEqual(style, GridInputs.GridStyle.placeholder)
    }

    /// A column's width scales with the cell font, or a larger font would cut every value short with
    /// an ellipsis; at the standard size it is the same number as before.
    func testAColumnIsWiderInALargerFont() {
        let standard = GridMetrics.naturalWidths(headerCounts: [10], sampleCounts: [10])
        XCTAssertEqual(GridMetrics.naturalWidths(headerCounts: [10], sampleCounts: [10], fontSize: 12), standard)
        XCTAssertEqual(standard, [114])
        // 10 × 7.2 × 16 / 12 + 20 = 116, plus 22.
        XCTAssertEqual(GridMetrics.naturalWidths(headerCounts: [10], sampleCounts: [10], fontSize: 16)[0],
                       138, accuracy: 0.001)
        XCTAssertLessThan(GridMetrics.naturalWidths(headerCounts: [10], sampleCounts: [10], fontSize: 11)[0],
                          standard[0])
    }

    /// A box's edges are snapped half away from zero: a row ending at 456.5 with a box two points
    /// inside it puts the bottom at 454.5, which goes **up** to 455 — measured off the baselines,
    /// where a separator that went down sat a point below the row it belonged to.
    func testTheBoxEdgesSnapHalfAwayFromZero() {
        let first = GridMetrics.box(inRow: CGRect(x: 0, y: 453.5, width: 100, height: 23),
                                    contentHeight: 23, verticalPadding: 0)
        XCTAssertEqual(first.minY, 454)
        XCTAssertEqual(first.height, 23)

        let second = GridMetrics.box(inRow: CGRect(x: 0, y: 454.5, width: 100, height: 23),
                                     contentHeight: 23, verticalPadding: 0)
        XCTAssertEqual(second.minY, 455)
        XCTAssertEqual(second.height, 23)

        // Centred when the row is taller than the box, and still snapped on both edges.
        let centred = GridMetrics.box(inRow: CGRect(x: 0, y: 430, width: 100, height: 27),
                                      contentHeight: 23, verticalPadding: 4)
        XCTAssertEqual(centred.minY, 432)
        XCTAssertEqual(centred.height, 23)
    }

    // MARK: The shared geometry

    private var geometry: GridColumnGeometry { GridColumnGeometry(gutter: 60, widths: [100, 150, 80]) }

    /// Which drawn column a horizontal position falls in — the one piece the header and the body
    /// share, so a click on a header and a click on the cell below it cannot land differently.
    func testTheGeometryPicksTheColumnAPositionFallsIn() {
        let geometry = self.geometry
        XCTAssertEqual(geometry.totalWidth, 390)

        let zero = geometry.edges(of: 0)
        XCTAssertEqual(zero.left, 60)
        XCTAssertEqual(zero.right, 160)
        let last = geometry.edges(of: 2)
        XCTAssertEqual(last.left, 310)
        XCTAssertEqual(last.right, 390)
        // Past the end answers (0, 0), which is what the painter gets when it is asked to draw a
        // column the result does not have.
        let past = geometry.edges(of: 7)
        XCTAssertEqual(past.left, 0)
        XCTAssertEqual(past.right, 0)

        XCTAssertNil(geometry.column(atX: 59))       // the gutter is not a column
        XCTAssertEqual(geometry.column(atX: 60), 0)
        XCTAssertEqual(geometry.column(atX: 159.9), 0)
        XCTAssertEqual(geometry.column(atX: 160), 1)
        XCTAssertEqual(geometry.column(atX: 389.99), 2)
        XCTAssertNil(geometry.column(atX: 390))      // past the last one
    }

    /// A drag that reaches the gutter or the space past the last column is asking to extend the
    /// selection to the edge, not to drop it, so those clamp rather than answer `nil`.
    func testTheClampKeepsADragAtTheEdges() {
        let geometry = self.geometry
        XCTAssertEqual(geometry.clampedColumn(atX: 10, last: 2), 0)
        XCTAssertEqual(geometry.clampedColumn(atX: -40, last: 2), 0)
        XCTAssertEqual(geometry.clampedColumn(atX: 4_000, last: 2), 2)
        XCTAssertEqual(geometry.clampedColumn(atX: 100, last: 2), 0)
    }

    func testTheRowIsClampedToTheRowsThatExist() {
        let geometry = self.geometry
        XCTAssertEqual(geometry.row(atY: 0, rowHeight: 25, count: 40), 0)
        XCTAssertEqual(geometry.row(atY: 62, rowHeight: 25, count: 40), 2)
        XCTAssertEqual(geometry.row(atY: -20, rowHeight: 25, count: 40), 0)
        XCTAssertEqual(geometry.row(atY: 100_000, rowHeight: 25, count: 40), 39)
        XCTAssertEqual(geometry.row(atY: 100, rowHeight: 25, count: 0), 0)
    }

    func testTheColumnsInARangeAreTheOnesThatOverlapIt() {
        XCTAssertEqual(geometry.columns(in: 60...200), 0..<2)
        XCTAssertEqual(geometry.columns(in: 160...161), 1..<2)
        // Past the right edge, still inside a column: the range starts where the pointer is.
        XCTAssertEqual(geometry.columns(in: 300...1_000), 1..<3)
        // Clamped at both ends: a dirty rectangle that runs off the right edge entirely means the
        // last column, not an empty range.
        XCTAssertEqual(geometry.columns(in: 500...1_000), 2..<3)
        // …and one entirely in the gutter is the first column, because the gutter is drawn with it.
        XCTAssertEqual(geometry.columns(in: -50...10), 0..<1)
        XCTAssertEqual(GridColumnGeometry(gutter: 0, widths: []).columns(in: 0...10), 0..<0)
    }

    // MARK: What repaints

    private func inputs(selection: CellRange? = nil,
                        rowHeight: CGFloat = 25,
                        revision: Int = 1,
                        visibleSources: [Int] = [0]) -> GridInputs {
        GridInputs(revision: revision,
                   layout: GridInputs.GridColumnLayout(columnWidths: [100], visibleSources: visibleSources),
                   selection: selection,
                   edits: CellEdits(),
                   sort: nil,
                   filtered: [],
                   style: GridInputs.GridStyle(rowHeight: rowHeight, alternateRows: true,
                                               showRowNumbers: true, nullDisplay: "null",
                                               codeFontFamily: "", accent: "ice", isDark: false,
                                               sortEnabled: true),
                   filterPopover: nil,
                   viewing: nil)
    }

    /// A selection that moved one row invalidates the rows it left and the rows it entered — two,
    /// not one, or the cell it left keeps its wash.
    func testASelectionThatMovedOneRowInvalidatesBothRows() {
        let before = inputs(selection: CellRange(from: (3, 0), to: (3, 0)))
        let after = inputs(selection: CellRange(from: (4, 0), to: (4, 0)))
        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: before, new: after,
                                                     oldRowCount: 40, newRowCount: 40),
                       IndexSet([3, 4]))
        XCTAssertEqual(GridPaintDiff.invalidatedColumns(old: before, new: after), 0...0)
    }

    /// An input that changed nothing invalidates no row at all.
    func testAnUnchangedInputInvalidatesNothing() {
        let one = inputs(selection: CellRange(from: (3, 0), to: (5, 1)))
        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: one, new: one,
                                                     oldRowCount: 40, newRowCount: 40),
                       IndexSet())
    }

    /// Rows arriving or leaving invalidate exactly the ones that are new, and everything is
    /// invalidated when the geometry under every row moves.
    func testARowCountChangeInvalidatesTheNewRowsAndAStyleChangeInvalidatesAllOfThem() {
        let base = inputs()
        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: base, new: base,
                                                     oldRowCount: 40, newRowCount: 44),
                       IndexSet(integersIn: 40..<44))
        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: base, new: base,
                                                     oldRowCount: 44, newRowCount: 40),
                       IndexSet(integersIn: 40..<44))

        let taller = inputs(rowHeight: 30)
        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: base, new: taller,
                                                     oldRowCount: 40, newRowCount: 40),
                       IndexSet(integersIn: 0..<40))
        // `nil` is every column: a style change moved the separators under all of them.
        XCTAssertNil(GridPaintDiff.invalidatedColumns(old: base, new: taller))
    }

    /// `stagedKeys` names **source** columns and `allPositions` names **display** ones. On a grid
    /// with a hidden column the two disagree, and mapping one to the other by index repaints a
    /// cell that never changed while missing the one that did.
    func testAStagedEditIsMappedFromSourceColumnToDisplayColumn() {
        // Both sides draw the same columns, so the layout does not change and the diff has to
        // answer with a real range rather than with "every column".
        let before = inputs(visibleSources: [2, 0])
        var after = inputs(visibleSources: [2, 0])
        var edits = CellEdits()
        edits.edit("staged", at: CellKey(row: 0, column: 2), original: nil)
        after.edits = edits

        XCTAssertEqual(GridPaintDiff.invalidatedRows(old: before, new: after,
                                                     oldRowCount: 4, newRowCount: 4),
                       IndexSet([0]))
        // Source 2 is display 0 — not display 2, which is off the end of this grid entirely.
        XCTAssertEqual(GridPaintDiff.invalidatedColumns(old: before, new: after), 0...0)
    }
}
