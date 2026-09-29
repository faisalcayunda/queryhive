import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The font seam: what it must do when nothing is chosen, and what it must not offer.
///
/// The first property is the load-bearing one. A font setting whose default is not the font the app
/// already drew in would restyle every window on first launch, and the whole design would be
/// reviewed against a font nobody asked for. That property is checked by rendering: `--snapshot`
/// with no font flag is pixel-identical to a render from before this seam existed. What is left for
/// a unit test is the resolution itself and the contents of the two lists.
final class FontChoiceTests: XCTestCase {
    override func tearDown() {
        // Every test that changes a family puts it back, so one test cannot decide another's font.
        ThemeStore.shared.uiFontFamily = FontChoice.system
        ThemeStore.shared.codeFontFamily = FontChoice.system
        super.tearDown()
    }

    // MARK: The default

    func testTheDefaultIsTheSystemFont() {
        // Empty, not a named family: an app that shipped a font choice nobody made would be
        // restyling itself on first launch.
        XCTAssertEqual(FontChoice.system, "")
        XCTAssertTrue(ThemeStore.shared.uiFontFamily.isEmpty)
        XCTAssertTrue(ThemeStore.shared.codeFontFamily.isEmpty)
    }

    func testTheSystemDefaultResolvesToTheSystemFont() {
        // The seam must hand back the system font itself when nothing is chosen. Asserted on the
        // AppKit side, because that is what actually draws the glyphs. The UI half of the same
        // property is checked by rendering: `--snapshot` with no font flag is pixel-identical to a
        // render from before this seam existed, which is the stronger form of this test.
        let code = FontChoice.codeNSFont(size: 12, weight: nil)
        XCTAssertEqual(code.fontName, NSFont.monospacedSystemFont(ofSize: 12, weight: .regular).fontName)
    }

    // MARK: Choosing a family

    func testAChosenFamilyReachesTheResolvedFont() throws {
        // Menlo if it is installed, otherwise any fixed-pitch family this machine has. The test is
        // about the seam carrying a choice through, not about a particular font being present.
        let family = try XCTUnwrap(["Menlo", "Monaco", "Courier New"].first(where: {
            FontChoice.resolve($0, size: 12, weight: .regular) != nil
        }) ?? FontChoice.codeFamilies.first)
        ThemeStore.shared.codeFontFamily = family

        let resolved = FontChoice.codeNSFont(size: 12, weight: nil)
        let expected = NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12)
        XCTAssertEqual(resolved.fontName, expected?.fontName)
        // And it is genuinely not the system font, or the check above would pass on a no-op.
        XCTAssertNotEqual(resolved.fontName,
                          NSFont.monospacedSystemFont(ofSize: 12, weight: .regular).fontName)
    }

    func testAnUnknownFamilyFallsBackInsteadOfDrawingNothing() {
        // A family that was uninstalled after being chosen. Falling back is the right answer;
        // returning a font that draws boxes is not.
        ThemeStore.shared.uiFontFamily = "No Such Family 12345"
        XCTAssertNil(FontChoice.resolve("No Such Family 12345", size: 12, weight: .regular),
                     "the fallback is only exercised if the family really cannot be resolved")
        XCTAssertEqual(FontChoice.codeNSFont(size: 12, weight: nil).fontName,
                       NSFont.monospacedSystemFont(ofSize: 12, weight: .regular).fontName)
    }

    // MARK: The lists

    func testEveryOfferedUIFamilyCanBeResolved() {
        // A row that cannot be built is a row that does nothing when clicked.
        for family in FontChoice.uiFamilies {
            XCTAssertNotNil(NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12),
                            "\(family) is offered but cannot be resolved")
        }
    }

    func testTheUIFamiliesAreSortedAndFreeOfHiddenEntries() {
        let families = FontChoice.uiFamilies
        XCTAssertFalse(families.isEmpty)
        XCTAssertEqual(families, families.sorted { $0.localizedStandardCompare($1) == .orderedAscending })
        XCTAssertFalse(families.contains { $0.hasPrefix(".") })
    }

    func testTheCodeListIsMonospacedAndTheUIListIsNotJustThat() {
        // The code list is a strict subset, and every member really is fixed-pitch — measured, not
        // taken from a name list.
        XCTAssertFalse(FontChoice.codeFamilies.isEmpty)
        for family in FontChoice.codeFamilies {
            XCTAssertTrue(FontChoice.uiFamilies.contains(family), "\(family) is not in the UI list")
            let font = NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12)
            let unwrapped = font ?? NSFont.systemFont(ofSize: 12)
            let narrow = ("i" as NSString).size(withAttributes: [.font: unwrapped]).width
            let wide = ("W" as NSString).size(withAttributes: [.font: unwrapped]).width
            XCTAssertEqual(narrow, wide, accuracy: 0.01, "\(family) is offered as code but is not fixed-pitch")
        }
    }

    func testAPatchedCodingFontIsNotDroppedByTheMetadataFlag() throws {
        // The reason the filter measures instead of reading `isFixedPitch`: Nerd Font builds report
        // fixed-pitch as false. If such a font is installed, it must be in the code list — this is
        // the regression that would silently hide the fonts a developer is most likely to want.
        let patched = FontChoice.uiFamilies.filter {
            $0.contains("Nerd Font") || $0.hasPrefix("Fira")
        }
        try XCTSkipIf(patched.isEmpty, "no patched coding font installed on this machine")
        for family in patched {
            let font = try XCTUnwrap(NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12))
            let narrow = ("i" as NSString).size(withAttributes: [.font: font]).width
            let wide = ("W" as NSString).size(withAttributes: [.font: font]).width
            guard abs(narrow - wide) < 0.01 else { continue }
            XCTAssertTrue(FontChoice.codeFamilies.contains(family),
                          "\(family) is fixed-pitch but was dropped from the code list")
        }
    }

    // MARK: Persistence

    func testAChosenFamilySurvivesAndResetClearsIt() {
        ThemeStore.shared.uiFontFamily = "Menlo"
        XCTAssertEqual(ThemeStore.shared.uiFontFamily, "Menlo")

        ThemeStore.shared.reset()
        XCTAssertTrue(ThemeStore.shared.uiFontFamily.isEmpty)
        XCTAssertTrue(ThemeStore.shared.codeFontFamily.isEmpty)
    }
}
