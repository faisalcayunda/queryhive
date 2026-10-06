import AppKit
import XCTest

@testable import QueryHive

/// The painter owns `.underlineStyle` and `.underlineColor` on the layout manager and nothing else.
final class DiagnosticsPainterTests: XCTestCase {
    private func manager(_ text: String) -> (NSLayoutManager, NSTextStorage) {
        let storage = NSTextStorage(string: text)
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        layout.addTextContainer(NSTextContainer(size: NSSize(width: 400, height: 400)))
        return (layout, storage)
    }

    private func lexical(_ location: Int, _ length: Int) -> EditorDiagnostic {
        EditorDiagnostic(kind: .lexical(.unclosedQuote), range: NSRange(location: location, length: length),
                         message: "Unclosed quote")
    }

    private func server(_ location: Int, _ length: Int) -> EditorDiagnostic {
        EditorDiagnostic(kind: .server, range: NSRange(location: location, length: length), message: "boom")
    }

    private func style(_ layout: NSLayoutManager, _ index: Int) -> Int? {
        layout.temporaryAttribute(.underlineStyle, atCharacterIndex: index, effectiveRange: nil) as? Int
    }

    func testLexicalIsDashedAmberAndServerIsThickCoral() {
        let (layout, storage) = manager("select 'abc from t")
        let painter = DiagnosticsPainter()
        painter.apply([lexical(7, 4), server(14, 1)], to: layout, length: storage.length)
        XCTAssertEqual(style(layout, 8), DiagnosticsPainter.lexicalStyle)
        XCTAssertTrue(layout.temporaryAttribute(.underlineColor, atCharacterIndex: 8, effectiveRange: nil)
            as? NSColor === DiagnosticsPainter.amber)
        XCTAssertEqual(style(layout, 14), DiagnosticsPainter.serverStyle)
        XCTAssertTrue(layout.temporaryAttribute(.underlineColor, atCharacterIndex: 14, effectiveRange: nil)
            as? NSColor === DiagnosticsPainter.coral)
        XCTAssertNotEqual(DiagnosticsPainter.lexicalStyle, DiagnosticsPainter.serverStyle)
        XCTAssertNil(style(layout, 0))
        XCTAssertNil(style(layout, 12))
    }

    func testTheStylesAreWhatTheBlueprintSays() {
        XCTAssertEqual(DiagnosticsPainter.lexicalStyle,
                       NSUnderlineStyle.single.rawValue | NSUnderlineStyle.patternDash.rawValue)
        XCTAssertEqual(DiagnosticsPainter.serverStyle, NSUnderlineStyle.thick.rawValue)
    }

    func testAFixRemovesTheUnderline() {
        let (layout, storage) = manager("select 'abc")
        let painter = DiagnosticsPainter()
        painter.apply([lexical(7, 4)], to: layout, length: storage.length)
        XCTAssertNotNil(style(layout, 8))
        painter.apply([], to: layout, length: storage.length)
        XCTAssertNil(style(layout, 8))
        XCTAssertEqual(painter.painted, [])
    }

    func testAMovedIssueLeavesNothingBehindAtItsOldPlace() {
        let (layout, storage) = manager("select 'abc from t")
        let painter = DiagnosticsPainter()
        painter.apply([lexical(7, 4)], to: layout, length: storage.length)
        painter.apply([lexical(14, 4)], to: layout, length: storage.length)
        XCTAssertNil(style(layout, 8))
        XCTAssertNotNil(style(layout, 15))
    }

    func testNothingIsWrittenWhenNothingChanged() {
        let (layout, storage) = manager("select 'abc")
        let painter = DiagnosticsPainter()
        painter.apply([lexical(7, 4)], to: layout, length: storage.length)
        // A mark planted by hand survives a repeat of the same paint: the painter did not touch it.
        layout.addTemporaryAttribute(.underlineStyle, value: 99, forCharacterRange: NSRange(location: 0, length: 1))
        painter.apply([lexical(7, 4)], to: layout, length: storage.length)
        XCTAssertEqual(style(layout, 0), 99)
    }

    func testRangesPastTheEndOfTheTextAreIgnored() {
        let (layout, storage) = manager("select")
        let painter = DiagnosticsPainter()
        painter.apply([lexical(4, 9)], to: layout, length: storage.length)
        XCTAssertEqual(painter.painted, [])
    }

    func testTheOtherAttributeOwnersAreNotTouched() {
        let (layout, storage) = manager("select 'abc")
        layout.addTemporaryAttribute(.foregroundColor, value: NSColor.red, forCharacterRange: NSRange(location: 0, length: 6))
        layout.addTemporaryAttribute(.backgroundColor, value: NSColor.yellow, forCharacterRange: NSRange(location: 7, length: 4))
        let painter = DiagnosticsPainter()
        painter.apply([lexical(7, 4)], to: layout, length: storage.length)
        painter.apply([], to: layout, length: storage.length)
        painter.reset(layout, length: storage.length)
        XCTAssertEqual(layout.temporaryAttribute(.foregroundColor, atCharacterIndex: 2, effectiveRange: nil) as? NSColor, .red)
        XCTAssertEqual(layout.temporaryAttribute(.backgroundColor, atCharacterIndex: 8, effectiveRange: nil) as? NSColor, .yellow)
    }

    func testResetRemovesWhatWasPainted() {
        let (layout, storage) = manager("select 'abc")
        let painter = DiagnosticsPainter()
        painter.apply([server(0, 6)], to: layout, length: storage.length)
        painter.reset(layout, length: storage.length)
        XCTAssertNil(style(layout, 2))
        XCTAssertEqual(painter.painted, [])
    }

    func testTheColoursResolvePerAppearance() {
        for appearance in [NSAppearance(named: .darkAqua)!, NSAppearance(named: .aqua)!] {
            appearance.performAsCurrentDrawingAppearance {
                let amber = DiagnosticsPainter.amber.usingColorSpace(.sRGB)
                let coral = DiagnosticsPainter.coral.usingColorSpace(.sRGB)
                XCTAssertEqual(amber?.alphaComponent, 1)
                XCTAssertEqual(coral?.alphaComponent, 1)
            }
        }
        var dark: NSColor?
        var light: NSColor?
        NSAppearance(named: .darkAqua)!.performAsCurrentDrawingAppearance { dark = DiagnosticsPainter.amber.usingColorSpace(.sRGB) }
        NSAppearance(named: .aqua)!.performAsCurrentDrawingAppearance { light = DiagnosticsPainter.amber.usingColorSpace(.sRGB) }
        XCTAssertNotEqual(dark, light, "the amber must follow the appearance, or it fails 3:1 on one canvas")
    }
}
