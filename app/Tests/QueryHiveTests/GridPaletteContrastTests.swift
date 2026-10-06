import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The grid's palette against the seven canvases (W10-T2, blueprint w10 §4.1, owner decision O-26).
///
/// Row numbers and NULL are text and clear 4.5:1 whatever the setting; the idle funnel is a mark and
/// clears 3:1 whatever the setting; the separators and the header rule are hairlines until
/// Increase Contrast, and clear 3:1 under it. The staged washes carry the cell text at 4.5:1.
final class GridPaletteContrastTests: XCTestCase {
    private struct RGB { var r, g, b: Double }

    private static func srgb(_ colour: CGColor) -> (rgb: RGB, alpha: Double) {
        let ns = NSColor(cgColor: colour)!.usingColorSpace(.sRGB)!
        return (RGB(r: Double(ns.redComponent), g: Double(ns.greenComponent), b: Double(ns.blueComponent)),
                Double(ns.alphaComponent))
    }

    private static func canvas(_ theme: AppTheme) -> RGB {
        var out = RGB(r: 0, g: 0, b: 0)
        NSAppearance(named: theme.isDark ? .darkAqua : .aqua)!.performAsCurrentDrawingAppearance {
            let ns = NSColor(theme.canvas).usingColorSpace(.sRGB)!
            out = RGB(r: Double(ns.redComponent), g: Double(ns.greenComponent), b: Double(ns.blueComponent))
        }
        return out
    }

    private static func over(_ layer: CGColor, _ below: RGB) -> RGB {
        let (top, a) = srgb(layer)
        return RGB(r: top.r * a + below.r * (1 - a), g: top.g * a + below.g * (1 - a),
                   b: top.b * a + below.b * (1 - a))
    }

    private static func luminance(_ c: RGB) -> Double {
        func lin(_ v: Double) -> Double { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    private static func ratio(_ a: RGB, _ b: RGB) -> Double {
        let (x, y) = (luminance(a), luminance(b))
        return (max(x, y) + 0.05) / (min(x, y) + 0.05)
    }

    private func palette(_ theme: AppTheme, enhanced: Bool) -> GridPalette {
        GridPalette.resolve(NSAppearance(named: theme.isDark ? .darkAqua : .aqua)!, enhanced: enhanced)
    }

    private func forEveryCanvas(_ body: (AppTheme, Bool, GridPalette, RGB) -> Void) {
        XCTAssertEqual(AppTheme.allCases.count, 7, "the seven canvases")
        for theme in AppTheme.allCases {
            for enhanced in [false, true] {
                body(theme, enhanced, palette(theme, enhanced: enhanced), Self.canvas(theme))
            }
        }
    }

    func testRowNumbersAndNullClearFourAndAHalfOnEveryCanvasInBothSettings() {
        forEveryCanvas { theme, enhanced, p, canvas in
            // On the plain canvas and on a striped row, which is the worse of the two.
            for (name, ink) in [("row number", p.inkDim), ("NULL", p.inkNull)] {
                for ground in [canvas, Self.over(p.stripe, canvas)] {
                    let r = Self.ratio(Self.over(ink, ground), ground)
                    XCTAssertGreaterThanOrEqual(r, 4.5, "\(name) on \(theme.title), enhanced \(enhanced): \(r)")
                }
            }
        }
    }

    func testTheIdleFunnelClearsThreeToOneOnEveryCanvasInBothSettings() {
        forEveryCanvas { theme, enhanced, p, canvas in
            let band = Self.over(p.headerBand, canvas)
            let r = Self.ratio(Self.over(p.funnel, band), band)
            XCTAssertGreaterThanOrEqual(r, 3, "funnel on \(theme.title), enhanced \(enhanced): \(r)")
        }
    }

    /// O-26: a hairline until Increase Contrast, 3:1 under it.
    func testSeparatorsAndTheRuleAreHairlinesCalmAndThreeToOneEnhanced() {
        forEveryCanvas { theme, enhanced, p, canvas in
            let separator = Self.ratio(Self.over(p.inkFaint, canvas), canvas)
            let rule = Self.ratio(Self.over(p.rule, canvas), canvas)
            if enhanced {
                XCTAssertGreaterThanOrEqual(separator, 3, "separator on \(theme.title): \(separator)")
                XCTAssertGreaterThanOrEqual(rule, 3, "rule on \(theme.title): \(rule)")
            } else {
                XCTAssertEqual(GridInk.calm.separator, 0.05)
                XCTAssertEqual(GridInk.calm.rule, 0.12)
                XCTAssertLessThan(separator, 3, "a calm separator stays a hairline on \(theme.title)")
            }
        }
    }

    func testCellTextOnAChangedCellAnAddedRowAndADeletedRowClearsFourAndAHalf() {
        forEveryCanvas { theme, enhanced, p, canvas in
            let changed = Self.over(p.staged, canvas)
            let added = Self.over(p.insertedWash, canvas)
            let deleted = Self.over(p.deletedWash, canvas)
            for (name, wash, ink) in [("changed", changed, p.inkStrong), ("added", added, p.inkStrong),
                                      ("added, DEFAULT", added, p.inkSecondary),
                                      ("deleted", deleted, p.inkSecondary)] {
                let r = Self.ratio(Self.over(ink, wash), wash)
                XCTAssertGreaterThanOrEqual(r, 4.5, "\(name) on \(theme.title), enhanced \(enhanced): \(r)")
            }
        }
    }

    func testTheStagedMarksClearThreeToOneOverTheirOwnWash() {
        forEveryCanvas { theme, enhanced, p, canvas in
            for (name, mark, wash) in [("dot", p.amber, p.staged), ("+", p.markMint, p.insertedWash),
                                       ("−", p.markCoral, p.deletedWash)] {
                let ground = Self.over(wash, canvas)
                let r = Self.ratio(Self.over(mark, ground), ground)
                XCTAssertGreaterThanOrEqual(r, 3, "\(name) on \(theme.title), enhanced \(enhanced): \(r)")
            }
        }
    }

    /// The ring is `Tone.focusRing` (TD-1): the grid keeps no copy of the rule.
    func testTheCursorRingIsTheFocusRingToken() {
        for dark in [false, true] {
            let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
            var token = CGColor(gray: 0, alpha: 1)
            appearance.performAsCurrentDrawingAppearance { token = Tone.focusRingNS.cgColor }
            XCTAssertEqual(GridPalette.resolve(appearance, enhanced: false).cursor, token, "dark: \(dark)")
        }
    }

    func testTheCalmAndEnhancedPalettesDiffer() {
        let appearance = NSAppearance(named: .darkAqua)!
        XCTAssertNotEqual(GridPalette.resolve(appearance, enhanced: false),
                          GridPalette.resolve(appearance, enhanced: true),
                          "a change of Increase Contrast has to be a change of the palette, or the line cache keeps stale colours")
    }
}
