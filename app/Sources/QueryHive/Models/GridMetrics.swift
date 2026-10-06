import AppKit
import SwiftUI

/// The grid's numbers, as pure values: no view, no table, no state.
///
/// Everything the header, the painter and the table have to agree on lives here, so a disagreement
/// is a compile error rather than a pixel that is one point out in one of the three.
enum GridMetrics {
    /// The horizontal padding a cell carries on each side. A drawn column is its content width plus
    /// twice this, which is why `naturalWidths` measures the padding into the number it returns.
    static let cellPadding: CGFloat = 8
    /// The vertical padding around a data cell's text. `23 = 15 + 2 × 4` at the default row height.
    static let cellVerticalPadding: CGFloat = 4
    /// The gutter cell's box height, a constant since W10-T2: the number is 11 pt, and the box is
    /// not rebuilt from its line height (`13 + 2 × 6` at 10.5 pt, 14 + 2 × 6 at 11) because the
    /// separator that ends at the row's base, and the order the rows paint in, were measured against
    /// 25. The number is centred in the box rather than hung from its top.
    static let gutterBoxHeight: CGFloat = 25
    /// The row number's size: at the 11 pt floor, like everything a person has to read.
    static let gutterFontSize: CGFloat = 11
    /// The header band's own vertical padding, above the label and below the chip: `50 = 6 + 15 + 3
    /// + (14 + 2 × 3) + 6`. Six rather than the cell's four, because the header band is a surface of
    /// its own and the extra point on each side is what separated it from the first row.
    static let headerVerticalPadding: CGFloat = 6
    /// The number's own width, before its padding.
    static let gutterContent: CGFloat = 44
    /// The gap between the header's label and its type chip.
    static let headerSpacing: CGFloat = 3
    /// The chip's own vertical padding, above and below its text.
    static let chipPadding: CGFloat = 3
    /// The chip's horizontal padding, each side. The pill hugs its text — `Chip` is a `Text` with
    /// `.padding(.horizontal, 8)` and a `Capsule` background — so its box is the text's width plus
    /// twice this, and it does **not** stretch across the column.
    static let chipHorizontalPadding: CGFloat = 8

    /// The row-number column's full width, or zero when it is switched off. It is not one of the
    /// drawn columns, so it comes out of the panel before the slack those columns share is worked out.
    static func gutterWidth(showRowNumbers: Bool) -> CGFloat {
        showRowNumbers ? gutterContent + 2 * cellPadding : 0
    }

    /// One drawn column's full width, padding included.
    ///
    /// The formula the grid has always used: `min(max(n × 7.2 + 20, 84), 320) + 22`. The 7.2 pt per
    /// character is the monospaced advance at 12 pt, the 84–320 clamp keeps a two-character column
    /// readable and a 300-character one from eating the panel, and the trailing 22 is the cell's own
    /// 16 pt of padding plus the room the header's funnel needs. `n` is the wider of the header label
    /// and the longest of the first 200 fetched rows; NULL counts as 4.
    ///
    /// The advance scales with the cell font, or a larger font would cut every column short with an
    /// ellipsis; at the standard size the product is exactly 7.2, so nothing moves.
    static func naturalWidths(headerCounts: [Int], sampleCounts: [Int],
                              fontSize: Int = DataPreferences.standardGridFontSize) -> [CGFloat] {
        let advance = 7.2 * CGFloat(fontSize) / 12
        return zip(headerCounts, sampleCounts).map { header, sample in
            let n = max(header, sample)
            return min(max(CGFloat(n) * advance + 20, 84), 320) + 22
        }
    }

    /// The columns widened to fill the panel.
    ///
    /// A result whose columns stop two thirds of the way across reads as unfinished, and the empty
    /// band beside it is the first thing the eye lands on, so any slack is shared out in proportion
    /// to what each column already has. The gutter is subtracted first, because it is not one of
    /// these columns and its width would otherwise always be the amount they fell short by. A
    /// result that already overflows the panel is returned untouched: there is no slack, and
    /// rounding widths that are about to scroll anyway would only move the separators.
    static func fitted(_ natural: [CGFloat], available: CGFloat, gutter: CGFloat) -> [CGFloat] {
        let total = natural.reduce(0, +)
        let forColumns = available - gutter
        guard total > 0, total < forColumns else { return natural }
        let slack = forColumns - total
        return natural.map { $0 + slack * ($0 / total) }
    }

    /// Whether a column's type reads as a number, and so is drawn flush right and tinted mint.
    ///
    /// A `contains` over a list, quirks included: `point`, `interval` and `hstore` all match `int`
    /// and are drawn as numbers. That is what the grid has always done and the baseline records it,
    /// so it is kept — see the backlog entry in the Fase 5 blueprint, §1.3 item 6.
    static func isNumeric(type: String) -> Bool {
        let lowered = type.lowercased()
        return ["int", "long", "double", "decimal", "real", "bigint", "smallint", "tinyint",
                "numeric", "float"].contains { lowered.contains($0) }
    }

    /// One line's height in `font`, as SwiftUI lays it out.
    ///
    /// Measured rather than derived, and asked of SwiftUI rather than of AppKit. SwiftUI's line box
    /// is not `NSFont`'s ascent + descent + leading: at 12 pt monospaced those add up to 14.13 and
    /// SwiftUI gives 15, while at 10.5 pt they add up to 12.37 and SwiftUI gives 13. The header
    /// height (50 pt) and both cell boxes (23 and 25 pt) are built from this number, so a formula
    /// that merely looked plausible is a point out at exactly the sizes the grid draws.
    ///
    /// `Font(nsFont)` is the same font SwiftUI would have resolved, and it measures identically to
    /// `.system(size:design:.monospaced)`, so the answer is right for a chosen code family too.
    /// Cached per font: there are four of them in the grid and this is called on the main thread.
    static func lineHeight(of font: NSFont) -> CGFloat {
        let key = "\(font.fontName)@\(font.pointSize)"
        if let cached = lineHeights[key] { return cached }
        let host = NSHostingView(rootView: Text("x").font(Font(font)).fixedSize())
        host.layoutSubtreeIfNeeded()
        let height = host.fittingSize.height
        lineHeights[key] = height
        return height
    }

    /// Main-thread only, like every other part of the draw path.
    private static var lineHeights: [String: CGFloat] = [:]

    /// The header's height: the label line, the chip's line, and the padding between and around them.
    ///
    /// Built from the font metrics rather than written as 50, so a user who chooses another code
    /// family still gets a header that fits what is drawn in it — which is what the SwiftUI stack
    /// this replaces did.
    static func headerHeight(labelLine: CGFloat, chipLine: CGFloat) -> CGFloat {
        headerVerticalPadding + labelLine + headerSpacing
            + (chipLine + 2 * chipPadding) + headerVerticalPadding
    }

    /// The header's height for the fonts the grid draws with.
    static func headerHeight(fontSize: Int = DataPreferences.standardGridFontSize) -> CGFloat {
        headerHeight(labelLine: lineHeight(of: FontChoice.codeNSFont(size: CGFloat(fontSize), weight: .semibold)),
                     chipLine: lineHeight(of: FontChoice.codeNSFont(size: 11, weight: .semibold)))
    }

    /// A data cell's box height: one line of the cell font plus its vertical padding. 23 at the default.
    static func dataBoxHeight(fontSize: Int = DataPreferences.standardGridFontSize) -> CGFloat {
        lineHeight(of: FontChoice.codeNSFont(size: CGFloat(fontSize), weight: nil)) + 2 * cellVerticalPadding
    }

    /// The box a cell of this height occupies inside its row.
    ///
    /// Centred, then each vertical edge snapped with `.rounded()` — half away from zero — and the
    /// height taken as the difference. The snap is not cosmetic: measured against the baselines, a
    /// 30 pt row's separator ends at 454 and its gutter's at 455 for a row ending at 457, which is
    /// 453.5 and 454.5 rounded up. In Compact the box is taller than its row and simply overflows,
    /// one point above and one below (two for the gutter); the painter's row order is what decides
    /// which row wins the overlap.
    ///
    /// The box spans the row's full width. The cell's own horizontal padding is inside the column
    /// width `naturalWidths` already accounted for, so it is applied to the text, not here.
    static func box(inRow row: CGRect, contentHeight: CGFloat, verticalPadding: CGFloat) -> CGRect {
        let inset = (row.height - contentHeight) / 2
        let top = (row.minY + inset).rounded()
        let bottom = (row.maxY - inset).rounded()
        return CGRect(x: row.minX, y: top, width: row.width, height: bottom - top)
    }
}

/// Where each drawn column starts and ends, and which one a horizontal position falls in.
///
/// The one piece of geometry the header and the table body share, so a click on a header and a click
/// on the cell below it cannot land in different columns. Widths are the full drawn widths
/// (`naturalWidths` or `fitted`), padding included.
struct GridColumnGeometry: Equatable {
    let gutter: CGFloat
    let widths: [CGFloat]

    init(gutter: CGFloat, widths: [CGFloat]) {
        self.gutter = gutter
        self.widths = widths
    }

    var totalWidth: CGFloat { gutter + widths.reduce(0, +) }

    /// The left and right edge of a drawn column, or `(0, 0)` past the end.
    ///
    /// Deliberately **fractional**, and deliberately not snapped the way `GridMetrics.box` snaps a
    /// row's edges. A separator is drawn at `right - 1`; at a fractional right that line straddles
    /// two pixels, and which of the two the parity scan reads depends on the fraction — which is
    /// exactly what the baselines record, because the SwiftUI grid they came from laid its `HStack`
    /// out continuously and let the rasteriser do the same. Rounding each edge onto a whole pixel
    /// moved two of seven separators by 1 pt in every grid scene. The two edges do not drift,
    /// because `right` is computed from the column's own run of widths rather than from the
    /// previous column's snapped right.
    func edges(of position: Int) -> (left: CGFloat, right: CGFloat) {
        guard position >= 0, position < widths.count else { return (0, 0) }
        var left = gutter
        for index in 0..<position { left += widths[index] }
        return (left, left + widths[position])
    }

    /// The column a position falls in, or `nil` in the gutter or past the last column.
    func column(atX x: CGFloat) -> Int? {
        guard x >= gutter else { return nil }
        var edge = gutter
        for (index, width) in widths.enumerated() {
            if x < edge + width { return index }
            edge += width
        }
        return nil
    }

    /// The column a position falls in, clamped at both ends: the gutter and the empty space past the
    /// last column both mean the nearest column, because a drag that reaches them is asking to
    /// extend the selection to the edge, not to drop it.
    func clampedColumn(atX x: CGFloat, last: Int) -> Int {
        column(atX: x) ?? (x < gutter ? 0 : last)
    }

    /// The drawn columns that overlap a horizontal range. Used by `draw(_:)` to skip the columns a
    /// dirty rectangle cannot see.
    func columns(in range: ClosedRange<CGFloat>) -> Range<Int> {
        guard !widths.isEmpty else { return 0..<0 }
        let start = clampedColumn(atX: range.lowerBound, last: widths.count - 1)
        let end = clampedColumn(atX: range.upperBound, last: widths.count - 1)
        return start..<(end + 1)
    }

    /// The row a vertical position falls in, clamped to the rows that exist.
    func row(atY y: CGFloat, rowHeight: CGFloat, count: Int) -> Int {
        guard count > 0, rowHeight > 0 else { return 0 }
        return max(0, min(Int(floor(y / rowHeight)), count - 1))
    }
}
