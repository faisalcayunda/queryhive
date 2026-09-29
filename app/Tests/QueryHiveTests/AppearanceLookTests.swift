import XCTest

@testable import QueryHive

/// The appearance model: what a preset is, when the appearance is Custom, and what saving keeps.
///
/// `ThemeStore` is a singleton whose setters persist, so every test here snapshots the real
/// preferences and puts them back. Each test states the appearance it needs rather than trusting
/// what the one before left behind.
final class AppearanceLookTests: XCTestCase {
    private let keys = ["appTheme", "lightTheme", "accentChoice", "glowIntensity", "surfaceTone",
                        "appearanceMode", "uiFontFamily", "codeFontFamily", "savedAppearanceLooks"]

    override func setUp() {
        super.setUp()
        let defaults = UserDefaults.standard
        let snapshot = keys.reduce(into: [String: Any]()) { result, key in
            result[key] = defaults.object(forKey: key)
        }
        let keys = self.keys
        addTeardownBlock {
            let store = ThemeStore.shared
            store.reset()
            for look in store.savedLooks { store.deleteLook(look.id) }
            for key in keys {
                if let value = snapshot[key] {
                    defaults.set(value, forKey: key)
                } else {
                    defaults.removeObject(forKey: key)
                }
            }
        }
    }

    private var classic: ThemePreset {
        ThemePreset.all.first { $0.id == "classic" }!
    }

    // MARK: A preset is a whole appearance

    func testAPresetNamesAllFiveParts() {
        XCTAssertEqual(classic.look,
                       AppearanceLook(darkTheme: .midnight, lightTheme: .daylight,
                                      accent: .ice, tone: .glow, glow: 1.0))
    }

    func testApplyingAPresetMakesItTheCurrentOne() {
        let store = ThemeStore.shared
        store.apply(classic)
        XCTAssertEqual(store.currentPreset, classic)
        XCTAssertFalse(store.isCustomLook)
    }

    // MARK: Custom means "matches nothing"

    func testChangingOnePartMakesItCustom() {
        let store = ThemeStore.shared
        store.apply(classic)
        store.accent = .mint
        XCTAssertNil(store.currentPreset)
        XCTAssertTrue(store.isCustomLook, "a changed accent is not Classic")
    }

    func testMovingTheGlowMakesItCustomToo() {
        // The glow is part of a look rather than a decoration beside it, which is why applying a
        // preset sets it: otherwise "pick Classic, move the glow" would still read as Classic, and
        // the person who moved it was told they had changed nothing.
        let store = ThemeStore.shared
        store.apply(classic)
        store.glow = 0.4
        XCTAssertTrue(store.isCustomLook)
    }

    // MARK: Saving

    func testSavingKeepsExactlyWhatWasOnScreen() {
        let store = ThemeStore.shared
        store.apply(classic)
        store.accent = .violet
        store.tone = .soft
        let onScreen = store.currentLook

        let saved = store.saveLook(named: "Night shift")

        XCTAssertNotNil(saved)
        XCTAssertEqual(saved?.look, onScreen)
        XCTAssertEqual(store.savedLooks.map(\.name), ["Night shift"])
        // It matches the look that was just kept, so it is not custom any more, and it is still not
        // one of the presets.
        XCTAssertFalse(store.isCustomLook)
        XCTAssertNil(store.currentPreset)
    }

    func testABlankNameSavesNothing() {
        // A look nobody named is a look nobody can pick again, so it is refused rather than kept
        // under an empty row.
        let store = ThemeStore.shared
        XCTAssertNil(store.saveLook(named: "   "))
        XCTAssertTrue(store.savedLooks.isEmpty)
    }

    func testApplyingASavedLookBringsItBack() {
        let store = ThemeStore.shared
        store.apply(classic)
        store.accent = .violet
        let saved = store.saveLook(named: "Night shift")
        store.accent = .mint

        store.apply(saved!.look)

        XCTAssertEqual(store.accent, .violet)
        XCTAssertEqual(store.currentLook, saved?.look)
        XCTAssertFalse(store.isCustomLook)
    }

    func testDeletingRemovesItAndTheLookBecomesCustomAgain() {
        let store = ThemeStore.shared
        store.apply(classic)
        store.accent = .violet
        let saved = store.saveLook(named: "Night shift")!
        XCTAssertFalse(store.isCustomLook)

        store.deleteLook(saved.id)

        XCTAssertTrue(store.savedLooks.isEmpty)
        XCTAssertTrue(store.isCustomLook)
    }

    // MARK: What is written down

    func testASavedLookSurvivesTheRoundTripThroughJSON() throws {
        // The store keeps saved looks as JSON in one preference, so this is the shape that has to
        // survive a relaunch.
        let look = AppearanceLook(darkTheme: .graphite, lightTheme: .cloud,
                                  accent: .blue, tone: .plain, glow: 0.75)
        let saved = SavedLook(id: UUID(), name: "Flat-ish", look: look)

        let data = try JSONEncoder().encode([saved])
        let back = try JSONDecoder().decode([SavedLook].self, from: data)

        XCTAssertEqual(back, [saved])
    }
}
