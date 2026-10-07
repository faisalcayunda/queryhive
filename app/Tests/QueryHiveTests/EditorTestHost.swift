import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// A real `EditorPane` in a window, for the tests that need the editor as the app builds it: the
/// text view, its coordinator, the gutter and the scroll view, with the theme pinned and the
/// editor's switches restored afterwards. One per test, closed by `close()`.
@MainActor
final class EditorTestHost {
    let model: AppModel
    let tab: QueryTab
    let window: NSWindow
    let view: NSHostingView<AnyView>
    let textView: SQLTextView
    let coordinator: SQLEditor.Coordinator

    private let savedEditor = EditorLayout.current
    private let savedTheme: (dark: AppTheme, light: AppTheme, accent: AccentChoice, tone: SurfaceTone,
                             mode: AppearanceMode, systemIsDark: Bool, ui: String, code: String)

    var scrollView: NSScrollView { textView.enclosingScrollView! }
    var ruler: LineNumberRulerView { coordinator.ruler! }

    /// `layout` is what the editor's switches are set to before it is built; `theme` pins the canvas.
    init(sql: String, layout: EditorLayout = .standard, theme: AppTheme = .midnight,
         size: CGSize = CGSize(width: 800, height: 320), caret: Int = 0) throws {
        let store = ThemeStore.shared
        savedTheme = (store.darkTheme, store.lightTheme, store.accent, store.tone, store.mode,
                      store.systemIsDark, store.uiFontFamily, store.codeFontFamily)
        Self.apply(layout)
        store.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        store.pin(theme: theme, accent: .ice, tone: .glow, mode: theme.isDark ? .dark : .light,
                  systemIsDark: theme.isDark, uiFont: "", codeFont: "")
        model = Snapshot.seeded(scene: "syntax")
        tab = try XCTUnwrap(model.selectedTab)
        tab.sql = sql
        tab.caret = caret
        tab.selection = NSRange(location: caret, length: 0)
        view = NSHostingView(rootView: AnyView(EditorPane(tab: tab).environment(model)
            .frame(width: size.width, height: size.height).background(Tone.canvas)))
        view.frame = CGRect(origin: .zero, size: size)
        window = NSWindow(contentRect: view.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: theme.isDark ? .darkAqua : .aqua)
        window.contentView = view
        // SwiftUI builds the representable on its first layout pass, not on assignment.
        let deadline = Date().addingTimeInterval(10)
        var found: SQLTextView?
        while found == nil, Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            view.layoutSubtreeIfNeeded()
            found = Self.findTextView(in: view)
        }
        textView = try XCTUnwrap(found, "the editor was never built")
        coordinator = try XCTUnwrap(textView.delegate as? SQLEditor.Coordinator)
        pump(for: 0.2)
    }

    /// Restore what the test changed. Called from `tearDown`, so a failed assertion cannot leak it.
    func close() {
        window.isReleasedWhenClosed = false
        window.contentView = nil
        window.close()
        Self.apply(savedEditor)
        let store = ThemeStore.shared
        store.unpinSurface()
        let s = savedTheme
        store.pin(theme: s.dark, accent: s.accent, tone: s.tone, mode: s.mode, systemIsDark: s.systemIsDark,
                  uiFont: s.ui, codeFont: s.code)
        store.pin(theme: s.light)
    }

    static func apply(_ layout: EditorLayout) {
        let prefs = EditorPreferences.shared
        prefs.showLineNumbers = layout.showLineNumbers
        prefs.highlightCurrentLine = layout.highlightCurrentLine
        prefs.highlightCurrentStatement = layout.highlightCurrentStatement
        prefs.wordWrap = layout.wordWrap
        prefs.codeFolding = layout.codeFolding
        prefs.showInvisibles = layout.showInvisibles
        prefs.autoUppercaseKeywords = layout.autoUppercaseKeywords
        prefs.runButtonPerStatement = layout.runButtonPerStatement
        prefs.tabWidth = layout.tabWidth
        prefs.fontSize = layout.fontSize
    }

    static func findTextView(in view: NSView) -> SQLTextView? {
        if let match = view as? SQLTextView { return match }
        for subview in view.subviews { if let found = findTextView(in: subview) { return found } }
        return nil
    }

    // MARK: Waiting

    func pump(for seconds: TimeInterval) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            view.layoutSubtreeIfNeeded()
        }
    }

    @discardableResult
    func pump(until condition: () -> Bool, timeout: TimeInterval = 10) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline { pump(for: 0.03) }
        return condition()
    }

    /// The analysis has painted and outlined: the statements, run marks and folds are the text's own.
    func settle() throws {
        try coordinator.syncAnalysisForTesting()
        pump(for: 0.15)
    }

    // MARK: Pixels

    /// The hosted view as an sRGB bitmap at the machine's own scale.
    func bitmap() throws -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()
        let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: rep)
        return rep
    }
}

extension NSBitmapImageRep {
    /// 8-bit sRGB components of the pixel at a point given in the view's own units (the bitmap may be
    /// 1x or 2x).
    func rgb(atX x: CGFloat, y: CGFloat, viewSize: CGSize) -> (r: Double, g: Double, b: Double) {
        let scaleX = CGFloat(pixelsWide) / viewSize.width, scaleY = CGFloat(pixelsHigh) / viewSize.height
        let color = colorAt(x: min(max(Int(x * scaleX), 0), pixelsWide - 1),
                            y: min(max(Int(y * scaleY), 0), pixelsHigh - 1))?
            .usingColorSpace(.sRGB) ?? .black
        return (Double(color.redComponent) * 255, Double(color.greenComponent) * 255, Double(color.blueComponent) * 255)
    }

    /// Rec. 709 luma of 8-bit components, the number the gutter's comment quotes (25, 12, 7).
    static func luma(_ c: (r: Double, g: Double, b: Double)) -> Double { 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b }
}
