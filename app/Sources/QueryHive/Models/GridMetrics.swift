import Foundation
import AppKit
import QueryHiveFFI

enum GridMetrics {
    static let cellPadding: CGFloat = 8
    static let gutterContent: CGFloat = 44

    static func gutterWidth(showRowNumbers: Bool) -> CGFloat {
        showRowNumbers ? gutterContent + cellPadding * 2 : 0
    }

    /// Content width per drawn column (no padding), the formula of ResultGrid.swift:94.
    static func naturalWidths(headerCounts: [Int], sampleCounts: [Int]) -> [CGFloat] {
        zip(headerCounts, sampleCounts).map { header, sample in
            let n = max(header, sample)
            return min(max(CGFloat(n) * 7.2 + 20, 84), 320) + 2 * cellPadding
        }
    }

    /// Slack shared out in proportion, gutter subtracted.
    static func fitted(_ natural: [CGFloat], available: CGFloat, gutter: CGFloat) -> [CGFloat] {
        let total = natural.reduce(0, +)
        let forColumns = available - gutter
        guard total > 0, total < forColumns else { return natural }
        let slack = forColumns - total
        return natural.map { $0 + slack * ($0 / total) }
    }

    /// The current contains-list quirk, kept for pixel parity.
    static func isNumeric(type: String) -> Bool {
        type.localizedCaseInsensitiveContains("int")
    }

    static func lineHeight(of font: NSFont) -> CGFloat {
        ceil(font.ascender + font.descender + font.leading)
    }

    /// Box a cell occupies inside its row, snapped the way §1.1 measured.
    static func box(inRow row: CGRect, contentHeight: CGFloat, verticalPadding: CGFloat) -> CGRect {
        // Snap top and bottom separately with .rounded() (half away from zero).
        let top = row.minY + ((row.height - contentHeight) / 2).rounded()
        let bottom = row.maxY - ((row.height - contentHeight) / 2).rounded()
        let height = bottom - top
        return CGRect(x: row.minX + verticalPadding, y: top, width: row.width - 2 * verticalPadding, height: height)
    }

    static func headerHeight(labelLine: CGFloat, chipLine: CGFloat) -> CGFloat {
        // 6 + l + 3 + (c + 6) + 6
        6 + labelLine + 3 + (chipLine + 6) + 6
    }
}

struct GridColumnGeometry: Equatable {
    let gutter: CGFloat
    let widths: [CGFloat] // widths include the 2 × cellPadding

    var totalWidth: CGFloat { gutter + widths.reduce(0, +) }

    func edges(of position: Int) -> (left: CGFloat, right: CGFloat) {
        guard position >= 0, position < widths.count else { return (0, 0) }
        var left = gutter
        for i in 0..<position { left += widths[i] }
        return (left, left + widths[position])
    }

    func column(atX x: CGFloat) -> Int? {
        guard x >= gutter else { return nil }
        var edge = gutter
        for (index, width) in widths.enumerated() {
            if x < edge + width { return index }
            edge += width
        }
        return nil
    }

    func clampedColumn(atX x: CGFloat, last: Int) -> Int {
        guard x >= gutter else { return 0 }
        var edge = gutter
        for (index, width) in widths.enumerated() {
            if x < edge + width { return index }
            edge += width
        }
        return last
    }

    func columns(in range: ClosedRange<CGFloat>) -> ClosedRange<Int> {
        let start = column(atX: range.lowerBound) ?? 0
        let end = column(atX: range.upperBound) ?? (widths.count - 1)
        return start...end
    }

    func row(atY y: CGFloat, rowHeight: CGFloat, count: Int) -> Int {
        let row = Int(floor(y / rowHeight))
        return max(0, min(row, count - 1))
    }
}