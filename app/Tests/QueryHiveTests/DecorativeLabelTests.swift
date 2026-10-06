import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The one exception to the 11 pt floor (W9-T9, blueprint w9 §12.1 item 2): an uppercase caption at
/// 10 pt, in ink of at least 0.68. `FontFloorTests` keeps everything else at 11.
final class DecorativeLabelTests: XCTestCase {
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

    func testTheSizeIsTen() {
        XCTAssertEqual(DecorativeLabel.size, 10)
    }

    func testTheInkIsAtLeastPointSixtyEightCalmOrEnhanced() {
        let store = ThemeStore.shared
        store.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        XCTAssertGreaterThanOrEqual(alpha(DecorativeLabel.ink), 0.68 - 0.001)
        store.pin(increaseContrast: true)
        XCTAssertGreaterThanOrEqual(alpha(DecorativeLabel.ink), 0.68 - 0.001)
    }

    func testSectionLabelIsADecorativeLabel() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/QueryHive/Support/Theme.swift")
        let text = try String(contentsOf: sources, encoding: .utf8)
        let section = try XCTUnwrap(text.range(of: "struct SectionLabel"))
        XCTAssertTrue(text[section.lowerBound...].prefix(260).contains("DecorativeLabel(text:"))
    }
}
