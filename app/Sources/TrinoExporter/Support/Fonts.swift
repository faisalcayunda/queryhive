import AppKit
import SwiftUI

/// The font families the app can draw in, and the one seam every call site goes through to get one.
///
/// Font choice is a **user preference with two halves**, not one. Chrome text (labels, tabs,
/// buttons) and code text (the SQL editor, the result grid, every monospaced value) want different
/// families, and every database client this app is measured against offers them separately:
/// DataGrip has an editor font and a UI font, Navicat has one of each. A single "font" setting would
/// have to choose which of the two it meant.
///
/// ## Why the call sites go through a helper
///
/// There are ~140 `.font(.system(size:…))` call sites. None of them can ask a `Font` value which
/// family the user picked, because `Font` is a plain value with no environment behind it, so the
/// choice has to be applied where the font is *built*. `Font.ui` and `Font.code` are that place.
///
/// Each returns the system font **exactly as before** while no family is chosen, which is what
/// keeps this from becoming a restyle of the whole app: pick nothing and nothing moves, pick a
/// family and every text run that goes through the seam follows. That property is also what makes
/// the change checkable — a snapshot rendered with the defaults must be pixel-identical to one
/// rendered before the seam existed.
///
/// The seam deliberately does **not** cover `Image(systemName:)`. An SF Symbol is drawn from the
/// system symbol font, and handing it a text family replaces the glyph with a missing-character
/// box, so those call sites keep `.font(.system(…))` and the rewrite skips them.
enum FontChoice {
    /// The stored value for "the system font". An empty family is not a family, and it is what both
    /// the store and the picker use for "no choice made".
    static let system = ""

    /// Every family installed on this machine that can actually be resolved, for the chrome picker.
    ///
    /// Read from `NSFontManager` rather than shipped as a list, because a list would name fonts
    /// that are not installed (every candidate is offered, and picking a missing one silently falls
    /// back to the system font, which looks like the setting being broken). The resolution check
    /// drops the families `availableFontFamilies` reports but `font(withFamily:)` cannot build —
    /// hidden and alias entries such as `.AppleSystemUIFont` — which would be rows that do nothing.
    static let uiFamilies: [String] = {
        NSFontManager.shared.availableFontFamilies
            .filter { !$0.hasPrefix(".") }
            .filter { NSFontManager.shared.font(withFamily: $0, traits: [], weight: 5, size: 12) != nil }
            .sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }()

    /// The fixed-pitch families, for the code picker.
    ///
    /// Decided by **measuring glyph advances**, not by asking the font whether it is fixed-pitch.
    /// `NSFont.isFixedPitch` and CoreText's `traitMonoSpace` both report `false` for fonts whose
    /// metadata was rewritten by a patcher, and Nerd Font builds of Fira Code and Fira Mono are
    /// exactly that — the two coding fonts most likely to be installed on a developer's machine
    /// were the two this filter was silently dropping. Measured on this machine: the flag found 5
    /// families, measuring finds 9, and the four extra are the ones a developer would want.
    ///
    /// The test is the property itself rather than a proxy for it: a lowercase `i` and an uppercase
    /// `W` must have the same advance. Both are compared against `m` as well, because a font can
    /// happen to match on one pair.
    static let codeFamilies: [String] = {
        uiFamilies.filter { family in
            guard let font = NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: 12)
            else { return false }
            let narrow = ("i" as NSString).size(withAttributes: [.font: font]).width
            let wide = ("W" as NSString).size(withAttributes: [.font: font]).width
            let wide2 = ("m" as NSString).size(withAttributes: [.font: font]).width
            return narrow > 0 && abs(narrow - wide) < 0.01 && abs(narrow - wide2) < 0.01
        }
    }()

    /// Resolved fonts, keyed by family, size and weight. A body can ask for a font hundreds of
    /// times per frame, and `NSFontManager` is a lookup rather than a computation — caching it
    /// keeps the seam from being a per-frame cost. Only the main thread reads or writes this,
    /// because it is only ever touched from a view body.
    private static var cache: [String: NSFont] = [:]

    /// The AppKit weight for a SwiftUI weight. `NSFontManager` takes the 0–15 scale, where 5 is
    /// regular and 9 is bold.
    private static func appKitWeight(_ weight: Font.Weight) -> Int {
        switch weight {
        case .ultraLight: 2
        case .thin: 3
        case .light: 4
        case .regular: 5
        case .medium: 6
        case .semibold: 8
        case .bold: 9
        case .heavy: 11
        case .black: 12
        default: 5
        }
    }

    /// A family's font at a size and weight, or nil when the family cannot supply one.
    ///
    /// Internal rather than private so a test can ask the same question the seam asks, instead of
    /// going through a SwiftUI `Font` — there is no public way to read a family back out of one, and
    /// a test that cannot see what it is asserting on is not a test.
    static func resolve(_ family: String, size: CGFloat, weight: Font.Weight) -> NSFont? {
        let key = "\(family)|\(size)|\(appKitWeight(weight))"
        if let cached = cache[key] { return cached }
        guard let font = NSFontManager.shared.font(withFamily: family, traits: [],
                                                   weight: appKitWeight(weight), size: size)
        else { return nil }
        cache[key] = font
        return font
    }

    /// The interface font, or the system font when no family is chosen or the chosen one is gone.
    ///
    /// The no-choice branch rebuilds the *exact* call the call site used before this seam existed —
    /// no `weight:` when none was asked for, `design:` only when rounded — because `.system(size:
    /// 12, weight: .regular, design: .default)` is not guaranteed to be the same font as
    /// `.system(size: 12)`. Keeping them literally equal is what makes "pick nothing, nothing
    /// moves" checkable by diffing renders instead of taking it on faith.
    static func ui(size: CGFloat, weight: Font.Weight?, rounded: Bool) -> Font {
        let family = ThemeStore.shared.uiFontFamily
        if !family.isEmpty, let font = resolve(family, size: size, weight: weight ?? .regular) {
            return Font(font)
        }
        // A family the user picked and later uninstalled falls back rather than drawing boxes.
        switch (weight, rounded) {
        case let (weight?, true): return .system(size: size, weight: weight, design: .rounded)
        case let (weight?, false): return .system(size: size, weight: weight)
        case (nil, true): return .system(size: size, design: .rounded)
        case (nil, false): return .system(size: size)
        }
    }

    /// The code font: the editor, the grid, and every monospaced value.
    ///
    /// The no-choice branch rebuilds the exact call the site used before the seam, `weight:` only
    /// when one was asked for, for the same reason `ui` does: naming `.regular` is not the same
    /// request as leaving the weight out. Measured on the same scene, `.system(size:weight:.regular,
    /// design:.monospaced)` against `.system(size:design:.monospaced)` moved ~14 200 pixels, which is
    /// what a "default must not restyle the app" check is there to catch.
    static func code(size: CGFloat, weight: Font.Weight?) -> Font {
        let family = ThemeStore.shared.codeFontFamily
        if !family.isEmpty, let font = resolve(family, size: size, weight: weight ?? .regular) {
            return Font(font)
        }
        guard let weight else { return .system(size: size, design: .monospaced) }
        return .system(size: size, weight: weight, design: .monospaced)
    }

    /// The same choice as an `NSFont`, for the two AppKit surfaces that cannot take a `Font`: the
    /// SQL editor's text view and the syntax highlighter that paints into it.
    static func codeNSFont(size: CGFloat, weight: Font.Weight?) -> NSFont {
        let family = ThemeStore.shared.codeFontFamily
        if !family.isEmpty, let font = resolve(family, size: size, weight: weight ?? .regular) {
            return font
        }
        return .monospacedSystemFont(ofSize: size, weight: nsWeight(weight ?? .regular))
    }

    /// The AppKit weight for `NSFont.monospacedSystemFont`, which takes the system's own enum.
    private static func nsWeight(_ weight: Font.Weight) -> NSFont.Weight {
        switch weight {
        case .ultraLight: .ultraLight
        case .thin: .thin
        case .light: .light
        case .medium: .medium
        case .semibold: .semibold
        case .bold: .bold
        case .heavy: .heavy
        case .black: .black
        default: .regular
        }
    }

    /// A short line of the family's own name set in itself, for a picker row or a preview.
    static func sample(family: String, size: CGFloat) -> Font {
        guard !family.isEmpty, let font = resolve(family, size: size, weight: .regular) else {
            return .system(size: size)
        }
        return Font(font)
    }
}

extension Font {
    /// Chrome text. `rounded` keeps the app's display cut for headings while the system font is in
    /// use, and drops it once a family is chosen — a user who picked a font meant to see that font,
    /// and `rounded` is a property of the system face rather than a modifier that can be applied to
    /// someone else's.
    static func ui(_ size: CGFloat, weight: Font.Weight? = nil, rounded: Bool = false) -> Font {
        FontChoice.ui(size: size, weight: weight, rounded: rounded)
    }

    /// Code text: the SQL editor, the result grid, and monospaced values in panels.
    static func code(_ size: CGFloat, weight: Font.Weight? = nil) -> Font {
        FontChoice.code(size: size, weight: weight)
    }
}
