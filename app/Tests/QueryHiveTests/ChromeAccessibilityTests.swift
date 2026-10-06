import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The accessibility floor for the chrome: every icon has a name, a tab says what it is doing, the
/// system's display switches reach the palette, and the text and stage colours are readable.
final class AccessibilityLabelTests: XCTestCase {
    /// Every `IconButton(` in the app, by scanning the sources: a view tree cannot be walked
    /// without a window, and a missing label is a property of the call, not of what it draws.
    func testEveryIconButtonHasANonEmptyName() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/QueryHive")
        let files = try XCTUnwrap(FileManager.default.enumerator(at: sources,
                                                                 includingPropertiesForKeys: nil))
        var calls = 0
        for case let url as URL in files where url.pathExtension == "swift" {
            let text = try String(contentsOf: url, encoding: .utf8)
            var rest = Substring(text)
            while let range = rest.range(of: "IconButton(symbol:") {
                calls += 1
                // The argument list ends at the first `) {` or `, action`; 260 characters covers
                // every call in the tree and stops short of the next one.
                let call = String(rest[range.lowerBound...].prefix(260))
                // A name is a `help:` or `label:` argument that is not the empty literal; a
                // ternary or a model string counts, since the call site decides the words.
                let named = (call.contains("help:") || call.contains("label:"))
                    && !call.contains("help: \"\"") && !call.contains("label: \"\"")
                XCTAssertTrue(named, "\(url.lastPathComponent): IconButton without a name: \(call.prefix(90))")
                rest = rest[range.upperBound...]
            }
        }
        XCTAssertGreaterThan(calls, 10, "the scan found the calls")
    }

    func testAChipReadsItsKindAndItsValue() {
        let chip = Chip(text: "varchar", kind: "Type")
        XCTAssertEqual(chip.kind, "Type")
    }
}

final class TabChipSpecTests: XCTestCase {
    func testEachStageHasAWordAndAShape() {
        let expected: [(QueryTab.Stage, String?, TabChipSpec.Glyph)] = [
            (.idle, nil, .none),
            (.running, "running", .spinner),
            (.done, "finished", .checkmark),
            (.failed, "failed", .exclamation),
        ]
        for (stage, value, glyph) in expected {
            let spec = TabChipSpec(title: "Query 1", stage: stage, isSelected: false)
            XCTAssertEqual(spec.label, "Query 1")
            XCTAssertEqual(spec.value, value)
            XCTAssertEqual(spec.glyph, glyph)
            XCTAssertEqual(spec.actions, ["Close"])
        }
    }

    func testSelectionIsCarriedSeparatelyFromTheStage() {
        XCTAssertTrue(TabChipSpec(title: "a", stage: .failed, isSelected: true).isSelected)
        XCTAssertFalse(TabChipSpec(title: "a", stage: .failed, isSelected: false).isSelected)
    }
}

final class SurfacePolicyTests: XCTestCase {
    override func tearDown() {
        ThemeStore.shared.unpinSurface()
        super.tearDown()
    }

    private func alpha(_ color: Color) -> CGFloat {
        var value: CGFloat = 0
        NSAppearance(named: .darkAqua)!.performAsCurrentDrawingAppearance {
            value = NSColor(color).usingColorSpace(.sRGB)!.alphaComponent
        }
        return value
    }

    func testPinningChangesTheTextAndTheHairlines() {
        let store = ThemeStore.shared
        store.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        let calm = (alpha(Tone.secondary), alpha(Tone.hairline), alpha(Tone.outline))
        XCTAssertEqual(calm.0, 0.68, accuracy: 0.001)
        XCTAssertEqual(calm.1, 0.07, accuracy: 0.001)

        store.pin(increaseContrast: true)
        XCTAssertTrue(store.surface.enhanced)
        XCTAssertGreaterThan(alpha(Tone.secondary), calm.0)
        XCTAssertGreaterThan(alpha(Tone.hairline), calm.1)
        XCTAssertGreaterThan(alpha(Tone.outline), calm.2)
    }

    func testReduceTransparencyAloneIsEnoughToSwitchTheGlassOff() {
        let store = ThemeStore.shared
        store.pin(reduceMotion: false, reduceTransparency: true, increaseContrast: false)
        XCTAssertTrue(store.surface.enhanced)
        XCTAssertFalse(store.surface.reduceMotion)
    }

    func testReduceMotionDoesNotChangeTheColours() {
        let store = ThemeStore.shared
        store.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        let before = alpha(Tone.secondary)
        store.pin(reduceMotion: true)
        XCTAssertTrue(store.surface.reduceMotion)
        XCTAssertFalse(store.surface.enhanced)
        XCTAssertEqual(alpha(Tone.secondary), before)
    }

    func testAPinnedStoreIgnoresTheSystemNotification() {
        let store = ThemeStore.shared
        store.pin(reduceMotion: true)
        NSWorkspace.shared.notificationCenter.post(
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertTrue(store.surface.reduceMotion)
    }
}

final class ChromeContrastTests: XCTestCase {
    private func resolve(_ color: Color, dark: Bool) -> (r: Double, g: Double, b: Double, a: Double) {
        let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
        var out = (r: 0.0, g: 0.0, b: 0.0, a: 0.0)
        appearance.performAsCurrentDrawingAppearance {
            let ns = NSColor(color).usingColorSpace(.sRGB)!
            out = (Double(ns.redComponent), Double(ns.greenComponent), Double(ns.blueComponent),
                   Double(ns.alphaComponent))
        }
        return out
    }

    private func luminance(_ c: (r: Double, g: Double, b: Double)) -> Double {
        func lin(_ v: Double) -> Double { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    /// WCAG contrast of `foreground` composited over `canvas`.
    private func contrast(_ foreground: Color, over canvas: Color, dark: Bool) -> Double {
        let back = resolve(canvas, dark: dark)
        let front = resolve(foreground, dark: dark)
        let mixed = (r: front.r * front.a + back.r * (1 - front.a),
                     g: front.g * front.a + back.g * (1 - front.a),
                     b: front.b * front.a + back.b * (1 - front.a))
        let (a, b) = (luminance(mixed), luminance((back.r, back.g, back.b)))
        return (max(a, b) + 0.05) / (min(a, b) + 0.05)
    }

    /// `layers` painted in order over `canvas`, as one opaque colour.
    private func flatten(_ layers: [Color], over canvas: Color, dark: Bool) -> Color {
        var out = resolve(canvas, dark: dark)
        for layer in layers {
            let l = resolve(layer, dark: dark)
            out = (r: l.r * l.a + out.r * (1 - l.a), g: l.g * l.a + out.g * (1 - l.a),
                   b: l.b * l.a + out.b * (1 - l.a), a: 1)
        }
        return Color(.sRGB, red: out.r, green: out.g, blue: out.b, opacity: 1)
    }

    override func tearDown() {
        ThemeStore.shared.unpinSurface()
        super.tearDown()
    }

    func testTextTokensClearFourAndAHalfOnEveryCanvasInBothPolicies() {
        for enhanced in [false, true] {
            ThemeStore.shared.pin(reduceMotion: false, reduceTransparency: false,
                                  increaseContrast: enhanced)
            for theme in AppTheme.allCases {
                for (name, token) in [("secondary", Tone.secondary), ("readout", Tone.readout)] {
                    let ratio = contrast(token, over: theme.canvas, dark: theme.isDark)
                    XCTAssertGreaterThanOrEqual(ratio, 4.5,
                        "\(name) on \(theme.title), enhanced \(enhanced): \(ratio)")
                }
            }
        }
    }

    /// The Log tab's count sits on a capsule over the tab's own wash over the canvas. The accent used
    /// to be the digits' colour when selected, which is 1.0:1 on the three light themes.
    func testTheTabCountClearsFourAndAHalfOnItsCapsuleInEveryTheme() {
        for enhanced in [false, true] {
            ThemeStore.shared.pin(reduceMotion: false, reduceTransparency: false,
                                  increaseContrast: enhanced)
            for theme in AppTheme.allCases {
                for (state, selected, wash) in [("selected", true, PanelTabButton.selectedWash),
                                                ("hovered", false, PanelTabButton.hoverWash),
                                                ("idle", false, 0)] {
                    let capsule = flatten([Tone.ink.opacity(wash), Tone.ink.opacity(PanelTabButton.countWash)],
                                          over: theme.canvas, dark: theme.isDark)
                    let ratio = contrast(PanelTabButton.ink(selected: selected), over: capsule,
                                         dark: theme.isDark)
                    XCTAssertGreaterThanOrEqual(ratio, 4.5,
                        "\(state) count on \(theme.title), enhanced \(enhanced): \(ratio)")
                }
            }
        }
    }

    func testStageTintsClearThreeToOneOnTheirOwnCanvases() {
        for theme in AppTheme.allCases {
            for (name, token) in [("mint", Tone.markMint), ("coral", Tone.markCoral),
                                  ("amber", Tone.markAmber)] {
                let ratio = contrast(token, over: theme.canvas, dark: theme.isDark)
                XCTAssertGreaterThanOrEqual(ratio, 3, "\(name) on \(theme.title): \(ratio)")
            }
        }
    }

    func testAControlEdgeIsVisibleWhenTheSystemAsksForContrast() {
        ThemeStore.shared.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: true)
        for theme in AppTheme.allCases {
            let ratio = contrast(Tone.outline, over: theme.canvas, dark: theme.isDark)
            XCTAssertGreaterThanOrEqual(ratio, 3, "outline on \(theme.title): \(ratio)")
        }
    }
}

final class EmptyCopyTests: XCTestCase {
    func testTheWorkspaceNoLongerPromisesToStreamToDisk() {
        XCTAssertFalse(EmptyCopy.workspace.contains("streams"))
        XCTAssertTrue(EmptyCopy.workspace.contains("press Run"))
    }

    func testThePlaceholderFollowsTheDialect() {
        XCTAssertEqual(EmptyCopy.placeholder(for: .trino), "SELECT * FROM catalog.schema.table")
        XCTAssertEqual(EmptyCopy.placeholder(for: .postgres), "SELECT * FROM schema.table")
        XCTAssertEqual(EmptyCopy.placeholder(for: .mysql), "SELECT * FROM database.table")
        XCTAssertEqual(EmptyCopy.placeholder(for: nil), "SELECT * FROM table")
    }

    func testTheHelpNamesTheKeyOfTheSchemeInForce() {
        XCTAssertEqual(EmptyCopy.help("New Query", .newQuery, scheme: .dbeaver), "New Query (⌃])")
        XCTAssertEqual(EmptyCopy.help("New Query", .newQuery, scheme: .queryhive), "New Query (⌘T)")
    }
}
