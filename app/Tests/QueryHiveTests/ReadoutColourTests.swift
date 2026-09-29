import AppKit
import XCTest

@testable import QueryHive

/// The colour the editor's two readouts are drawn in.
///
/// The gutter numbers and the line count in the corner are one readout split in two, so they must be
/// one colour. They were not: the gutter used AppKit's `secondaryLabelColor`, the *system's* label
/// colour, which is a different grey from the app's `Tone.secondary` on every theme — and it sat
/// inches from a count drawn in the app's own. On a themed app, a system grey is a colour that
/// belongs to no theme.
@MainActor
final class ReadoutColourTests: XCTestCase {
    private func resolved(_ colour: NSColor, _ appearance: NSAppearance.Name) throws -> NSColor {
        let aqua = try XCTUnwrap(NSAppearance(named: appearance))
        var out = colour
        aqua.performAsCurrentDrawingAppearance {
            out = colour.usingColorSpace(.sRGB) ?? colour
        }
        return out
    }

    func testTheReadoutIsTheAppsInkOnADarkAppearance() throws {
        let dark = try resolved(Tone.readoutNS, .darkAqua)
        // The app's ink on a dark canvas is white, with no colour cast. A system label colour is
        // neither pure white nor this opacity.
        XCTAssertEqual(dark.redComponent, 1, accuracy: 0.001)
        XCTAssertEqual(dark.greenComponent, 1, accuracy: 0.001)
        XCTAssertEqual(dark.blueComponent, 1, accuracy: 0.001)
    }

    func testTheReadoutIsTheAppsInkOnALightAppearance() throws {
        let light = try resolved(Tone.readoutNS, .aqua)
        XCTAssertEqual(light.redComponent, 0, accuracy: 0.001)
        XCTAssertEqual(light.greenComponent, 0, accuracy: 0.001)
        XCTAssertEqual(light.blueComponent, 0, accuracy: 0.001)
    }

    func testTheReadoutIsFaintRatherThanSolid() throws {
        // It is a readout, not content: it has to be there without competing with the SQL.
        for appearance in [NSAppearance.Name.darkAqua, .aqua] {
            let colour = try resolved(Tone.readoutNS, appearance)
            XCTAssertEqual(colour.alphaComponent, 0.55, accuracy: 0.001,
                           "the readout opacity changed; the gutter and the count share it")
        }
    }

    // There is deliberately no test asserting this differs from `NSColor.secondaryLabelColor`.
    // Measured, it does not: that system colour resolves to white at 0.55 on a dark appearance and
    // to black at 0.55 on a light one, which is what this is. The difference that was actually
    // visible was the *opacity* — the gutter at 0.55 against a count at 0.374 — and the reason to
    // write the colour out here is that the system's is the system's to change, not that it is a
    // different grey today.
}
