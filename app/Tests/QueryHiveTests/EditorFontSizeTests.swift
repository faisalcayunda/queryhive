import AppKit
import XCTest

@testable import QueryHive

/// ⌘+, ⌘− and ⌘0 (FR-ED-07, D-16): the region that has the keyboard decides whose size moves, the
/// size stays inside its range, and the editor keeps the line at the top of the view.
@MainActor
final class EditorFontSizeTests: XCTestCase {
    private var host: EditorTestHost?
    private var savedEditorSize = 0.0
    private var savedGridSize = 0

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        savedEditorSize = EditorPreferences.shared.fontSize
        savedGridSize = DataPreferences.shared.gridFontSize
    }

    override func tearDown() {
        host?.close()
        host = nil
        EditorPreferences.shared.fontSize = savedEditorSize
        DataPreferences.shared.gridFontSize = savedGridSize
        super.tearDown()
    }

    func testTheEditorsSizeIsHeldInsideItsRange() {
        XCTAssertEqual(EditorPreferences.clampedFontSize(3), 10)
        XCTAssertEqual(EditorPreferences.clampedFontSize(99), 28)
        XCTAssertEqual(EditorPreferences.clampedFontSize(.nan), EditorPreferences.standardFontSize)
        XCTAssertEqual(EditorPreferences.standardFontSize, 12.5)
    }

    func testTheEditorGrowsAndShrinksByOnePointAndResetsToTheDefault() {
        let model = AppModel(persistsSession: false)
        EditorPreferences.shared.fontSize = 12.5
        model.adjustFontSize(.bigger, in: .editor)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 13.5)
        model.adjustFontSize(.smaller, in: .editor)
        model.adjustFontSize(.smaller, in: .editor)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 11.5)
        model.adjustFontSize(.reset, in: .editor)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 12.5)
        XCTAssertEqual(Announcer.last, "Editor text size 12.5 points")
    }

    func testTheKeysStopAtTheEndsAndSaySo() {
        let model = AppModel(persistsSession: false)
        EditorPreferences.shared.fontSize = 28
        XCTAssertFalse(model.fontSizeChanges(.bigger, in: .editor), "⌘+ at the top of the range does nothing")
        XCTAssertTrue(model.fontSizeChanges(.smaller, in: .editor))
        model.adjustFontSize(.bigger, in: .editor)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 28)
        EditorPreferences.shared.fontSize = 10
        XCTAssertFalse(model.fontSizeChanges(.smaller, in: .editor))
        XCTAssertTrue(model.fontSizeChanges(.reset, in: .editor), "Actual Size is enabled away from 12.5")
    }

    /// The grid has its own size, in whole points, 11 to 16, 12 by default. The editor's is untouched.
    func testTheResultsRegionMovesTheGridsSizeAndNotTheEditors() {
        let model = AppModel(persistsSession: false)
        DataPreferences.shared.gridFontSize = 12
        EditorPreferences.shared.fontSize = 12.5
        model.adjustFontSize(.bigger, in: .results)
        XCTAssertEqual(DataPreferences.shared.gridFontSize, 13)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 12.5)
        for _ in 0..<10 { model.adjustFontSize(.bigger, in: .results) }
        XCTAssertEqual(DataPreferences.shared.gridFontSize, 16)
        XCTAssertFalse(model.fontSizeChanges(.bigger, in: .results))
        model.adjustFontSize(.reset, in: .results)
        XCTAssertEqual(DataPreferences.shared.gridFontSize, 12)
        for _ in 0..<10 { model.adjustFontSize(.smaller, in: .results) }
        XCTAssertEqual(DataPreferences.shared.gridFontSize, 11)
    }

    func testTheSidebarAndAnUnfocusedWindowFallBackToTheEditor() {
        let model = AppModel(persistsSession: false)
        EditorPreferences.shared.fontSize = 12.5
        model.adjustFontSize(.bigger, in: .sidebar)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 13.5)
        model.adjustFontSize(.bigger)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 14.5, "no region with the keyboard is the editor's")
    }

    // MARK: Keys

    func testTheMenuHasTheThreeActionsWithTheirKeys() throws {
        for scheme in ShortcutScheme.allCases {
            let specs = AppMenu.specs(for: scheme)
            let titles = Dictionary(uniqueKeysWithValues: specs.map { ($0.id, ($0.title, $0.shortcut?.display)) })
            XCTAssertEqual(titles["fontBigger"]?.0, "Increase Font Size")
            XCTAssertEqual(titles["fontBigger"]?.1, "⌘+")
            XCTAssertEqual(titles["fontSmaller"]?.0, "Decrease Font Size")
            XCTAssertEqual(titles["fontSmaller"]?.1, "⌘-")
            XCTAssertEqual(titles["fontReset"]?.0, "Actual Size")
            XCTAssertEqual(titles["fontReset"]?.1, "⌘0")
        }
    }

    func testTheMenuItemsPerformAndGreyOutAtTheEnds() throws {
        let model = AppModel(persistsSession: false)
        let specs = AppMenu.specs(for: .queryhive)
        let bigger = try XCTUnwrap(specs.first { $0.id == "fontBigger" })
        EditorPreferences.shared.fontSize = 12.5
        XCTAssertTrue(AppMenu.isEnabled(bigger, in: model))
        AppMenu.perform(bigger, in: model)
        XCTAssertEqual(EditorPreferences.shared.fontSize, 13.5)
        EditorPreferences.shared.fontSize = 28
        XCTAssertFalse(AppMenu.isEnabled(bigger, in: model))
    }

    /// ⌘= is the key a US keyboard has for ⌘+; with or without the shift it is Increase.
    func testEqualsIsAnAliasOfPlusAndNothingElseIs() {
        func route(_ chars: String?, command: Bool = true, control: Bool = false, option: Bool = false) -> AppModel.FontStep? {
            FontKeyRouter.route(charactersIgnoringModifiers: chars, command: command, control: control, option: option)
        }
        XCTAssertEqual(route("="), .bigger)
        XCTAssertNil(route("=", command: false))
        XCTAssertNil(route("=", control: true))
        XCTAssertNil(route("=", option: true))
        XCTAssertNil(route("-"))
        XCTAssertNil(route("0"))
        XCTAssertNil(route(nil))
    }

    // MARK: In the editor

    private static let longDocument = (1...200).map { "SELECT \($0) AS line_no FROM hive.analytics.t_\($0 / 8)" }
        .joined(separator: "\n")

    /// The line at the top of the view is the line at the top afterwards, whatever the new size.
    func testTheTopLineStaysAtTheTopAcrossASizeChange() throws {
        EditorPreferences.shared.fontSize = 12.5
        let h = try EditorTestHost(sql: Self.longDocument, size: CGSize(width: 800, height: 320))
        host = h
        try h.settle()
        let tv = h.textView
        let layoutManager = try XCTUnwrap(tv.layoutManager), container = try XCTUnwrap(tv.textContainer)
        layoutManager.ensureLayout(for: container)
        let starts = SQLFolding.lineStarts(in: tv.string as NSString)
        let glyph = layoutManager.glyphIndexForCharacter(at: starts[90])
        let rect = layoutManager.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
        let clip = h.scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: rect.minY + tv.textContainerOrigin.y))
        h.scrollView.reflectScrolledClipView(clip)
        h.pump(for: 0.2)
        let before = try XCTUnwrap(h.coordinator.topVisibleCharacter())
        XCTAssertEqual(before, starts[90])

        EditorPreferences.shared.fontSize = 20.5
        XCTAssertTrue(h.pump(until: { abs((tv.font?.pointSize ?? 0) - 20.5) < 0.01 }), "the size never reached the editor")
        h.pump(for: 0.4)
        XCTAssertEqual(h.coordinator.topVisibleCharacter(), before, "the view slid away from the line it was on")

        EditorPreferences.shared.fontSize = 10
        XCTAssertTrue(h.pump(until: { abs((tv.font?.pointSize ?? 0) - 10) < 0.01 }))
        h.pump(for: 0.4)
        XCTAssertEqual(h.coordinator.topVisibleCharacter(), before)
    }

    /// A size change marks the whole text for a repaint, and the analysis still answers a window at a
    /// time: no more than the per-turn budget is handed back, in a document near the ceiling.
    func testASizeChangeOnATwoMegabyteDocumentPaintsOnlyAWindowWithinTheBudget() throws {
        let line = "SELECT 1, 'two', three FROM t;\n"
        let text = String(repeating: line, count: 1_900_000 / (line as NSString).length)
        let analysis = try EditorAnalysis(text: text)
        let whole = NSRange(location: 0, length: analysis.length)
        try analysis.markDirty(whole)
        let budget = 131_072
        let paint = try analysis.paint(window: NSRange(location: 0, length: 20_000), budget: budget)
        XCTAssertLessThanOrEqual(paint.ranges.reduce(0) { $0 + $1.length }, budget)
        XCTAssertTrue(paint.dirtyElsewhere, "the rest of the document is still owed a paint")
        XCTAssertFalse(paint.inactive)
    }

    // MARK: Base font after an outside load

    private func isItalic(_ font: Any?) -> Bool {
        (font as? NSFont)?.fontDescriptor.symbolicTraits.contains(.italic) ?? false
    }

    /// `NSTextView.font` answers with the font of character 0, and a text that starts with a painted
    /// comment has an italic character 0. An outside load must still come back upright: the base
    /// attributes carry the worn upright face, not whatever the old first character wore.
    func testAnOutsideLoadAfterAStartingCommentComesBackUpright() throws {
        let h = try EditorTestHost(sql: "-- first\nSELECT 1 FROM t")
        host = h
        try h.settle()
        let storage = try XCTUnwrap(h.textView.textStorage)
        XCTAssertTrue(isItalic(storage.attribute(.font, at: 0, effectiveRange: nil)),
                      "the fixture must have a painted, italic comment at index 0")

        let next = "SELECT 2 FROM u WHERE x = 1"
        h.tab.sql = next
        XCTAssertTrue(h.pump(until: { storage.string == next }), "the load never reached the editor")
        let at = (next as NSString).range(of: "FROM").location
        XCTAssertFalse(isItalic(storage.attribute(.font, at: 0, effectiveRange: nil)))
        XCTAssertFalse(isItalic(storage.attribute(.font, at: at, effectiveRange: nil)))
        XCTAssertFalse(isItalic(h.textView.typingAttributes[.font]), "typing would continue in italics")
        try h.settle()
        XCTAssertFalse(isItalic(storage.attribute(.font, at: at, effectiveRange: nil)))
    }

    /// Past the analysis ceiling nothing paints, so a wrong base font would stay for good.
    func testAnOutsideLoadPastTheCeilingAfterAStartingCommentStaysUpright() throws {
        let h = try EditorTestHost(sql: "-- first\nSELECT 1 FROM t")
        host = h
        try h.settle()
        let storage = try XCTUnwrap(h.textView.textStorage)
        XCTAssertTrue(isItalic(storage.attribute(.font, at: 0, effectiveRange: nil)))

        let ceiling = try editorCeiling()
        let big = String(repeating: "SELECT 1;\n", count: ceiling / 10 + 1)
        let length = (big as NSString).length
        XCTAssertGreaterThan(length, ceiling)
        h.tab.sql = big
        XCTAssertTrue(h.pump(until: { storage.length == length }, timeout: 30), "the load never reached the editor")
        h.pump(for: 0.3)
        XCTAssertEqual(storage.length, length)
        for index in [0, length / 2, length - 1] {
            XCTAssertFalse(isItalic(storage.attribute(.font, at: index, effectiveRange: nil)), "offset \(index)")
        }
        XCTAssertFalse(isItalic(h.textView.typingAttributes[.font]))
    }

    /// The base font follows the size the editor wears, not the setting a paint may have read early.
    func testTheBaseFontKeepsTheWornSizeAfterAnOutsideLoad() throws {
        let h = try EditorTestHost(sql: "-- first\nSELECT 1")
        host = h
        EditorPreferences.shared.fontSize = 17
        XCTAssertTrue(h.pump(until: { abs((h.textView.font?.pointSize ?? 0) - 17) < 0.01 }))
        h.tab.sql = "SELECT 2"
        XCTAssertTrue(h.pump(until: { h.textView.string == "SELECT 2" }))
        let font = h.textView.textStorage?.attribute(.font, at: 0, effectiveRange: nil) as? NSFont
        XCTAssertEqual(font?.pointSize ?? 0, 17, accuracy: 0.01)
        XCTAssertFalse(isItalic(font))
    }
}
