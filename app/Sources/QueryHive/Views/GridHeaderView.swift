import AppKit
import CoreText

/// The grid's column headers, drawn by hand on top of the table.
///
/// An `NSTableHeaderView` subclass with its own `draw(_:)` and its own hit-testing, for the same
/// reason the table body draws itself: the grid has **one** `NSTableColumn` (blueprint D-2), so
/// AppKit's header machinery knows about one column and would draw one cell. Everything here comes
/// from `GridColumnGeometry`, which the body reads too, so a click on a header and a click on the
/// cell below it cannot land in different columns.
///
/// It still scrolls with the content and stays above it, which is `NSTableHeaderView`'s own
/// behaviour and the reason to subclass it rather than draw a band beside the table.
final class GridHeaderView: NSTableHeaderView {

    /// One drawn column, as the header needs it: identity, what to draw, and how wide.
    struct Column: Equatable {
        /// The source index — what the sort and the filter are keyed by.
        var source: Int
        /// The label, after a rename.
        var title: String
        /// The engine's own type name, for the chip.
        var type: String
        /// The **drawn** width, padding included.
        var width: CGFloat
        var sortDirection: GridSort.Direction?
        var isRenamed: Bool
        var isFiltered: Bool
        /// The filter's own label — "3 values", "contains abc" — for the funnel's tooltip, which
        /// names what is applied rather than which column it is on.
        var filterLabel: String?
        /// Whether the column is drawn flush right, in the body and in the header alike.
        var isNumeric: Bool
    }

    /// The columns, in display order. Setting this redraws and re-measures the header's height.
    var columns: [Column] = [] {
        didSet {
            if columns != oldValue {
                height = Self.measuredHeight(columns: columns)
                needsDisplay = true
            }
        }
    }

    /// Whether the row-number column is drawn. It is not one of `columns`.
    var showRowNumbers = true {
        didSet { if showRowNumbers != oldValue { needsDisplay = true } }
    }

    /// The one geometry the header and the body share.
    var geometry = GridColumnGeometry(gutter: 0, widths: []) {
        didSet { if geometry != oldValue { needsDisplay = true } }
    }

    /// The header draws with the coordinator's palette rather than one of its own, so a draw it
    /// makes on its own — before the table has been through an update — is still the right colours.
    var paint = GridPaintContext.empty {
        didSet { needsDisplay = true }
    }

    /// The table's commands, so a header click reaches the same sort cycle a menu item does.
    var commands: GridCommands?
    /// Builds the header's right-click menu for one column. `display` is the drawn position, which
    /// is what Move Left/Right act on; `truncated` says whether the result can be re-sorted on the
    /// server, which is what adds the "on Server" items.
    var columnMenu: ((_ source: Int, _ display: Int, _ truncated: Bool) -> NSMenu?)?
    /// Whether the result is cut short by the row limit, for the menu's server items.
    var resultTruncated = false

    /// Left and right mouse, so a funnel click opens the filter without moving the selection.
    var onFilterClick: ((Int, CGRect) -> Void)?

    /// The header's height, top-down. Re-measured whenever the columns change.
    private(set) var height: CGFloat = GridMetrics.headerHeight()

    private var palette: GridPalette { paint.palette }
    private var lines: GridLineCache? { lineCache }

    /// The line cache the table also draws into, so a header label and a cell value that happen to
    /// be the same string cost one line between them.
    weak var lineCache: GridLineCache?

    /// The registered tooltip region in window coordinates, for `GridToolTip.local`.
    private var tooltipWindowRect: NSRect?

    override var isFlipped: Bool { true }

    /// AppKit draws nothing here itself: with one table column its own header cell would be a single
    /// band the width of the document.
    override func draw(_ dirtyRect: NSRect) {
        guard let cg = NSGraphicsContext.current?.cgContext else { return }
        // AppKit calls this once per header and once per resize; a header that has not been given
        // columns yet has nothing to draw and must not paint a band over the top of the grid.
        guard !columns.isEmpty else { return }
        cg.saveGState()
        // A flipped view: the CTM's y now points down, so the text matrix has to point back up or
        // every glyph is drawn mirrored. `textPosition` stays in the view's own coordinates.
        cg.textMatrix = CGAffineTransform(scaleX: 1, y: -1)

        // The band, then the hairline that separates it from the rows.
        cg.setFillColor(palette.headerBand)
        cg.fill(bounds)
        cg.setFillColor(palette.rule)
        cg.fill(CGRect(x: dirtyRect.minX, y: bounds.maxY - 1, width: dirtyRect.width, height: 1))

        if showRowNumbers {
            drawGutterHeader(into: cg)
        }
        // Only the columns the dirty rectangle reaches: with 500 columns, drawing them all on every
        // draw cost more than the rows under them.
        let visible = geometry.columns(in: dirtyRect.minX...max(dirtyRect.maxX, dirtyRect.minX))
        for display in visible where columns.indices.contains(display) {
            draw(column: columns[display], display: display, into: cg)
        }
        cg.restoreGState()
    }

    private func drawGutterHeader(into cg: CGContext) {
        // Centred vertically, not top-aligned: the `HStack` the old header was centred its children,
        // so the gutter's 25 pt box sits in the middle of the 50 pt band. Top-aligning it moved the
        // "#" twelve points up and its separator with it.
        let box = CGRect(x: 0, y: (bounds.height - paint.gutterBoxHeight) / 2,
                         width: geometry.gutter, height: paint.gutterBoxHeight)
        let line = line("#", font: paint.gutterFont, color: palette.inkDim)
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        cg.textPosition = CGPoint(x: box.maxX - GridMetrics.cellPadding - width,
                                  y: box.minY + GridMetrics.gutterVerticalPadding
                                      + CTFontGetAscent(paint.gutterFont as CTFont))
        CTLineDraw(line, cg)
        cg.textPosition = .zero
        cg.setFillColor(palette.inkFaint)
        cg.fill(CGRect(x: box.maxX - 1, y: box.minY, width: 1, height: box.height))
    }

    private func draw(column: Column, display: Int, into cg: CGContext) {
        let edges = geometry.edges(of: display)
        let box = CGRect(x: edges.left, y: 0, width: edges.right - edges.left, height: bounds.height)
        let content = box.insetBy(dx: GridMetrics.cellPadding, dy: 0)

        // The label line. Its width is what is left after the two marks that can follow it: the
        // rename dot and the sort chevron.
        var marks: CGFloat = 0
        if column.isRenamed { marks += 4 + GridMetrics.headerSpacing }
        if column.sortDirection != nil { marks += 8 + GridMetrics.headerSpacing }
        let labelWidth = max(0, content.width - marks)
        let label = lines?.line(column.title, font: paint.headerFont, color: palette.inkLabel,
                                width: labelWidth)
            ?? CTLineCreateWithAttributedString(NSAttributedString(string: column.title))
        let labelLineWidth = CGFloat(CTLineGetTypographicBounds(label, nil, nil, nil))
        let top = GridMetrics.headerVerticalPadding
        let labelY = top + CTFontGetAscent(paint.headerFont as CTFont)
        var x = column.isNumeric ? content.maxX - labelLineWidth - marks : content.minX
        cg.textPosition = CGPoint(x: x, y: labelY)
        CTLineDraw(label, cg)

        if column.isRenamed {
            cg.setFillColor(palette.accent.copy(alpha: 0.7) ?? palette.accent)
            cg.fillEllipse(in: CGRect(x: x + labelLineWidth + GridMetrics.headerSpacing,
                                      y: top + (paint.headerLabelLine - 4) / 2,
                                      width: 4, height: 4))
            x += 4 + GridMetrics.headerSpacing
        }
        if let direction = column.sortDirection {
            draw(symbol: direction == .ascending ? "chevron.up" : "chevron.down",
                 pointSize: 8, weight: .bold,
                 in: CGRect(x: x + labelLineWidth + GridMetrics.headerSpacing,
                            y: top + (paint.headerLabelLine - 8) / 2,
                            width: 8, height: 8),
                 tint: palette.accent, into: cg)
        }

        // The type chip, under the label. Wrapped rather than cut, exactly as the `Chip` view the
        // header used to be: a `character varying` in a narrow column makes two lines, and the
        // header row grows to fit them.
        drawChip(column.type, in: CGRect(x: content.minX,
                                          y: top + paint.headerLabelLine + GridMetrics.headerSpacing,
                                          width: content.width,
                                          height: bounds.height - top - paint.headerLabelLine
                                              - GridMetrics.headerSpacing - GridMetrics.headerVerticalPadding),
                 alignedRight: column.isNumeric,
                 into: cg)

        // The funnel, at the box's top-right corner with 6 in from the right and 4 from the top —
        // its hit area is the glyph's own bounds, as it has always been.
        let glyph = Self.funnelBounds(content: content)
        draw(symbol: column.isFiltered ? "line.3.horizontal.decrease.circle.fill"
                                       : "line.3.horizontal.decrease.circle",
             pointSize: 10, weight: .regular,
             in: glyph,
             tint: column.isFiltered ? palette.accent : palette.inkNull, into: cg)

        cg.setFillColor(palette.inkFaint)
        cg.fill(CGRect(x: box.maxX - 1, y: 0, width: 1, height: bounds.height))

        cg.textPosition = .zero
    }

    private func drawChip(_ text: String, in rect: CGRect, alignedRight: Bool, into cg: CGContext) {
        let chipFont = paint.chipFont
        // Wrapped at the content width, which is what the SwiftUI `Text` inside a `frame(width:)`
        // did — the chip's own box then hugs whatever the text actually needs.
        let chips = GridChipText.lines(text, font: chipFont, width: rect.width)
        guard !chips.isEmpty else { return }
        let widest = chips.map { CGFloat(CTLineGetTypographicBounds($0, nil, nil, nil)) }.max() ?? 0
        let chipHeight = CGFloat(chips.count) * paint.chipLine + 2 * GridMetrics.chipPadding
        let width = min(widest + 2 * GridMetrics.chipHorizontalPadding, rect.width)
        let x = alignedRight ? rect.maxX - width : rect.minX
        let box = CGRect(x: x, y: rect.minY, width: width, height: chipHeight)
        let path = CGPath(roundedRect: box, cornerWidth: box.height / 2, cornerHeight: box.height / 2,
                          transform: nil)
        cg.setFillColor(palette.typeTint(text).copy(alpha: 0.14) ?? palette.ice)
        cg.addPath(path)
        cg.fillPath()
        cg.setFillColor(palette.typeTint(text))
        for (index, line) in chips.enumerated() {
            let lineWidth = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
            cg.textPosition = CGPoint(x: box.midX - lineWidth / 2,
                                      y: box.minY + GridMetrics.chipPadding
                                          + CGFloat(index) * paint.chipLine
                                          + CTFontGetAscent(chipFont as CTFont))
            CTLineDraw(line, cg)
        }
        cg.textPosition = .zero
    }

    /// Draw an SF Symbol in the tint the header wants.
    ///
    /// Two earlier attempts lost the glyph and are recorded so they are not tried again.
    /// `cg.clip(to:mask:)` on the symbol's own `CGImage` wants a **grayscale** mask (`DeviceGray`,
    /// no alpha); a symbol's image is RGBA, so the clip painted nothing and every chevron and funnel
    /// vanished. Rasterising the symbol into a small bitmap and filling that `.sourceIn` kept the
    /// antialiased edge's partial alpha, so a stroke that peaked at `#70D0ED` came out `#599FB8`.
    /// `SymbolConfiguration(paletteColors:)` returns a non-template image already holding the paint,
    /// which is exactly what `foregroundStyle` gave the SwiftUI `Image(systemName:)` this replaces.
    private func draw(symbol name: String, pointSize: CGFloat, weight: NSFont.Weight,
                      in rect: CGRect, tint: CGColor, into cg: CGContext) {
        guard rect.width >= 1, rect.height >= 1,
              let color = NSColor(cgColor: tint),
              let image = Self.symbolImage(name, pointSize: pointSize, weight: weight,
                                           color: color) else { return }
        cg.saveGState()
        // The view is flipped and `NSImage.draw(in:)` is not, so mirror about the rect's own centre;
        // drawing in the same coordinates after that keeps the glyph upright.
        cg.translateBy(x: 0, y: rect.midY * 2)
        cg.scaleBy(x: 1, y: -1)
        let graphics = NSGraphicsContext(cgContext: cg, flipped: false)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = graphics
        // Centred in the box rather than stretched to it: the symbol keeps its own aspect, which is
        // what `Image(systemName:)` did inside its frame.
        let size = image.size
        image.draw(in: CGRect(x: rect.midX - size.width / 2, y: rect.midY - size.height / 2,
                              width: size.width, height: size.height))
        NSGraphicsContext.restoreGraphicsState()
        cg.restoreGState()
    }

    /// The symbol at a point size and weight, coloured, cached.
    ///
    /// `pointSize`/`weight` is what `.font(.system(size:weight:))` on an `Image(systemName:)`
    /// resolved to, and it is the closest AppKit equivalent; sizing by the image's own `size` was
    /// tried and is a different glyph size, not a resample of the same one.
    private static func symbolImage(_ name: String, pointSize: CGFloat, weight: NSFont.Weight,
                                    color: NSColor) -> NSImage? {
        let key = "\(name)|\(Int(pointSize * 100))|\(weight.rawValue)|\(color.hashValue)"
        if let cached = symbolCache[key] { return cached }
        let configuration = NSImage.SymbolConfiguration(pointSize: pointSize, weight: weight)
            .applying(.init(paletteColors: [color]))
        guard let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(configuration) else { return nil }
        symbolCache[key] = image
        return image
    }

    private static var symbolCache: [String: NSImage] = [:]

    private func line(_ text: String, font: NSFont, color: CGColor) -> CTLine {
        lines?.line(text, font: font, color: color)
            ?? CTLineCreateWithAttributedString(NSAttributedString(string: text))
    }

    // MARK: Hit testing

    /// The funnel's own bounds, which are also its hit area — 10 pt of glyph with a point of slack,
    /// the same target the SwiftUI button had. W10-T2 is what widens it.
    private static func funnelBounds(content: CGRect) -> CGRect {
        CGRect(x: content.maxX - 10, y: 4, width: 10, height: 10)
    }

    override func mouseDown(with event: NSEvent) {
        // A click on the funnel opens the filter; anywhere else on the header runs the sort cycle.
        // No waiting for mouse-up: a header click has always sorted on press.
        let point = convert(event.locationInWindow, from: nil)
        guard let display = display(at: point) else { return }
        let column = columns[display]
        let content = CGRect(x: geometry.edges(of: display).left + GridMetrics.cellPadding, y: 0,
                             width: column.width - 2 * GridMetrics.cellPadding, height: bounds.height)
        if Self.funnelBounds(content: content).contains(point) {
            let edges = geometry.edges(of: display)
            onFilterClick?(column.source,
                           CGRect(x: edges.left + content.width - 10, y: bounds.height - 4 - 10,
                                  width: 10, height: 10))
            return
        }
        commands?.sortClick(column.source)
    }

    /// The drawn column a point falls in, or `nil` in the gutter or past the last column.
    private func display(at point: CGPoint) -> Int? {
        geometry.column(atX: point.x)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let point = convert(event.locationInWindow, from: nil)
        guard let display = display(at: point) else { return nil }
        return columnMenu?(columns[display].source, display, resultTruncated)
    }

    // MARK: Tooltips

    /// The band's tooltip region, rebuilt on the table's schedule rather than its own (§8.5 wants
    /// one controller for the header and the body, rebuilt 100 ms after the pointer stops moving).
    ///
    /// One rectangle over the whole band instead of one per column: which column the pointer is
    /// over is known when the string is asked for, so per-column registration would be work done
    /// ahead of hovers that may never come — and the band scrolls horizontally, which would make
    /// every one of those rectangles stale.
    func scheduleTooltips() {
        removeAllToolTips()
        tooltipWindowRect = nil
        guard bounds.width > 1, bounds.height > 1 else { return }
        addToolTip(bounds, owner: self, userData: nil)
        tooltipWindowRect = convert(bounds, to: nil)
    }

    /// "Sort by …" over a column, "Filter this column" / "Filtered by …" over its funnel — the two
    /// strings the SwiftUI header had, which is what §13 item 25 asks the test to lock.
    ///
    /// The sort wording is the old one and not the blueprint's §12.4 phrasing, deliberately: a
    /// header click routes **server-first** and only falls back to the rows in hand
    /// (`AppModel.setSort`), so "over the rows already fetched, not the whole result" would
    /// describe only the fallback path and deny the common one.
    ///
    /// `override` because `NSView` already carries the owner method through
    /// `NSObject(NSToolTipOwner)`, and so no explicit `NSViewToolTipOwner` conformance either.
    override func view(_ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint,
                       userData: UnsafeMutableRawPointer?) -> String {
        let local = GridToolTip.local(point: point, in: self, registered: tooltipWindowRect)
        guard bounds.contains(local), let display = display(at: local),
              columns.indices.contains(display)
        else { return "" }
        let column = columns[display]
        let content = CGRect(x: geometry.edges(of: display).left + GridMetrics.cellPadding, y: 0,
                             width: column.width - 2 * GridMetrics.cellPadding, height: bounds.height)
        if Self.funnelBounds(content: content).contains(local) {
            return column.isFiltered ? "Filtered by \(column.filterLabel ?? "")" : "Filter this column"
        }
        return "Sort by \(column.title) — on the server when possible, over the rows fetched otherwise"
    }

    /// The header's height for a set of columns: the label line, the chip block, and the padding.
    ///
    /// Measured rather than assumed to be 50, because a chip that wraps makes its column taller and
    /// the band has to hold the tallest of them, which is what the SwiftUI `HStack` did.
    static func measuredHeight(columns: [Column]) -> CGFloat {
        let labelLine = GridMetrics.lineHeight(of: FontChoice.codeNSFont(size: 12, weight: .semibold))
        let chipLine = GridMetrics.lineHeight(of: FontChoice.codeNSFont(size: 11, weight: .semibold))
        let tallest = columns.map { column -> CGFloat in
            let width = column.width - 2 * GridMetrics.cellPadding
            let lines = max(1, GridChipText.lineCount(column.type, font: FontChoice.codeNSFont(size: 11, weight: .semibold),
                                                      width: width))
            return CGFloat(lines) * chipLine + 2 * GridMetrics.chipPadding
        }.max() ?? 0
        return GridMetrics.headerVerticalPadding + labelLine + GridMetrics.headerSpacing
            + max(tallest, chipLine + 2 * GridMetrics.chipPadding) + GridMetrics.headerVerticalPadding
    }
}

/// How a chip's text breaks into lines in a column.
///
/// The chip is a `Text` with no line limit, so a type name wider than its column is *wrapped*, not
/// cut. The header's height depends on that, so the two answers — how many lines, and what they say
/// — come from one place, and both agree with the body's.
enum GridChipText {
    /// The lines the text breaks into inside `width`, using CoreText's own line breaking so a word
    /// that does not fit lands on the next line rather than being cut in half.
    static func lines(_ text: String, font: NSFont, width: CGFloat) -> [CTLine] {
        guard !text.isEmpty, width > 1 else { return [] }
        // `kCTForegroundColorFromContextAttributeName` rather than a baked colour: the chip's tint
        // is set on the context right before the line is drawn, and a `CTLine` that carried its own
        // colour would ignore it and draw black — which is what a chip that is measured by its
        // saturation would then fail on.
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true,
        ]
        let attributed = NSAttributedString(string: text, attributes: attributes)
        let typesetter = CTTypesetterCreateWithAttributedString(attributed)
        var result: [CTLine] = []
        var start = 0
        let length = attributed.length
        while start < length {
            let count = CTTypesetterSuggestLineBreak(typesetter, start, Double(width))
            guard count > 0 else { break }
            result.append(CTTypesetterCreateLine(typesetter, CFRangeMake(start, count)))
            start += count
        }
        // One unbreakable word wider than the column still has to be drawn, or a long type name in a
        // narrow column would show an empty chip.
        return result.isEmpty ? [CTLineCreateWithAttributedString(attributed)] : result
    }

    static func lineCount(_ text: String, font: NSFont, width: CGFloat) -> Int {
        lines(text, font: font, width: width).count
    }
}

extension GridPalette {
    /// The tint of a column type's chip: numeric mint, boolean violet, date or time amber, and
    /// everything else ice — the same four-way vocabulary the SwiftUI `typeTint` used.
    func typeTint(_ type: String) -> CGColor {
        if GridMetrics.isNumeric(type: type) { return mint }
        let lowered = type.lowercased()
        if lowered.contains("bool") { return violet }
        if lowered.contains("date") || lowered.contains("time") { return orange }
        return ice
    }
}
