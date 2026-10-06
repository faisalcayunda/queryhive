import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The text the palette does not claim — identifiers, dotted names — must stay readable on the
/// editor canvas. Keywords and numbers carry a temporary colour from the analysis; everything else
/// takes whatever the storage carries, and a storage with no `.foregroundColor` is drawn black by
/// AppKit, which is invisible on a dark canvas (the owner's screenshot: coloured keywords over
/// near-black identifiers).
///
/// Each test asks what a character is *drawn* in: the layout manager's temporary colour if it has
/// one, else the storage's, else AppKit's black — resolved in the text view's own appearance and
/// held against the canvas of the pinned theme.
@MainActor
final class EditorTextColourTests: XCTestCase {
    private static let sql = """
    SELECT kode_wilayah, nama
    FROM hive.analytics.penerima_manfaat
    WHERE tahun = 2026
    ORDER BY nama
    """
    /// Words that no palette class claims, and the keyword and number that prove a paint has run.
    private static let identifiers = ["kode_wilayah", "nama", "hive", "analytics", "penerima_manfaat", "tahun"]

    private var windows: [NSWindow] = []
    private var savedTheme: (dark: AppTheme, light: AppTheme, accent: AccentChoice, tone: SurfaceTone,
                             mode: AppearanceMode, systemIsDark: Bool, ui: String, code: String)!
    private var savedFontSize = 0.0

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        let store = ThemeStore.shared
        savedTheme = (store.darkTheme, store.lightTheme, store.accent, store.tone, store.mode,
                      store.systemIsDark, store.uiFontFamily, store.codeFontFamily)
        savedFontSize = EditorPreferences.shared.fontSize
    }

    override func tearDown() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
        EditorPreferences.shared.fontSize = savedFontSize
        ThemeStore.shared.unpinSurface()
        let s = savedTheme!
        ThemeStore.shared.pin(theme: s.dark, accent: s.accent, tone: s.tone, mode: s.mode,
                              systemIsDark: s.systemIsDark, uiFont: s.ui, codeFont: s.code)
        ThemeStore.shared.pin(theme: s.light)
        super.tearDown()
    }

    // MARK: Cases

    func testIdentifiersAreReadableInDarkOnLoad() throws {
        let (textView, _) = try hostEditor(dark: true)
        assertReadable(textView, "dark, on load")
    }

    /// A control: the light canvas never hid the bug, because AppKit's black reads on it. This one
    /// must stay green on the way to the fix.
    func testIdentifiersAreReadableInLightOnLoad() throws {
        let (textView, _) = try hostEditor(dark: false)
        assertReadable(textView, "light, on load")
    }

    /// A colour resolved once, in whichever appearance was current, would read in one direction
    /// only. The text has to follow the window both ways without being rebuilt.
    func testTheTextFollowsTheAppearanceAtRuntime() throws {
        let (textView, window) = try hostEditor(dark: false)
        assertReadable(textView, "light, before the switch")
        switchAppearance(dark: true, window: window)
        assertReadable(textView, "dark, after light -> dark")
        switchAppearance(dark: false, window: window)
        assertReadable(textView, "light, after dark -> light")
    }

    func testIdentifiersAreReadableAfterAFontSizeChange() throws {
        let (textView, _) = try hostEditor(dark: true)
        let before = textView.font?.pointSize ?? 0
        EditorPreferences.shared.fontSize = before + 4
        pump(until: { (textView.font?.pointSize ?? 0) > before }, "the font size never reached the editor")
        // The size change repaints the text from the analysis.
        pump(for: 0.5)
        assertReadable(textView, "dark, after a font-size change")
    }

    func testTypedTextIsReadable() throws {
        let (textView, _) = try hostEditor(dark: true)
        let end = (textView.string as NSString).length
        textView.setSelectedRange(NSRange(location: end, length: 0))
        textView.insertText("\nzz_typed", replacementRange: textView.selectedRange())
        let typed = (textView.string as NSString).range(of: "zz_typed")
        XCTAssertNotEqual(typed.location, NSNotFound)
        // At once, before the analysis has had a turn: this is the typing attributes.
        assertReadable(textView, "dark, just typed", words: ["zz_typed"])
        pump(for: 0.5)
        assertReadable(textView, "dark, typed then painted", words: ["zz_typed"])
    }

    // MARK: Hosting

    private func pin(dark: Bool) {
        ThemeStore.shared.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: dark ? .midnight : .daylight, accent: .ice, tone: .glow,
                              mode: dark ? .dark : .light, systemIsDark: dark, uiFont: "", codeFont: "")
    }

    private func appearance(dark: Bool) throws -> NSAppearance {
        try XCTUnwrap(NSAppearance(named: dark ? .darkAqua : .aqua))
    }

    /// The real editor — `EditorPane`, so `makeNSView` and the coordinator — in a window of the
    /// given appearance, painted once.
    private func hostEditor(dark: Bool) throws -> (SQLTextView, NSWindow) {
        pin(dark: dark)
        let model = Snapshot.seeded(scene: "syntax")
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = Self.sql
        tab.caret = 0
        tab.selection = NSRange(location: 0, length: 0)
        let size = CGSize(width: 800, height: 300)
        let host = NSHostingView(rootView: AnyView(EditorPane(tab: tab).environment(model)
            .frame(width: size.width, height: size.height).background(Tone.canvas)))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.appearance = try appearance(dark: dark)
        window.contentView = host
        windows.append(window)
        // SwiftUI builds the representable on its first layout pass, not on assignment.
        pump(until: { Self.findTextView(in: host) != nil }, "the editor was never built")
        let textView = try XCTUnwrap(Self.findTextView(in: host), "no SQL text view")
        pumpUntilPainted(textView)
        return (textView, window)
    }

    private func switchAppearance(dark: Bool, window: NSWindow) {
        pin(dark: dark)
        window.appearance = try? appearance(dark: dark)
        window.contentView?.needsDisplay = true
        pump(for: 0.3)
    }

    private static func findTextView(in view: NSView) -> SQLTextView? {
        if let match = view as? SQLTextView { return match }
        for subview in view.subviews { if let found = findTextView(in: subview) { return found } }
        return nil
    }

    // MARK: Waiting

    private func pump(for seconds: TimeInterval) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            windows.forEach { $0.contentView?.layoutSubtreeIfNeeded() }
        }
    }

    private func pump(until condition: () -> Bool, _ message: String,
                      file: StaticString = #filePath, line: UInt = #line) {
        let deadline = Date().addingTimeInterval(10)
        while !condition(), Date() < deadline { pump(for: 0.03) }
        XCTAssertTrue(condition(), message, file: file, line: line)
    }

    /// A keyword is coloured only once the analysis has painted, so that is the "loaded" signal.
    private func pumpUntilPainted(_ textView: SQLTextView) {
        pump(until: { textView.layoutManager?.temporaryAttribute(
            .foregroundColor, atCharacterIndex: 0, effectiveRange: nil) != nil },
             "the editor never painted its first keyword")
    }

    // MARK: Colour

    private func assertReadable(_ textView: SQLTextView, _ label: String, words: [String] = identifiers,
                                file: StaticString = #filePath, line: UInt = #line) {
        // The canvas the text is drawn over, resolved for the appearance the view is in.
        var canvas = RGB(red: 0, green: 0, blue: 0)
        textView.effectiveAppearance.performAsCurrentDrawingAppearance {
            canvas = RGB(NSColor(ThemeStore.shared.theme.canvas)) ?? canvas
        }
        let text = textView.string as NSString
        for word in words {
            let at = text.range(of: word).location
            guard at != NSNotFound else {
                XCTFail("[\(label)] \"\(word)\" is not in the text", file: file, line: line)
                continue
            }
            var drawn = RGB(red: 0, green: 0, blue: 0)
            textView.effectiveAppearance.performAsCurrentDrawingAppearance {
                drawn = (RGB(Self.effectiveColour(of: textView, at: at)) ?? drawn).over(canvas)
            }
            let ratio = drawn.contrast(with: canvas)
            XCTAssertGreaterThanOrEqual(
                ratio, 4.5, String(format: "[%@] \"%@\" is drawn at %.2f:1 on the canvas (%@ on %@)",
                                   label, word, ratio, drawn.description, canvas.description),
                file: file, line: line)
        }
    }

    /// What the layout manager draws the character in: a temporary colour wins over the storage's,
    /// and text with neither is drawn black.
    private static func effectiveColour(of textView: NSTextView, at index: Int) -> NSColor {
        if let temporary = textView.layoutManager?.temporaryAttribute(
            .foregroundColor, atCharacterIndex: index, effectiveRange: nil) as? NSColor { return temporary }
        if let stored = textView.textStorage?.attribute(
            .foregroundColor, at: index, effectiveRange: nil) as? NSColor { return stored }
        return .black
    }

    private struct RGB: CustomStringConvertible {
        var red: Double, green: Double, blue: Double, alpha = 1.0
        init(red: Double, green: Double, blue: Double, alpha: Double = 1) {
            (self.red, self.green, self.blue, self.alpha) = (red, green, blue, alpha)
        }
        /// Must be called inside the appearance the colour is meant for.
        init?(_ color: NSColor) {
            guard let c = color.usingColorSpace(.sRGB) else { return nil }
            self.init(red: Double(c.redComponent), green: Double(c.greenComponent),
                      blue: Double(c.blueComponent), alpha: Double(c.alphaComponent))
        }
        func over(_ background: RGB) -> RGB {
            RGB(red: red * alpha + background.red * (1 - alpha),
                green: green * alpha + background.green * (1 - alpha),
                blue: blue * alpha + background.blue * (1 - alpha))
        }
        /// WCAG 2.x relative luminance and contrast.
        private var luminance: Double {
            func linear(_ c: Double) -> Double { c <= 0.03928 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4) }
            return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
        }
        func contrast(with other: RGB) -> Double {
            (max(luminance, other.luminance) + 0.05) / (min(luminance, other.luminance) + 0.05)
        }
        var description: String { String(format: "#%02X%02X%02X", Int((red * 255).rounded()),
                                         Int((green * 255).rounded()), Int((blue * 255).rounded())) }
    }
}
