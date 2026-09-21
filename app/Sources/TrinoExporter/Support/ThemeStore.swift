import AppKit
import SwiftUI

/// Which appearance the app draws in: the system's, or one pinned by the user.
///
/// This is separate from `AppTheme` on purpose. The mode decides light vs dark; the theme decides
/// *which* light or dark. `system` follows macOS and keeps following it — changing the system
/// appearance while the app is open repaints it, because AppKit re-resolves every dynamic colour
/// (see `Tone.ink`) when the effective appearance changes.
enum AppearanceMode: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: "System"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    var detail: String {
        switch self {
        case .system: "Follow macOS, and keep following it when the system switches."
        case .light: "Always light, whatever the system is set to."
        case .dark: "Always dark, whatever the system is set to."
        }
    }

    /// What to hand `preferredColorScheme`. `nil` means "no opinion", which is what makes SwiftUI
    /// inherit the system's appearance — and, crucially, keep inheriting it live.
    var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

/// The canvas a theme paints behind everything.
///
/// Each theme is a **pair** of canvases, one per appearance, because a single value cannot serve
/// both. The chrome layer is white at low opacity — `Tone.ink.opacity(0.07)` hairlines, body text,
/// recessed fields — across ten view files, and those values are only legible on something dark.
/// Rather than rewrite all of them, the chrome is expressed through `Tone.ink`/`Tone.recess`,
/// which are *dynamic* colours: AppKit hands back white in a dark appearance and black in a light
/// one, resolved per draw, so every one of those call sites is already correct in both.
enum AppTheme: String, CaseIterable, Identifiable {
    case midnight
    case graphite
    case nord
    case ink
    case daylight
    case cloud
    case paper

    var id: String { rawValue }

    /// Which appearance this theme belongs to, so Settings can offer the right ones for the mode.
    var isDark: Bool {
        switch self {
        case .midnight, .graphite, .nord, .ink: true
        case .daylight, .cloud, .paper: false
        }
    }

    var title: String {
        switch self {
        case .midnight: "Midnight"
        case .graphite: "Graphite"
        case .nord: "Nord"
        case .ink: "Ink"
        case .daylight: "Daylight"
        case .cloud: "Cloud"
        case .paper: "Paper"
        }
    }

    var detail: String {
        switch self {
        case .midnight: "The near-black indigo this app shipped with."
        case .graphite: "Neutral charcoal with no colour cast."
        case .nord: "The Nord polar-night slate, a touch lighter than the rest."
        case .ink: "Almost pure black, for an OLED display in a dark room."
        case .daylight: "The light counterpart to Midnight: a cool off-white with a blue cast."
        case .cloud: "A neutral light grey with no colour cast. The light counterpart to Graphite."
        case .paper: "A warm neutral white, easier on the eye than a pure #FFFFFF."
        }
    }

    /// The canvas behind everything. Read through `Tone.canvas`, so a change here repaints every
    /// surface at once — including the glass, which tints itself with this colour.
    var canvas: Color {
        switch self {
        case .midnight: Color(hex: 0x0A0B1E)
        // Measured off a reference screenshot rather than eyeballed: its content area is exactly
        // #151517, a neutral with no hue at all.
        case .graphite: Color(hex: 0x151517)
        case .nord: Color(hex: 0x2E3440)
        case .ink: Color(hex: 0x060709)
        // Not #FFFFFF: a pure-white canvas makes every 1pt hairline invisible and turns the frosted
        // panels into grey rectangles. Both lights sit slightly off-white, as macOS's own light
        // windows do, so the chrome has somewhere to be.
        case .daylight: Color(hex: 0xF4F6FB)
        case .cloud: Color(hex: 0xF5F5F7)
        case .paper: Color(hex: 0xFAF8F4)
        }
    }
}

/// The glow → deep pair the app's gradients are built from.
///
/// Only brand colours are offered. `amber`, `coral` and `mint` are deliberately absent even though
/// they are perfectly good colours: they are this app's **status** vocabulary — failure, destroy,
/// success — and an accent that duplicates one of them would make the primary Run capsule look
/// like the destructive Replace button.
enum AccentChoice: String, CaseIterable, Identifiable {
    case ice
    case violet
    case magenta
    case mint
    case blue

    var id: String { rawValue }

    var title: String {
        switch self {
        case .ice: "Ice"
        case .violet: "Violet"
        case .magenta: "Magenta"
        case .mint: "Mint"
        case .blue: "Blue"
        }
    }

    /// The lit half of the pair: `Tone.accent`, the glow behind the workspace and the top of every
    /// gradient.
    var glow: Color {
        switch self {
        case .ice: Color(hex: 0x4FD8FF)
        case .violet: Color(hex: 0x7B61FF)
        case .magenta: Color(hex: 0xFF4FA3)
        case .mint: Color(hex: 0x3EE6A8)
        // Measured off the reference screenshot's primary button. Deliberately not the same value
        // as `Tone.blue`, which is the fixed "html" format tint: this one is chrome and follows
        // the user, that one is a category and never moves.
        case .blue: Color(hex: 0x679EFD)
        }
    }

    /// The deep half: `Tone.accentDeep`, the bottom of every gradient.
    var deep: Color {
        switch self {
        case .ice: Color(hex: 0x7B61FF)
        case .violet: Color(hex: 0x4B2FD6)
        case .magenta: Color(hex: 0x7B61FF)
        case .mint: Color(hex: 0x12A886)
        case .blue: Color(hex: 0x3B6FD4)
        }
    }
}

/// How the app's coloured surfaces are painted.
///
/// Orthogonal to the theme and the accent: those choose *which* colours, this chooses *how* they
/// are laid down. It exists because a two-colour gradient behind every primary action is a strong
/// opinion, and there are rooms — a shared screen, a projector, a screenshot in a report — where
/// the app should state its accent and stop.
///
/// `glow` is the only tone that draws a gradient anywhere. The other two are flat by construction
/// rather than by a contrast setting: they swap the fill, drop the sheen, drop the coloured shadow,
/// and flatten the backdrop's radial glows, which are gradients themselves.
enum SurfaceTone: String, CaseIterable, Identifiable {
    /// The gradient capsule, the sheen, the coloured glow, the lit backdrop. What shipped.
    case glow
    /// One flat colour per surface. No gradient, no sheen, no glow.
    case plain
    /// A faint wash of the accent inside an accent border. The quietest of the three.
    case soft

    var id: String { rawValue }

    var title: String {
        switch self {
        case .glow: "Glow"
        case .plain: "Plain"
        case .soft: "Soft"
        }
    }

    var detail: String {
        switch self {
        case .glow: "Gradient fills, a sheen, a coloured glow behind the primary action, and a lit backdrop."
        case .plain: "One flat colour per surface. No gradient, no sheen, no glow."
        case .soft: "A faint wash of the accent in an accent border. No gradient, no glow."
        }
    }

    /// Whether the sheen, the coloured shadow and the backdrop's radial glows are drawn. All three
    /// are gradients, so this is exactly the question "may this tone draw a gradient".
    var isLuminous: Bool { self == .glow }
}

/// A named canvas, accent and tone, for the combinations worth reaching in one click.
///
/// The tiles and swatches below are the whole control surface — every theme takes every accent and
/// every tone — but three pairings are not arbitrary: the app's own look, a neutral charcoal with
/// blue, and the same charcoal with every gradient switched off, which is the one people ask for
/// by name.
struct ThemePreset: Identifiable {
    let id: String
    let title: String
    let detail: String
    /// A preset carries a canvas for **each** appearance, not one. A single canvas cannot serve
    /// both — Classic on a light canvas is Daylight, not Midnight — and without the pair, applying
    /// a preset in light mode would write to the dark half and change nothing on screen.
    let darkTheme: AppTheme
    let lightTheme: AppTheme
    let accent: AccentChoice
    let tone: SurfaceTone

    /// The canvas this preset would paint in the given appearance.
    func theme(isDark: Bool) -> AppTheme { isDark ? darkTheme : lightTheme }

    static let all: [ThemePreset] = [
        ThemePreset(id: "classic", title: "Classic",
                    detail: "Midnight in the dark, Daylight in the light — the app's own look — ice accent, gradient fills.",
                    darkTheme: .midnight, lightTheme: .daylight, accent: .ice, tone: .glow),
        ThemePreset(id: "slate", title: "Slate",
                    detail: "Neutral charcoal in the dark, Cloud in the light, blue accent, gradient fills.",
                    darkTheme: .graphite, lightTheme: .cloud, accent: .blue, tone: .glow),
        ThemePreset(id: "flat", title: "Flat",
                    detail: "Slate with every gradient switched off: flat fills, no sheen, no glow, flat backdrop.",
                    darkTheme: .graphite, lightTheme: .cloud, accent: .blue, tone: .plain),
    ]
}

/// The chosen appearance, remembered across launches.
///
/// A singleton because the palette is read through `Tone`'s static members from ~145 call sites in
/// ten files — `Tone.accent` cannot reach an environment value. That is also what makes this cheap:
/// `Tone.canvas` and `Tone.accent` are computed properties that read this store, and SwiftUI's
/// observation tracking registers that read while a view body runs, so changing a theme repaints
/// every view that uses the palette without a single call site being touched.
///
/// `@Observable` is what carries that tracking across the static indirection; a plain value in
/// `UserDefaults` would persist the choice and never redraw.
@Observable
final class ThemeStore {
    static let shared = ThemeStore()

    private static let themeKey = "appTheme"
    private static let accentKey = "accentChoice"
    private static let glowKey = "glowIntensity"
    private static let toneKey = "surfaceTone"
    private static let modeKey = "appearanceMode"
    private static let lightThemeKey = "lightTheme"

    /// While true, changes are held in memory and never written to `UserDefaults`. Set by
    /// `pin(theme:accent:)` for a render that must not rewrite the user's preferences — an icon
    /// build or a `--snapshot` run, both of which read the palette like any other view.
    private var isPinned = false

    // The stored values. Private so that every write goes through the public property and is
    // therefore subject to the pin; a snapshot run that wrote straight to `storedTheme` would
    // silently persist.
    // The theme is stored as a pair, one per appearance, so switching Light/Dark keeps a choice
    // for each instead of overwriting one with the other. A single value would mean "switch to
    // light" silently threw away the dark theme the user had picked.
    private var storedDarkTheme: AppTheme = {
        let raw = UserDefaults.standard.string(forKey: themeKey) ?? ""
        return AppTheme(rawValue: raw).flatMap { $0.isDark ? $0 : nil } ?? .midnight
    }()
    private var storedLightTheme: AppTheme = {
        let raw = UserDefaults.standard.string(forKey: lightThemeKey) ?? ""
        return AppTheme(rawValue: raw).flatMap { $0.isDark ? nil : $0 } ?? .daylight
    }()
    private var storedMode: AppearanceMode = {
        let raw = UserDefaults.standard.string(forKey: modeKey) ?? ""
        return AppearanceMode(rawValue: raw) ?? .system
    }()
    private var storedAccent: AccentChoice = {
        let raw = UserDefaults.standard.string(forKey: accentKey) ?? ""
        return AccentChoice(rawValue: raw) ?? .ice
    }()
    private var storedGlow: Double = {
        UserDefaults.standard.object(forKey: glowKey) as? Double ?? 1.0
    }()
    private var storedTone: SurfaceTone = {
        let raw = UserDefaults.standard.string(forKey: toneKey) ?? ""
        return SurfaceTone(rawValue: raw) ?? .glow
    }()

    /// Whether the app follows the system's appearance or pins one.
    var mode: AppearanceMode {
        get { storedMode }
        set { storedMode = newValue; persist() }
    }

    /// Which canvas, for the appearance currently in effect.
    ///
    /// Setting it writes to whichever half that appearance owns, so "Light" and "Dark" each keep
    /// their own theme. A theme of the wrong kind is redirected to that appearance's default rather
    /// than accepted — otherwise picking Midnight while in light mode would paint a near-black
    /// canvas under black text.
    var theme: AppTheme {
        get { isDarkAppearance ? storedDarkTheme : storedLightTheme }
        set {
            if newValue.isDark { storedDarkTheme = newValue } else { storedLightTheme = newValue }
            persist()
        }
    }

    /// The theme for each appearance, for Settings to show both halves at once.
    var darkTheme: AppTheme {
        get { storedDarkTheme }
        set { storedDarkTheme = newValue; persist() }
    }

    var lightTheme: AppTheme {
        get { storedLightTheme }
        set { storedLightTheme = newValue; persist() }
    }

    /// Whether the appearance in effect is dark, which is what decides which theme applies.
    ///
    /// Read from the store's own view of the mode rather than from AppKit, because this is asked
    /// during a view body: a `system` mode has to answer with the *system's* current appearance, and
    /// `ThemeStore.resolvedIsDark` is kept up to date from AppKit's effective appearance.
    var isDarkAppearance: Bool {
        switch mode {
        case .dark: true
        case .light: false
        case .system: systemIsDark
        }
    }

    /// The system's appearance, kept current by `AppearanceObserver`.
    var systemIsDark = true

    var accent: AccentChoice {
        get { storedAccent }
        set { storedAccent = newValue; persist() }
    }

    /// How hard the backdrop's two radial glows burn, as a multiplier on the design's own
    /// opacities: 1.0 is what shipped, 0 is a flat canvas, 1.5 is the loudest this goes before the
    /// glow starts washing out the text sitting on it.
    var glow: Double {
        get { storedGlow }
        set { storedGlow = newValue; persist() }
    }

    /// How the coloured surfaces are painted — gradient or flat. Independent of the theme and the
    /// accent, so "charcoal, blue, flat" is a thing a user can ask for.
    var tone: SurfaceTone {
        get { storedTone }
        set { storedTone = newValue; persist() }
    }

    private func persist() {
        guard !isPinned else { return }
        UserDefaults.standard.set(storedDarkTheme.rawValue, forKey: Self.themeKey)
        UserDefaults.standard.set(storedLightTheme.rawValue, forKey: Self.lightThemeKey)
        UserDefaults.standard.set(storedAccent.rawValue, forKey: Self.accentKey)
        UserDefaults.standard.set(storedGlow, forKey: Self.glowKey)
        UserDefaults.standard.set(storedTone.rawValue, forKey: Self.toneKey)
        UserDefaults.standard.set(storedMode.rawValue, forKey: Self.modeKey)
    }

    /// The backdrop opacity for a glow the design drew at `base`, scaled by the user's choice and
    /// by how much glow the current appearance can carry.
    ///
    /// The light factor is not a taste call. A radial glow is a *dark-canvas* idiom: on near-black
    /// it reads as light spilling from a corner, and the same gradient over off-white reads as a
    /// stain — measured on Daylight, the accent at the design's own 0.30 laid a visibly dirty wash
    /// across the lower half of the window. Light does not need to be added to a light canvas, so
    /// the glow is kept only as a faint tint that says which accent is in use.
    func lit(_ base: Double) -> Double {
        base * glow * (isDarkAppearance ? 1.0 : Self.lightGlowFactor)
    }

    /// How much of the dark backdrop's glow survives on a light canvas.
    private static let lightGlowFactor = 0.28

    /// Put the palette back where it started, for the Settings pane's Reset.
    func reset() {
        mode = .system
        darkTheme = .midnight
        lightTheme = .daylight
        accent = .ice
        glow = 1.0
        tone = .glow
    }

    /// Apply a named pair in one step. Goes through the same setters, so it persists the same way
    /// and is held back by the same pin.
    func apply(_ preset: ThemePreset) {
        // Both halves, so switching Light/Dark afterwards still lands on this preset's canvas for
        // that appearance rather than on whatever the other half happened to hold.
        darkTheme = preset.darkTheme
        lightTheme = preset.lightTheme
        accent = preset.accent
        tone = preset.tone
    }

    /// Whether the current pair is exactly this preset, so Settings can mark it as the active one.
    func matches(_ preset: ThemePreset) -> Bool {
        theme == preset.theme(isDark: isDarkAppearance) && accent == preset.accent && tone == preset.tone
    }

    /// Fix the palette for a render that must not depend on the user's preferences, and must not
    /// change them either. Used by `--snapshot --theme`, which needs to draw a theme the user has
    /// not chosen without leaving that choice behind in `UserDefaults`.
    /// Every argument is optional, and every one is written **in memory only**.
    ///
    /// `isPinned` is raised first, before anything is assigned. That ordering is the whole point:
    /// the public setters persist, so a caller that wanted to *review* an appearance had to be able
    /// to hand its values to something that never writes. A `--snapshot` run that set `mode` through
    /// the public property wrote `appearanceMode`, `appTheme`, `accentChoice`, `surfaceTone` and
    /// `lightTheme` straight into the user's preferences — the exact thing this method promises not
    /// to do.
    func pin(theme: AppTheme? = nil, accent: AccentChoice? = nil, tone: SurfaceTone? = nil,
             mode: AppearanceMode? = nil, systemIsDark: Bool? = nil) {
        isPinned = true
        // A snapshot render has no real window, so it cannot ask AppKit which appearance is in
        // effect; the caller states it. This is what lets `--mode system --system-appearance light`
        // draw the light theme without the machine actually being in light mode.
        if let systemIsDark { self.systemIsDark = systemIsDark }
        // Mode before theme: the theme setter picks a half from the appearance in effect, so the
        // mode has to be in place first or a light request would land in the dark slot.
        if let mode { storedMode = mode }
        if let theme {
            if theme.isDark { storedDarkTheme = theme } else { storedLightTheme = theme }
        }
        if let accent { storedAccent = accent }
        if let tone { storedTone = tone }
    }
}
