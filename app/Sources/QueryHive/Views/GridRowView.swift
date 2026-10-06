import AppKit
import CoreText
import SwiftUI

/// The colours one `draw(_:)` pass paints with, already resolved for one appearance.
///
/// Resolved rather than resolved-on-demand because a `CGColor` read out of a dynamic `NSColor`
/// during a draw is a per-cell cost, and because a cached `CTLine` has to carry a colour that is
/// already final. `GridPalette.resolve(appearance:)` is the one place that happens, and the table
/// throws the line cache away when it returns a different palette — see `GridRowPainter.epoch`.
struct GridPalette: Equatable {
    /// `ink` — white on a dark appearance, black on a light one — at the alphas the grid uses.
    let ink: CGColor
    let inkStrong: CGColor
    let inkLabel: CGColor
    let inkDim: CGColor
    let inkFaint: CGColor
    let inkNull: CGColor
    let rule: CGColor
    /// `recess` at 0.30: the header band, a shade *of* the surface rather than a slab over it.
    let headerBand: CGColor
    let stripe: CGColor
    let selection: CGColor
    let staged: CGColor
    let accent: CGColor
    let amber: CGColor
    let secondary: CGColor
    /// The fixed categorical tints the type chip uses.
    let mint: CGColor
    let violet: CGColor
    let ice: CGColor
    let coral: CGColor
    let orange: CGColor

    /// The grid's colours for one appearance.
    ///
    /// Built from the same closures `Tone` uses rather than converted from a SwiftUI `Color`, so a
    /// dynamic colour keeps resolving per appearance instead of being snapshotted at whatever
    /// appearance happened to be current.
    static func resolve(_ appearance: NSAppearance) -> GridPalette {
        var palette: GridPalette!
        appearance.performAsCurrentDrawingAppearance {
            func ink(_ alpha: CGFloat) -> CGColor { Tone.inkNS(alpha).cgColor }
            func recess(_ alpha: CGFloat) -> CGColor {
                NSColor(name: nil) { $0.isDark ? NSColor.black.withAlphaComponent(alpha)
                                                : NSColor.white.withAlphaComponent(alpha) }.cgColor
            }
            let accent = ThemeStore.shared.accent
            palette = GridPalette(
                ink: ink(1),
                inkStrong: ink(0.9),
                inkLabel: ink(0.92),
                inkDim: ink(0.35),
                inkFaint: ink(0.05),
                inkNull: ink(0.3),
                rule: ink(0.12),
                headerBand: recess(0.30),
                stripe: ink(0.03),
                selection: accent.glow.resolvedCGColor(alpha: 0.20),
                staged: NSColor(hex: 0xFFB547).withAlphaComponent(0.20).cgColor,
                accent: accent.glow.resolvedCGColor(alpha: nil),
                amber: NSColor(hex: 0xFFB547).cgColor,
                secondary: ink(0.68),
                mint: NSColor(hex: 0x3EE6A8).cgColor,
                violet: NSColor(hex: 0x7B61FF).cgColor,
                ice: NSColor(hex: 0x4FD8FF).cgColor,
                coral: NSColor(hex: 0xFF5E6C).cgColor,
                orange: NSColor.systemOrange.cgColor
            )
        }
        return palette
    }
}

extension Color {
    /// This colour as a `CGColor`, for the AppKit drawing that cannot take a `Color`.
    ///
    /// `alpha` overrides whatever the colour already carries, which is how `Tone.ink` becomes the six
    /// opacities the grid draws in. A dynamic colour is resolved against the given appearance rather
    /// than against whatever happens to be current, which is what lets `GridPalette.resolve` build
    /// the whole palette inside one `performAsCurrentDrawingAppearance`.
    func resolvedCGColor(alpha: CGFloat?, appearance: NSAppearance? = nil) -> CGColor {
        let base = NSColor(self)
        let resolved: NSColor
        if let appearance {
            var value: NSColor = base
            appearance.performAsCurrentDrawingAppearance { value = base }
            resolved = value
        } else {
            resolved = base
        }
        return (alpha.map { resolved.withAlphaComponent($0) } ?? resolved).cgColor
    }
}

/// One row's worth of resolved text, ready to draw.
///
/// Built by the coordinator from `ResultRows` and the column formats, not by the painter: the
/// painter is deliberately given strings and flags rather than the rows, so it has no way to reach
/// `UserDefaults` or to parse a JSON cell on the draw path.
struct GridRowText {
    /// The display column `cells[0]` belongs to: only a range of the columns is built (D-23), so
    /// `cells[i]` is the text of display column `first + i`.
    var first = 0
    /// One per built column, in display order. A slot with no text draws nothing.
    var cells: [String]
    /// The flags that change how a cell is drawn: NULL italic, `∅` for an empty string, `…` for a
    /// value cut short.
    var flags: [CellFlags]

    /// The display columns this row's text covers.
    var built: Range<Int> { first..<(first + cells.count) }
}

/// Everything a `draw(_:)` pass shares between rows: geometry, colours, fonts and the style flags.
///
/// Built once per pass. The fonts and the box heights are measured once here rather than per row,
/// because measuring a line height is an `NSHostingView` round trip.
struct GridPaintContext {
    var geometry: GridColumnGeometry
    var palette: GridPalette
    var cellFont: NSFont
    var gutterFont: NSFont
    var headerFont: NSFont
    var chipFont: NSFont
    var rowHeight: CGFloat
    var dataBoxHeight: CGFloat
    var gutterBoxHeight: CGFloat
    /// One line of a header label, and one of a chip: the two numbers the header's height is built
    /// from, measured here rather than per draw.
    var headerLabelLine: CGFloat
    var chipLine: CGFloat
    var alternateRows: Bool
    var showRowNumbers: Bool
    var nullDisplay: String
    /// One per drawn column: whether it is drawn flush right.
    var numeric: [Bool]

    /// The fonts and box heights for one style, measured. Everything except the geometry, the
    /// palette and the flags, which the caller supplies.
    static func resolve(style: GridInputs.GridStyle, appearance: NSAppearance) -> GridPaintContext {
        let cellFont = FontChoice.codeNSFont(size: 12, weight: nil)
        let gutterFont = FontChoice.codeNSFont(size: 10.5, weight: nil)
        let headerFont = FontChoice.codeNSFont(size: 12, weight: .semibold)
        let chipFont = FontChoice.codeNSFont(size: 11, weight: .semibold)
        return GridPaintContext(
            geometry: GridColumnGeometry(gutter: 0, widths: []),
            palette: GridPalette.resolve(appearance),
            cellFont: cellFont,
            gutterFont: gutterFont,
            headerFont: headerFont,
            chipFont: chipFont,
            rowHeight: style.rowHeight,
            dataBoxHeight: GridMetrics.lineHeight(of: cellFont) + 2 * GridMetrics.cellVerticalPadding,
            gutterBoxHeight: GridMetrics.lineHeight(of: gutterFont) + 2 * GridMetrics.gutterVerticalPadding,
            headerLabelLine: GridMetrics.lineHeight(of: headerFont),
            chipLine: GridMetrics.lineHeight(of: chipFont),
            alternateRows: style.alternateRows,
            showRowNumbers: style.showRowNumbers,
            nullDisplay: style.nullDisplay,
            numeric: []
        )
    }

    /// A context with nothing measured yet, for a header that has not been given one.
    static var empty: GridPaintContext { resolve(style: .placeholder, appearance: .currentDrawing()) }
}

/// Paints one row into a graphics context.
///
/// Table-level rather than per-row-view (blueprint D-1): a row that draws itself into one shared
/// context keeps SwiftUI's paint order and, in Compact, its one-point overflow over the neighbouring
/// rows. An `NSTableRowView` is clipped to its own bounds, and the overflow of the last row would
/// fall outside every row view there is.
enum GridRowPainter {

    /// The six paints of §6.1, in order, for one row.
    ///
    /// `row` is the row's index in the rows the grid is drawing, which is what the gutter prints and
    /// what the selection rectangle holds.
    static func paint(row: Int,
                      columns: Range<Int>,
                      context: GridPaintContext,
                      text: GridRowText,
                      staged: Set<Int>,
                      selection: CellRange?,
                      lines: GridLineCache,
                      width: CGFloat,
                      into cg: CGContext) {
        let rowRect = CGRect(x: 0, y: CGFloat(row) * context.rowHeight,
                             width: width, height: context.rowHeight)

        // 1. Stripe, over the whole document width rather than only as far as the last column: a
        //    band that stopped at the last separator read as a row that had run out.
        if context.alternateRows, row % 2 == 1 {
            cg.setFillColor(context.palette.stripe)
            cg.fill(rowRect)
        }

        // 2. Gutter.
        if context.showRowNumbers {
            paintGutter(row: row, context: context, lines: lines, into: cg)
        }

        // 3. Each column in `columns`: wash, text, staged dot, separator. The rest are off the
        //    dirty rectangle, and their text was never built.
        for column in columns.clamped(to: 0..<context.geometry.widths.count) {
            let edges = context.geometry.edges(of: column)
            paintCell(row: row, column: column,
                      edges: edges,
                      rowRect: rowRect,
                      context: context,
                      text: text,
                      staged: staged.contains(column),
                      selection: selection,
                      lines: lines,
                      into: cg)
        }
    }

    private static func paintGutter(row: Int, context: GridPaintContext,
                                    lines: GridLineCache, into cg: CGContext) {
        let rowRect = CGRect(x: 0, y: CGFloat(row) * context.rowHeight,
                             width: context.geometry.gutter, height: context.rowHeight)
        let box = GridMetrics.box(inRow: rowRect,
                                  contentHeight: context.gutterBoxHeight,
                                  verticalPadding: 0)
        let content = box.insetBy(dx: GridMetrics.cellPadding, dy: 0)
        let number = "\(row + 1)"
        let line = lines.line(number, font: context.gutterFont, color: context.palette.inkDim)
        let lineWidth = CTLineGetTypographicBounds(line, nil, nil, nil)
        // Right-aligned in the 44 pt column, which is what the header's "#" does too.
        let x = content.maxX - CGFloat(lineWidth)
        draw(line: line, at: CGPoint(x: x, y: baseline(for: line, in: box, font: context.gutterFont)),
             into: cg)
        cg.setFillColor(context.palette.inkFaint)
        cg.fill(CGRect(x: box.maxX - 1, y: box.minY, width: 1, height: box.height))
    }

    private static func paintCell(row: Int, column: Int,
                                  edges: (left: CGFloat, right: CGFloat),
                                  rowRect: CGRect,
                                  context: GridPaintContext,
                                  text: GridRowText,
                                  staged: Bool,
                                  selection: CellRange?,
                                  lines: GridLineCache,
                                  into cg: CGContext) {
        let columnRect = CGRect(x: edges.left, y: rowRect.minY,
                                width: edges.right - edges.left, height: rowRect.height)
        let box = GridMetrics.box(inRow: columnRect,
                                  contentHeight: context.dataBoxHeight,
                                  verticalPadding: 0)

        // Wash: a staged change wins over the selection, because what the user has changed is the
        // thing they most need to see.
        let selected = selection?.contains(row: row, column: column) ?? false
        if staged {
            cg.setFillColor(context.palette.staged)
            cg.fill(box)
        } else if selected {
            cg.setFillColor(context.palette.selection)
            cg.fill(box)
        }

        // Text.
        let slot = column - text.first
        let flags = text.flags.indices.contains(slot) ? text.flags[slot] : CellFlags()
        let shown = text.cells.indices.contains(slot) ? text.cells[slot] : ""
        let content = box.insetBy(dx: GridMetrics.cellPadding, dy: 0)
        let numeric = context.numeric.indices.contains(column) ? context.numeric[column] : false
        if flags.contains(.null) {
            draw(line: lines.line(context.nullDisplay,
                                  font: context.cellFont,
                                  color: context.palette.inkNull,
                                  italic: true),
                 at: CGPoint(x: content.minX,
                             y: baseline(forHeight: context, in: box, font: context.cellFont)),
                 into: cg)
        } else if flags.contains(.empty) {
            draw(line: lines.line("∅", font: context.cellFont, color: context.palette.inkNull),
                 at: CGPoint(x: content.minX,
                             y: baseline(forHeight: context, in: box, font: context.cellFont)),
                 into: cg)
        } else if !shown.isEmpty {
            let clipped = lines.line(shown, font: context.cellFont, color: context.palette.inkStrong,
                                     width: content.width)
            let lineWidth = CGFloat(CTLineGetTypographicBounds(clipped, nil, nil, nil))
            let x = numeric ? content.maxX - lineWidth : content.minX
            draw(line: clipped, at: CGPoint(x: x, y: baseline(forHeight: context, in: box, font: context.cellFont)),
                 into: cg)
        }

        // Staged dot: 4 × 4 at the box's top-right corner, 3 in from each edge. A dot as well as the
        // wash, because a cell can be both selected and changed and one tint cannot say which.
        if staged {
            cg.setFillColor(context.palette.amber)
            cg.fillEllipse(in: CGRect(x: box.maxX - 3 - 4, y: box.minY + 3, width: 4, height: 4))
        }

        // Separator: 1 pt at the box's right edge.
        cg.setFillColor(context.palette.inkFaint)
        cg.fill(CGRect(x: box.maxX - 1, y: box.minY, width: 1, height: box.height))
    }

    /// Where a line's baseline sits inside a cell box.
    ///
    /// The box's centre, offset by half the font's ascent–descent difference: that centres the
    /// glyphs' own extent rather than the line box, which is what SwiftUI's centred `Text` does. The
    /// value is rounded, because a fractional baseline on a 1× display resolves to one of two pixel
    /// rows depending on the row's own y, and a grid whose text jitters by a pixel between rows is
    /// exactly what the layout gate would catch.
    private static func baseline(forHeight context: GridPaintContext, in box: CGRect,
                                 font: NSFont) -> CGFloat {
        baseline(for: font, box: box)
    }

    private static func baseline(for line: CTLine, in box: CGRect, font: NSFont) -> CGFloat {
        baseline(for: font, box: box)
    }

    private static func baseline(for font: NSFont, box: CGRect) -> CGFloat {
        let ascent = CTFontGetAscent(font as CTFont)
        let descent = CTFontGetDescent(font as CTFont)
        return (box.midY + (ascent - descent) / 2).rounded()
    }

    private static func draw(line: CTLine, at point: CGPoint, into cg: CGContext) {
        cg.textPosition = point
        CTLineDraw(line, cg)
        cg.textPosition = .zero
    }
}

/// `CTLine`s, keyed by the text and the style that produced them.
///
/// Two generations rather than one map plus eviction: when the current map passes a bound the old
/// one is dropped and the current becomes it, so nothing is evicted while it is still being asked
/// for and a lookup that misses the current generation still finds the previous one.
///
/// Keyed by the *text* rather than by (row, column) as the blueprint sketched. A row/column key is
/// only valid while the rows under it do not change, and the grid replaces its whole result up to
/// five times a second while a run streams — the same key would then hand back a line for a value
/// that is no longer there. Keying by the text is also what makes a column of repeated values cost
/// one line.
final class GridLineCache {
    private struct Key: Hashable {
        let text: String
        let font: String
        let color: String
        let italic: Bool
        let width: CGFloat
    }

    private var current: [Key: CTLine] = [:]
    private var previous: [Key: CTLine] = [:]
    private var approximateBytes = 0
    /// Raised when the fonts or the palette change: lines built then are no longer drawable.
    private(set) var epoch = 0

    /// The bound on the current generation, in entries. A guess the Fase 2 bench tunes; a line is
    /// tens of bytes plus the text it was built from, so this is well inside the memory budget.
    private let limit = 32_768
    /// The bound in bytes, which is what actually protects the footprint: a row of 1,024-character
    /// values costs far more than a row of numbers, and only counting entries would let a wide
    /// result through.
    private let byteLimit = 8 * 1_024 * 1_024

    /// One line, cut to `width` when a width is given.
    ///
    /// The cut is unconditional and does not take a switch, because a cell that is *not* cut is a
    /// cell that draws across its neighbours: the SwiftUI cell it replaces was `lineLimit(1)` with
    /// `.truncationMode(.tail)` — always a tail ellipsis at the frame — and gating the cut on a
    /// "this value was long" flag let a 130-character name run over the four columns after it. A
    /// line that already fits is returned by `CTLineCreateTruncatedLine` untouched, so clipping is
    /// not a change for the 99% of cells that fit; it is the 1% that stop leaking.
    func line(_ text: String, font: NSFont, color: CGColor, italic: Bool = false,
              width: CGFloat = .greatestFiniteMagnitude) -> CTLine {
        let key = Key(text: text,
                      font: "\(font.fontName)@\(font.pointSize)|\(font.fontDescriptor.symbolicTraits.rawValue)",
                      color: String(describing: color.components ?? []) + "\(color.alpha)",
                      italic: italic, width: width)
        if let hit = current[key] { return hit }
        if let hit = previous[key] { current[key] = hit; return hit }
        let made = build(text, font: font, color: color, italic: italic, width: width)
        current[key] = made
        approximateBytes += 48 + text.utf8.count * 2
        if current.count > limit || approximateBytes > byteLimit { rotate() }
        return made
    }

    private func build(_ text: String, font: NSFont, color: CGColor, italic: Bool,
                       width: CGFloat) -> CTLine {
        var attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: NSColor(cgColor: color) ?? .labelColor,
        ]
        if italic {
            // A shear, the same way `.italic()` obliques a face that has no italic of its own.
            var matrix = CGAffineTransform(a: 1, b: 0, c: 0.2, d: 1, tx: 0, ty: 0)
            let sheared = CTFontCreateWithFontDescriptor(
                font.fontDescriptor, font.pointSize, &matrix)
            attributes[.font] = sheared
        }
        let attributed = NSAttributedString(string: text, attributes: attributes)
        let line = CTLineCreateWithAttributedString(attributed)
        guard width < .greatestFiniteMagnitude else { return line }
        let token = CTLineCreateWithAttributedString(NSAttributedString(string: "…", attributes: attributes))
        return CTLineCreateTruncatedLine(line, Double(width), .end, token) ?? line
    }

    private func rotate() {
        previous = current
        current = [:]
        approximateBytes = 0
    }

    /// Throw the current generation away because the fonts, the palette or the appearance changed.
    func invalidate() {
        current = [:]
        previous = [:]
        approximateBytes = 0
        epoch += 1
    }
}

/// Per-row display text, so a row that is redrawn during a scroll does not re-render its formats.
///
/// Bounded by the rows around the viewport rather than by a count: the grid is asked for the rows it
/// is showing, and a fling asks for a sliding window of a few hundred. Keeping the window and
/// dropping what has left it is what keeps a million-row result from caching a million rows.
final class GridRowTextCache {
    private var rows: [Int: GridRowText] = [:]
    private var bytes = 0
    private let byteLimit = 16 * 1_024 * 1_024

    /// The text of `row` covering at least `columns`. A row built over a range that does not hold
    /// them is built again over the new one (`build` is given the range to cover).
    func text(at row: Int, columns: Range<Int>, build: (Range<Int>) -> GridRowText) -> GridRowText {
        if let hit = rows[row], hit.built.lowerBound <= columns.lowerBound && columns.upperBound <= hit.built.upperBound { return hit }
        let made = build(columns)
        if let old = rows[row] { bytes -= Self.size(of: old) }
        rows[row] = made
        bytes += Self.size(of: made)
        if bytes > byteLimit { prune() }
        return made
    }

    private static func size(of row: GridRowText) -> Int {
        row.cells.reduce(48) { $0 + $1.utf8.count * 2 }
    }

    /// Keep the rows around `focus`: everything further than `page` rows away is dropped, and if
    /// that is not enough the window is halved until the estimate fits.
    func keep(around focus: ClosedRange<Int>, page: Int) {
        for distance in stride(from: 2, through: 1, by: -1) {
            let low = focus.lowerBound - page * distance
            let high = focus.upperBound + page * distance
            let kept = rows.filter { low...high ~= $0.key }
            let size = kept.values.reduce(0) { $0 + Self.size(of: $1) }
            if size <= byteLimit || distance == 1 {
                rows = Dictionary(uniqueKeysWithValues: kept.map { ($0.key, $0.value) })
                bytes = size
                return
            }
        }
    }

    private func prune() {
        let keys = rows.keys.sorted()
        guard let low = keys.first, let high = keys.last else { return }
        keep(around: low...high, page: max(1, (high - low) / 4))
    }

    func removeAll() {
        rows.removeAll()
        bytes = 0
    }
}

/// Which rows a change to the grid's inputs has to repaint.
///
/// Pure, so it is tested without a window: a selection that moved one row invalidates two, an
/// unchanged input invalidates none, and a change of columns or style invalidates everything because
/// the geometry under every row moved.
enum GridPaintDiff {
    static func invalidatedRows(old: GridInputs?, new: GridInputs,
                                oldRowCount: Int, newRowCount: Int) -> IndexSet {
        var result = IndexSet()
        if oldRowCount != newRowCount {
            let start = min(oldRowCount, newRowCount)
            result.insert(integersIn: start..<max(oldRowCount, newRowCount))
        }
        if old?.selection != new.selection {
            let union = Set((old?.selection?.allPositions ?? []) + (new.selection?.allPositions ?? []))
            for row in union.map({ $0.row }) { result.insert(row) }
        }
        let oldStaged = Set(old?.edits.stagedKeys ?? [])
        let newStaged = Set(new.edits.stagedKeys)
        if oldStaged != newStaged {
            for key in oldStaged.union(newStaged) { result.insert(key.row) }
        }
        if old?.layout != new.layout || old?.style != new.style {
            result.insert(integersIn: 0..<newRowCount)
        }
        return result
    }

    /// The columns a change has to repaint, for the rows that both inputs share. `nil` means every
    /// column: the layout or the style moved under all of them.
    static func invalidatedColumns(old: GridInputs?, new: GridInputs) -> ClosedRange<Int>? {
        if old?.layout != new.layout || old?.style != new.style { return nil }
        var columns = IndexSet()
        if old?.selection != new.selection {
            let all = (old?.selection?.allPositions ?? []) + (new.selection?.allPositions ?? [])
            for position in all { columns.insert(position.column) }
        }
        // Source to display, here and nowhere else: `stagedKeys` names sources, `allPositions`
        // names display columns, and `edges(of:)` below takes the second. A staged edit in a
        // source the display does not show has no rectangle to repaint at all, so it drops.
        let visible = new.layout.visibleSources
        for key in (old?.edits.stagedKeys ?? []) + new.edits.stagedKeys {
            if let display = visible.firstIndex(of: key.column) { columns.insert(display) }
        }
        guard let low = columns.min(), let high = columns.max() else { return nil }
        return low...high
    }
}
