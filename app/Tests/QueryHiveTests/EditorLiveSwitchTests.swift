import AppKit
import XCTest

@testable import QueryHive

/// A switch in Settings > Editor flipped while a query is on screen, with word wrap on.
///
/// 0.2.2 shipped an editor that drew no text at all after "Show invisible characters" was turned on —
/// and after any other editor switch: the characters were still there, the gutter still numbered
/// them, and nothing was painted. Every switch re-ran the wrap pass, which set the text container to
/// 0 pt wide and relied on the text view's next frame change to widen it again; a switch changes no
/// frame, so the container stayed 0 pt wide. The app (built against SDK 14) lays a 0 pt container out
/// as nothing; this test process (SDK 27) lays it out as unbounded, which keeps the text but stops it
/// wrapping. So the test asserts the wrap: the container is as wide as the view and a long line
/// still breaks.
@MainActor
final class EditorLiveSwitchTests: XCTestCase {
    private var host: EditorTestHost?
    private static let longLine = "SELECT " + (1...24).map { "kolom_nomor_\($0)" }.joined(separator: ", ") + " FROM t"
    private static let sql = "SELECT\tkode_wilayah,   nama  \n\tFROM hive.analytics.penerima_manfaat  \n"
        + longLine + "\nWHERE tahun = 2026\t;\n"

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    override func tearDown() {
        host?.close()
        host = nil
        super.tearDown()
    }

    func testTurningInvisiblesOnAndOffKeepsTheTextWrappedToTheView() throws {
        let host = try makeHost(EditorLayout.standard)
        try assertWrapped(host, "before any switch")

        EditorPreferences.shared.showInvisibles = true
        XCTAssertTrue(host.pump(until: { host.textView.layoutManager?.showsInvisibleCharacters == true }),
                      "the switch reached the editor")
        host.pump(for: 0.2)
        XCTAssertEqual(host.textView.string, Self.sql, "the text itself is untouched")
        try assertWrapped(host, "invisibles on")

        EditorPreferences.shared.showInvisibles = false
        XCTAssertTrue(host.pump(until: { host.textView.layoutManager?.showsInvisibleCharacters == false }))
        host.pump(for: 0.2)
        try assertWrapped(host, "invisibles off again")
    }

    /// Not only invisibles: the wrap pass ran for every switch.
    func testAnotherEditorSwitchKeepsTheTextWrappedToTheView() throws {
        let host = try makeHost(EditorLayout.standard)
        EditorPreferences.shared.highlightCurrentLine = false
        XCTAssertTrue(host.pump(until: { host.coordinator.parent.layout.highlightCurrentLine == false }))
        host.pump(for: 0.2)
        try assertWrapped(host, "current-line band off")
    }

    /// Word wrap itself turned back on: the text view was free to grow sideways while it was off.
    func testTurningWordWrapBackOnWrapsToTheView() throws {
        var noWrap = EditorLayout.standard
        noWrap.wordWrap = false
        let host = try makeHost(noWrap)
        XCTAssertEqual(fragments(host, of: Self.longLine), 1, "with wrap off the long line is one fragment")

        EditorPreferences.shared.wordWrap = true
        XCTAssertTrue(host.pump(until: { host.textView.textContainer?.widthTracksTextView == true }))
        host.pump(for: 0.2)
        try assertWrapped(host, "wrap back on")
    }

    // MARK: Helpers

    private func makeHost(_ layout: EditorLayout) throws -> EditorTestHost {
        let host = try EditorTestHost(sql: Self.sql, layout: layout)
        self.host = host
        try host.settle()
        return host
    }

    private func assertWrapped(_ host: EditorTestHost, _ when: String,
                               file: StaticString = #filePath, line: UInt = #line) throws {
        let textView = host.textView
        let container = try XCTUnwrap(textView.textContainer)
        // The clip view reaches under the gutter, so what is visible of the text view runs from its
        // left edge (x = 0, nothing scrolled sideways) to the visible rect's right edge.
        let visible = host.scrollView.documentVisibleRect
        XCTAssertLessThanOrEqual(visible.minX, 0.5, "\(when): not scrolled sideways", file: file, line: line)
        XCTAssertEqual(textView.frame.width, visible.maxX, accuracy: 0.5,
                       "\(when): the text view is as wide as the view", file: file, line: line)
        XCTAssertEqual(container.size.width, textView.frame.width - 2 * textView.textContainerInset.width, accuracy: 0.5,
                       "\(when): the container is as wide as the text view's inside", file: file, line: line)
        XCTAssertGreaterThan(fragments(host, of: Self.longLine), 1,
                             "\(when): a line wider than the view wraps", file: file, line: line)
        XCTAssertGreaterThan(try ink(host), 500, "\(when): the text is drawn", file: file, line: line)
    }

    /// How many line fragments the first occurrence of `text` is laid out in.
    private func fragments(_ host: EditorTestHost, of text: String) -> Int {
        guard let layoutManager = host.textView.layoutManager else { return 0 }
        let characters = (host.textView.string as NSString).range(of: text)
        let glyphs = layoutManager.glyphRange(forCharacterRange: characters, actualCharacterRange: nil)
        var count = 0
        layoutManager.enumerateLineFragments(forGlyphRange: glyphs) { _, _, _, _, _ in count += 1 }
        return count
    }

    /// Pixels in the first three lines of the text area that stand off the canvas: text or markers.
    private func ink(_ host: EditorTestHost) throws -> Int {
        let rep = try host.bitmap()
        let size = host.view.bounds.size
        let visible = host.scrollView.convert(host.scrollView.bounds, to: host.view)
        let gutter = host.scrollView.rulersVisible ? host.ruler.ruleThickness : 0
        let lines = CGRect(x: visible.minX + gutter + 4, y: visible.minY + 2,
                           width: min(600, visible.width - gutter - 8), height: 50)
        var luma: [Double] = []
        var y = lines.minY
        while y < lines.maxY {
            var x = lines.minX
            while x < lines.maxX {
                luma.append(NSBitmapImageRep.luma(rep.rgb(atX: x, y: y, viewSize: size)))
                x += 0.5
            }
            y += 0.5
        }
        let median = luma.sorted()[luma.count / 2]
        return luma.filter { abs($0 - median) > 40 }.count
    }
}
