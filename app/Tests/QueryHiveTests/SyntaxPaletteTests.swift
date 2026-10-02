import AppKit
import XCTest

@testable import QueryHive

/// G8: every syntax token clears 4.5:1 against every canvas of its appearance — except `comment`
/// in the dark, which is meant to recede. The dark `keyword` and `punctuation` values are the
/// V-12 Nord fix; this test locks them.
@MainActor
final class SyntaxPaletteTests: XCTestCase {
    /// The canvases, mirroring `AppTheme.canvas` in `Support/ThemeStore.swift`.
    private static let darkCanvases: [(AppTheme, UInt32)] = [
        (.midnight, 0x0A0B1E), (.graphite, 0x151517), (.nord, 0x2E3440), (.ink, 0x060709)]
    private static let lightCanvases: [(AppTheme, UInt32)] = [
        (.daylight, 0xF4F6FB), (.cloud, 0xF5F5F7), (.paper, 0xFAF8F4)]
    private static let classes: [EditorColorClass] = [
        .comment, .string, .quotedIdentifier, .number, .keyword,
        .literal, .function, .punctuation, .parameter]

    /// The table above is the theme store's values, not a copy that can drift: each entry must
    /// resolve to the hex beside it.
    func testTheCanvasTableMatchesTheThemeStore() {
        for (theme, hex) in Self.darkCanvases + Self.lightCanvases {
            XCTAssertEqual(Self.hex(of: NSColor(theme.canvas)), hex, "\(theme)")
        }
    }
    func testEveryTokenClearsContrastOnEveryCanvas() throws {
        for dark in [true, false] {
            let appearance = try XCTUnwrap(NSAppearance(named: dark ? .darkAqua : .aqua))
            appearance.performAsCurrentDrawingAppearance {
                for cls in Self.classes {
                    // Deliberate: the one token meant to recede in the dark.
                    if cls == .comment, dark { continue }
                    let foreground = Self.hex(of: SQLSyntax.colour(for: cls))
                    for (theme, background) in dark ? Self.darkCanvases : Self.lightCanvases {
                        XCTAssertGreaterThanOrEqual(Self.contrast(foreground, background),
                                                   4.5, "\(cls) on \(theme.rawValue)")
                    }
                }
            }
        }
    }
    private static func hex(of color: NSColor) -> UInt32 {
        let resolved = color.usingColorSpace(.sRGB) ?? color
        func byte(_ component: CGFloat) -> UInt32 { UInt32((component * 255).rounded()) }
        return byte(resolved.redComponent) << 16 | byte(resolved.greenComponent) << 8
            | byte(resolved.blueComponent)
    }

    /// WCAG 2.x contrast of two sRGB hex values.
    private static func contrast(_ foreground: UInt32, _ background: UInt32) -> Double {
        func luminance(_ hex: UInt32) -> Double {
            func linear(_ byte: UInt32) -> Double {
                let c = Double(byte) / 255
                return c <= 0.03928 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
            }
            return 0.2126 * linear(hex >> 16 & 0xFF) + 0.7152 * linear(hex >> 8 & 0xFF)
                + 0.0722 * linear(hex & 0xFF)
        }
        let light = max(luminance(foreground), luminance(background))
        let dark = min(luminance(foreground), luminance(background))
        return (light + 0.05) / (dark + 0.05)
    }
}
