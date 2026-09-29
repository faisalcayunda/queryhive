import SwiftUI

// Design language modelled on CleanMyMac: a near-black canvas lit by two glowing module
// colours, frosted glass surfaces, rounded type, and gradient accents on every action.
//
// The *layout* on top of this language is Navicat's, not CleanMyMac's: an object tree down the
// left, a tabbed query workspace, an editor over a panel you can drag, a toolbar, and a status
// bar. CleanMyMac supplies the surfaces and the colour; Navicat supplies the furniture.
//
// QueryHive is the sibling of Iceberg Tools and shares its palette deliberately. Only the mark
// differs -- an iceberg is a berg on water, QueryHive is a comb of cells with one lit.

// Two kinds of colour live here, and the split is the whole design of the theming:
//
//   * `accent` / `accentDeep` / `canvas` are **computed** and read `ThemeStore`, so they follow the
//     user's choice. They are read from ~145 call sites in ten files, most inside a view body;
//     SwiftUI's observation tracking registers the store read that happens while that body runs,
//     so changing a theme repaints every one of them. Stored `let`s would bake the palette in at
//     first access and Settings would appear to do nothing.
//
//   * Everything else is a fixed `let`. These are the app's *meaning* colours, and they must not
//     follow the accent. `ice`, `mint`, `amber`, `violet`, `coral` and `blue` are used as a
//     distinguishable *set* — the five suggestion kinds, the nine format tints, the four column
//     types, the three connection states. If `ice` became the accent, picking Mint would paint a
//     table icon and a column icon the same green, and the set would stop carrying information.
//     A theme may repaint the chrome; it may not collapse a vocabulary.
enum Tone {
    /// The near-black behind everything, from the chosen theme. `glass` and every panel tint
    /// themselves with this, so one value repaints the whole shell.
    static var canvas: Color { ThemeStore.shared.theme.canvas }

    /// The interactive colour: focus rings, selected tabs and tiles, toggle on-states, checkmarks,
    /// the Run capsule. Follows the user's accent.
    static var accent: Color { ThemeStore.shared.accent.glow }

    /// The deep end of every accent gradient. Follows the accent, one step behind `accent`.
    static var accentDeep: Color { ThemeStore.shared.accent.deep }

    // MARK: The mark

    // The comb's own two colours, and the one pair a theme cannot move.
    //
    // `accent` began as this pair, so it is tempting to let the comb follow the accent. It must
    // not: the comb *is* the app's identity — it is what `make-icon.sh` bakes into
    // `assets/icon.icns`, a fixed file no user setting can reach — and a mark that is ice on one
    // Mac and mint on another is not a mark. The chrome follows the accent; the logo does not.
    static let brandGlow = Color(hex: 0x4FD8FF)
    static let brandDeep = Color(hex: 0x7B61FF)

    /// The icon tile's two ends, and the light along its top edge.
    ///
    /// The tile is dark and the mark on it is white, because the bright cyan-to-violet tile put
    /// white at **1.67:1** against its lightest corner — under the 3:1 non-text minimum, with the
    /// comb legible only thanks to the shadow under it. Dark ends take the same white to 10.1:1 and
    /// 18.1:1. These are only for the icon: they are fixed values like `brandGlow`/`brandDeep`, not
    /// something the accent can reach.
    static let brandTileTop = Color(hex: 0x3B2AA8)
    static let brandTileBase = Color(hex: 0x12103A)
    /// The rim along the tile's top edge. On a dark Dock the near-black tile is 1.06:1 against its
    /// background, and this is what gives it an edge (13.4:1).
    static let brandRim = Color(hex: 0xA8F0FF)

    // MARK: The fixed vocabulary

    /// The app's cyan. A *categorical* colour, not the accent: it is the "column" suggestion, the
    /// "txt" format, the "append" write mode, the database icon, and the "connecting" dot — every
    /// one of them a member of a set whose members have to stay tellable apart.
    static let ice = Color(hex: 0x4FD8FF)
    static let violet = Color(hex: 0x7B61FF)
    static let magenta = Color(hex: 0xFF4FA3)
    static let mint = Color(hex: 0x3EE6A8)
    static let amber = Color(hex: 0xFFB547)
    static let coral = Color(hex: 0xFF5E6C)
    static let gray = Color(hex: 0x9AA0A8)
    static let blue = Color(hex: 0x4F8DFF)

    // MARK: Appearance-adaptive chrome

    /// The chrome's "ink": white on a dark appearance, black on a light one.
    ///
    /// This is the whole trick that makes light mode possible without rewriting the ~119 chrome
    /// call sites. Those sites say `Tone.ink.opacity(0.07)` for a hairline, `Tone.ink.opacity(0.9)` for
    /// body text, and so on — values that are correct on near-black and invisible on off-white.
    /// `Tone.ink` is a **dynamic** colour: AppKit resolves it per draw against the appearance in
    /// effect, so `Tone.ink.opacity(0.07)` is a white hairline in the dark and a black one in the light,
    /// and every existing opacity keeps meaning what it meant.
    ///
    /// Resolved by AppKit rather than tracked in Swift code, which is what makes it correct under
    /// `AppearanceMode.system` too: when the user switches macOS appearance, AppKit re-resolves
    /// every dynamic colour and redraws, with no observer and no plumbing here.
    static var ink: Color { Color(nsColor: NSColor(name: nil) { $0.isDark ? .white : .black }) }

    /// The chrome's "recess": a translucent black on a dark appearance, a translucent white on a
    /// light one. The counterpart to `ink`, for the fields and wells that sit *into* the canvas
    /// rather than on top of it — `Tone.recess.opacity(0.30)` today.
    static var recess: Color { Color(nsColor: NSColor(name: nil) { $0.isDark ? .black : .white }) }

    /// Secondary body text: the chrome's ink at 68%, which is what `Tone.secondary` has always been.
    static var secondary: Color { ink.opacity(0.68) }

    /// The quiet readouts: the editor's line numbers and the line count in its corner.
    ///
    /// One colour for both, because they sit inches apart saying the same thing and drifted apart
    /// the moment they were written separately: the gutter was drawn at 0.55 and the count at 0.374,
    /// so the count looked washed out beside the number it was reporting.
    ///
    /// The gutter also borrowed AppKit's `secondaryLabelColor`. Measured, that resolves to white at
    /// 0.55 on a dark appearance and black at 0.55 on a light one — the same value this writes out.
    /// It is stated here anyway because the system's label grey is the system's to change, and a
    /// themed app whose chrome is one palette should not have one readout resolved by somebody
    /// else's.
    static var readout: Color { ink.opacity(readoutOpacity) }

    /// The same colour for AppKit.
    static var readoutNS: NSColor { inkNS(readoutOpacity) }

    /// `Tone.ink` at an opacity, for the AppKit drawing that cannot take a SwiftUI `Color`.
    ///
    /// Resolved per appearance rather than converted once: `NSColor(Color)` snapshots whichever
    /// appearance happened to be current, so a ruler painted that way keeps its grey after the
    /// appearance flips.
    static func inkNS(_ opacity: CGFloat) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.isDark ? NSColor.white.withAlphaComponent(opacity)
                              : NSColor.black.withAlphaComponent(opacity)
        }
    }

    /// Written once and read by both, so the two readouts cannot drift apart again.
    private static let readoutOpacity: CGFloat = 0.55
}

extension NSAppearance {
    /// Whether this appearance is one of the dark ones, asked through `bestMatch` because a view's
    /// appearance can be a vibrancy or high-contrast variant rather than plain `.darkAqua`.
    var isDark: Bool { bestMatch(from: [.aqua, .darkAqua]) == .darkAqua }
}

extension Color {
    init(hex: UInt32) {
        self.init(red: Double(hex >> 16 & 0xFF) / 255, green: Double(hex >> 8 & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
    }
}

extension Font {
    // Routed through the seam like every other text run, so a chosen family reaches the hero
    // titles and the monospaced values too. `heroTitle`/`cardTitle` keep their rounded cut while
    // the system font is in use and drop it once a family is chosen — see `Font.ui`.
    static var heroTitle: Font { .ui(30, weight: .bold, rounded: true) }
    static var cardTitle: Font { .ui(14, weight: .semibold, rounded: true) }
    static var body13: Font { .ui(13) }
    static var mono12: Font { .code(12) }
    static var mono13: Font { .code(13) }
}

/// The colour pair of a module or result: the backdrop glows and every accent derive from it.
struct Hue {
    let glow: Color
    let accent: Color

    private var tone: SurfaceTone { ThemeStore.shared.tone }

    /// The fill for a coloured surface: a gradient under `glow`, a single flat colour under `plain`
    /// and `soft`.
    ///
    /// This is where the tone is enforced, rather than at each call site. Every primary action,
    /// driver tile and empty-state illustration fills itself with `hue.gradient`, so returning a
    /// flat `LinearGradient` of one colour — not a `Color` — keeps all of them compiling while
    /// removing the ramp. `LinearGradient` of a single colour renders as that colour, which is why
    /// the return type does not have to change.
    var gradient: LinearGradient {
        switch tone {
        case .glow:
            LinearGradient(colors: [glow, accent], startPoint: .topLeading, endPoint: .bottomTrailing)
        case .plain:
            LinearGradient(colors: [accent, accent], startPoint: .top, endPoint: .bottom)
        case .soft:
            // The wash, not the full colour: `soft` states the accent without filling with it.
            LinearGradient(colors: [glow.opacity(0.22), glow.opacity(0.22)], startPoint: .top, endPoint: .bottom)
        }
    }

    /// The colour an accent border or label should use under the current tone. `soft` keeps the
    /// accent; `plain` uses it at full strength.
    var stroke: Color { tone == .soft ? glow.opacity(0.55) : glow }

    /// Whether the sheen overlay, the coloured drop shadow and the halo are drawn. They are all
    /// gradients, so a flat tone drops every one of them.
    var isLuminous: Bool { tone.isLuminous }

    /// Computed, not a `static let`: this pair is the user's accent, and a stored constant would
    /// be evaluated once at first use and then keep the old colour for the life of the process.
    static var exporter: Hue { Hue(glow: Tone.accent, accent: Tone.accentDeep) }
    /// Connections keep their own fixed magenta → violet. It is how the second module is told apart
    /// from the first at a glance, so it must not become a second copy of the accent.
    static let connection = Hue(glow: Tone.magenta, accent: Tone.violet)
    static let success = Hue(glow: Tone.mint, accent: Color(hex: 0x12A886))
    static let failure = Hue(glow: Tone.amber, accent: Tone.coral)
}

/// The workspace backdrop: one radial glow per module colour, drawn behind the panels so the
/// glass has something to sit on. Radial gradients, never `.blur` (a snapshot capture does not
/// render it) and far cheaper than a 190pt blur.
///
/// The two opacities below are the design's own 0.30 / 0.22 scaled by the user's glow setting, so
/// at the default the drawing is byte-for-byte what shipped.
struct Backdrop: View {
    let hue: Hue
    private var store: ThemeStore { .shared }

    var body: some View {
        ZStack {
            Tone.canvas
            // The two radial glows are gradients, so a flat tone drops them entirely rather than
            // merely dimming them — "no gradient" has to mean no gradient, and a faint radial wash
            // is exactly the thing being switched off.
            if store.tone.isLuminous {
                RadialGradient(colors: [hue.glow, .clear], center: .center, startRadius: 0, endRadius: 340)
                    .frame(width: 680, height: 680)
                    .opacity(store.lit(0.30))
                    .offset(x: 90, y: -280)
                RadialGradient(colors: [hue.accent, .clear], center: .center, startRadius: 0, endRadius: 280)
                    .frame(width: 560, height: 560)
                    .opacity(store.lit(0.22))
                    .offset(x: -260, y: 320)
            }
        }
        .animation(.easeInOut(duration: 0.8), value: hue.glow)
        .ignoresSafeArea()
    }
}

extension View {
    /// Frosted panel surface: ultraThinMaterial tinted with the canvas colour plus a soft top
    /// light, matched to CleanMyMac's glass rather than a flat white wash. `tint` overrides the
    /// border colour (used to show which connection a panel belongs to).
    func glass(_ radius: CGFloat = 14, tint: Color? = nil, tintOpacity: Double = 0.45, lineWidth: CGFloat = 1) -> some View {
        background {
            RoundedRectangle(cornerRadius: radius, style: .continuous).fill(.ultraThinMaterial)
            RoundedRectangle(cornerRadius: radius, style: .continuous).fill(Tone.canvas.opacity(0.45))
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(LinearGradient(colors: [.white.opacity(0.10), .clear], startPoint: .top, endPoint: .center))
        }
        .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous)
            .strokeBorder(tint?.opacity(tintOpacity) ?? Tone.ink.opacity(0.12), lineWidth: lineWidth))
    }

    func field(invalid: Bool = false) -> some View {
        textFieldStyle(.plain)
            .font(.body13)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(invalid ? Tone.coral.opacity(0.8) : Tone.ink.opacity(0.1), lineWidth: invalid ? 1.5 : 1))
    }

    /// A toolbar-height recessed control (the folder chip, the output-name field).
    func toolbarField() -> some View {
        textFieldStyle(.plain)
            .font(.ui(12))
            .padding(.horizontal, 9)
            .frame(height: 28)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Tone.ink.opacity(0.10)))
    }

    /// The SQL editor's inset: recessed black, block-shaped, and focus shown with the module
    /// accent rather than the system focus ring.
    func editorBox(focused: Bool) -> some View {
        background(Tone.recess.opacity(0.34), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(focused ? Tone.accent.opacity(0.45) : Tone.ink.opacity(0.08), lineWidth: 1))
    }

    /// Hairline panel edge used between the sidebar, the toolbar and the panel.
    func panelEdge(_ edge: Edge) -> some View {
        overlay(alignment: edge == .trailing ? .trailing : edge == .leading ? .leading : edge == .top ? .top : .bottom) {
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: edge == .leading || edge == .trailing ? 1 : nil,
                                                         height: edge == .top || edge == .bottom ? 1 : nil)
        }
    }
}

/// The one spacing scale the whole shell measures from. Every pane's left edge landing on the
/// same vertical grid is most of what makes a window look deliberate rather than assembled.
enum Metrics {
    /// Horizontal gutter for every pane.
    static let gutter: CGFloat = 12

    /// Just the traffic lights plus their breathing room: the strip no longer holds a wordmark.
    static let titleStrip: CGFloat = 40
    static let tabStrip: CGFloat = 36
    static let toolbar: CGFloat = 46
    // There was a `paneHeader: 32` here, for the row above the SQL editor. That row is gone — the
    // editor draws its own clear button and line count over its text — and a spacing constant
    // nothing measures from is worse than no constant, because the next pane would align to a
    // height that no longer exists anywhere in the window.
    static let panelTabs: CGFloat = 30
    static let statusBar: CGFloat = 26

    /// Height of a toolbar control: Run, the connection picker, the format and folder chips.
    static let control: CGFloat = 28
    static let treeRow: CGFloat = 23
    /// One indent step in the object tree.
    static let treeIndent: CGFloat = 15
}

// MARK: Actions

/// The primary action of a Navicat-style toolbar wearing CleanMyMac's clothes: a gradient
/// capsule with a sheen, a coloured glow and a press-scale. The round orb belongs to a
/// full-screen flow, and this app's flow is a toolbar.
struct HubButton: View {
    let title: String
    var symbol: String?
    var hue: Hue = .exporter
    let action: () -> Void
    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if let symbol { Image(systemName: symbol).font(.system(size: 11, weight: .bold)) }
                Text(title).font(.ui(13, weight: .semibold))
            }
            .foregroundStyle(enabled ? .white : Tone.ink.opacity(0.42))
            .padding(.horizontal, 14)
            .frame(height: 28)
            .background {
                // A disabled primary keeps its shape and drops its colour entirely. Fading the
                // gradient (opacity + desaturate) produced a muddy grey that read as "almost
                // available"; an empty capsule with dim text reads as "not yet", which is true.
                if enabled {
                    Capsule().fill(hue.gradient)
                    // The sheen is a gradient, so a flat tone drops it. Without this the capsule
                    // would keep a highlight ramp and "Plain" would only be half true.
                    if hue.isLuminous {
                        Capsule().fill(LinearGradient(colors: [.white.opacity(0.28), .clear],
                                                      startPoint: .top, endPoint: .center))
                    }
                } else {
                    Capsule().fill(Tone.ink.opacity(0.05))
                }
            }
            .overlay(Capsule().strokeBorder(enabled ? Tone.ink.opacity(0.25) : Tone.ink.opacity(0.10)))
            .shadow(color: hue.accent.opacity(hue.isLuminous && enabled ? (hovering ? 0.65 : 0.45) : 0),
                    radius: hovering ? 14 : 9, y: 3)
            .contentShape(Capsule())
        }
        .buttonStyle(PressScale())
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.18), value: hovering)
    }
}

/// Square icon button for the toolbar and the tree header.
struct IconButton: View {
    let symbol: String
    var tint: Color = Tone.ink
    var help: String = ""
    var diameter: CGFloat = 28
    let action: () -> Void
    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: diameter * 0.42, weight: .semibold))
                .foregroundStyle(enabled ? tint : tint.opacity(0.3))
                .frame(width: diameter, height: diameter)
                .background(Tone.ink.opacity(hovering && enabled ? 0.13 : 0.06),
                            in: RoundedRectangle(cornerRadius: diameter * 0.26, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: diameter * 0.26, style: .continuous)
                    .strokeBorder(Tone.ink.opacity(0.08)))
                .contentShape(RoundedRectangle(cornerRadius: diameter * 0.26, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .onHover { hovering = $0 }
        .help(help)
    }
}

struct PressScale: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.95 : 1)
            .animation(.spring(response: 0.25, dampingFraction: 0.6), value: configuration.isPressed)
    }
}

/// A secondary action: a glyph and a label on an outlined capsule.
///
/// These used to be one grey pill whose only variable was `tint`, which is why a header holding
/// "Load File…" and "Clear" read as two identical blobs with no way to tell which one mattered.
/// `role` supplies that hierarchy and the glyph makes each button scannable without reading it.
struct PillButton: View {
    enum Role {
        /// Next to a primary, or the committing half of a pair: Load File…, Cancel, Copy Name.
        case secondary
        /// An action with nothing at stake: Clear, Back. No fill until the pointer is on it.
        case quiet
        /// Removes something. Coral, always.
        case destructive
    }

    let title: String
    var symbol: String?
    var role: Role = .secondary
    /// For a 32pt header row, where a 28pt capsule would touch both edges.
    var compact = false
    let action: () -> Void

    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false

    private var tint: Color {
        switch role {
        case .secondary, .quiet: Tone.ink
        case .destructive: Tone.coral
        }
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if let symbol {
                    Image(systemName: symbol)
                        .font(.system(size: compact ? 9 : 10, weight: .semibold))
                        .opacity(0.8)
                }
                Text(title).font(.ui(compact ? 11.5 : 12.5, weight: .medium))
            }
            .foregroundStyle(tint.opacity(enabled ? 1 : 0.4))
            .padding(.horizontal, compact ? 10 : 13)
            .frame(height: compact ? 24 : 28)
            .background(fill, in: Capsule())
            .overlay(Capsule().strokeBorder(border))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    private var fill: Color {
        switch role {
        case .secondary: Tone.ink.opacity(hovering && enabled ? 0.16 : 0.10)
        case .quiet: Tone.ink.opacity(hovering && enabled ? 0.09 : 0)
        case .destructive: Tone.coral.opacity(hovering && enabled ? 0.18 : 0.09)
        }
    }

    private var border: Color {
        switch role {
        case .secondary: Tone.ink.opacity(0.14)
        case .quiet: Tone.ink.opacity(hovering && enabled ? 0.10 : 0.06)
        case .destructive: Tone.coral.opacity(0.45)
        }
    }
}

/// Vertical hairline between toolbar groups.
struct ToolbarSeparator: View {
    var body: some View {
        Rectangle().fill(Tone.ink.opacity(0.10)).frame(width: 1, height: 18).padding(.horizontal, 3)
    }
}

// MARK: Controls

struct Segmented<T: Hashable>: View {
    @Binding var selection: T
    let options: [T]
    let label: (T) -> String

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.self) { option in
                Button { selection = option } label: {
                    Text(label(option))
                        .font(.ui(12, weight: .medium))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                        .background(Tone.ink.opacity(selection == option ? 0.18 : 0), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .animation(.easeOut(duration: 0.15), value: selection)
    }
}

struct LabeledField<Content: View>: View {
    let label: String
    let content: Content

    init(_ label: String, @ViewBuilder content: () -> Content) {
        self.label = label
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label.uppercased())
                .font(.ui(11, weight: .semibold))
                .tracking(0.8)
                .foregroundStyle(Tone.secondary)
            content
        }
    }
}

/// Capsule switch for boolean format options (header row, BOM, jsonl, zip). A native Toggle
/// reads as a system control inside a dark glass panel; this keeps the module's vocabulary.
struct ChipToggle: View {
    let label: String
    @Binding var isOn: Bool
    @State private var hovering = false

    var body: some View {
        Button { isOn.toggle() } label: {
            HStack(spacing: 6) {
                Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(isOn ? Tone.accent : Tone.ink.opacity(0.35))
                Text(label).font(.ui(12))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(Tone.recess.opacity(0.24), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(isOn ? Tone.accent.opacity(0.45) : Tone.ink.opacity(hovering ? 0.18 : 0.09)))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: isOn)
    }
}

/// A bare checkbox, for sitting *beside* a text field rather than on its own row.
///
/// `ChipToggle` draws a dark chip around itself, which is right for a setting that owns its row and
/// wrong next to an input: two boxed controls side by side read as two fields. This one has no
/// background, so the eye sees a field and a modifier for it.
struct InlineCheckbox: View {
    let label: String
    @Binding var isOn: Bool
    @State private var hovering = false

    var body: some View {
        Button { isOn.toggle() } label: {
            HStack(spacing: 6) {
                Image(systemName: isOn ? "checkmark.square.fill" : "square")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(isOn ? Tone.accent : Tone.ink.opacity(hovering ? 0.55 : 0.35))
                Text(label)
                    .font(.ui(12))
                    .foregroundStyle(isOn ? Tone.ink.opacity(0.95) : Tone.secondary)
            }
            .padding(.vertical, 5)
            .padding(.horizontal, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(.easeOut(duration: 0.12), value: isOn)
    }
}

/// "1 column" / "2 columns", comma-grouped. Shared by every user-facing count in the app so
/// singular/plural never has to be spelled out at each call site.
func pluralized(_ n: Int, _ singular: String, _ plural: String? = nil) -> String {
    "\(n.formatted()) \(n == 1 ? singular : (plural ?? singular + "s"))"
}

/// Small monospaced capsule, tinted per use: the format badge, the connection scheme.
struct Chip: View {
    let text: String
    var tint: Color = Tone.ice

    var body: some View {
        Text(text)
            .font(.code(11, weight: .semibold))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.14), in: Capsule())
    }
}

/// A toolbar-width combo box: type a value, or pick one the object tree has already loaded.
///
/// A plain menu would be dishonest here — the tree is lazy, so on a freshly opened connection it
/// has no catalogs to offer at all. A plain text field would throw away what browsing already
/// discovered. Navicat's catalog/schema fields are editable dropdowns for the same reason.
struct ComboField: View {
    let placeholder: String
    @Binding var text: String
    let options: [String]
    var width: CGFloat = 116
    var monospaced = true
    /// A fetch is in flight for this field's options. Shown in place of the chevron so the field
    /// says "asking" rather than "there is nothing to choose from".
    var loading = false

    var body: some View {
        HStack(spacing: 0) {
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(monospaced ? .system(size: 12, design: .monospaced) : .system(size: 12))
                .padding(.horizontal, 9)
            if loading {
                ProgressView().controlSize(.mini).frame(width: 20)
            } else if !options.isEmpty {
                Menu {
                    ForEach(options, id: \.self) { option in
                        Button(option) { text = option }
                    }
                } label: {
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(Tone.secondary)
                        .frame(width: 20, height: Metrics.control)
                        .contentShape(Rectangle())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .frame(width: 20)
                .help("Pick from the objects the tree has loaded")
            }
        }
        .frame(width: width, height: Metrics.control)
        .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Tone.ink.opacity(0.10)))
    }
}

/// Uppercase section label, the one piece of chrome every Navicat pane header shares.
struct SectionLabel: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .font(.ui(10.5, weight: .semibold))
            .tracking(0.9)
            .foregroundStyle(Tone.secondary)
    }
}

// MARK: Illustrations

/// Glossy app-tile illustration. `halo` disables the radial glow behind the tile: the small
/// toolbar usage sits close to other chrome where the halo just adds noise.
struct SymbolHero: View {
    let symbol: String
    let hue: Hue
    var size: CGFloat = 170
    var halo: Bool = true

    var body: some View {
        let tile = RoundedRectangle(cornerRadius: size * 0.18, style: .continuous)
        ZStack {
            if halo && hue.isLuminous {
                Circle().fill(RadialGradient(colors: [hue.glow.opacity(0.35), .clear], center: .center, startRadius: 0, endRadius: size * 0.7))
            }
            tile.fill(hue.gradient)
                .overlay {
                    // The sheen and the coloured shadow are both gradients; a flat tone keeps the
                    // tile and its glyph and drops the gloss.
                    if hue.isLuminous {
                        tile.fill(LinearGradient(colors: [.white.opacity(0.4), .clear], startPoint: .top, endPoint: .center))
                    }
                }
                .overlay(tile.strokeBorder(hue.isLuminous ? Tone.ink.opacity(0.35) : hue.stroke.opacity(0.6), lineWidth: 1))
                .frame(width: size * 0.62, height: size * 0.62)
                .shadow(color: hue.accent.opacity(hue.isLuminous ? 0.6 : 0), radius: size * 0.14, y: size * 0.07)
            Image(systemName: symbol)
                .font(.system(size: size * 0.27, weight: .semibold))
                .foregroundStyle(.white)
                .shadow(color: .black.opacity(0.25), radius: 4, y: 2)
        }
        .frame(width: size, height: size)
    }
}

/// Seven hexagons on a flat-top lattice, laid out on a square canvas. The first entry is the
/// centre cell -- the query that got answered -- and the six around it are the rest of the
/// comb. Shared by the compact mark and the hero so the glyph can never drift between them.
///
/// The lattice extent is 5r across and 5.464r tall before the 0.94 inset, so the divisor has to
/// clear the *height*, not the width: 5.2 clips the top and bottom cells, 5.5 does not.
/// A flat-top regular hexagon, the one primitive every QueryHive mark is built from.
func hexagonPath(centre: CGPoint, radius: CGFloat) -> Path {
    var path = Path()
    for corner in 0..<6 {
        let angle = Double(corner) * Double.pi / 3
        let vertex = CGPoint(x: Double(centre.x) + Double(radius) * cos(angle),
                             y: Double(centre.y) + Double(radius) * sin(angle))
        if corner == 0 { path.move(to: vertex) } else { path.addLine(to: vertex) }
    }
    path.closeSubpath()
    return path
}

/// The seven cells of the hive mark: one lit centre and the six around it.
///
/// `inset` is the cell's radius as a fraction of the lattice radius, and it exists because the two
/// callers want different things from the same lattice. `HiveHero` draws the mark large and airy at
/// 0.94; the app icon needs the cells **separated**, and at 0.94 the gap between neighbours is
/// 0.104r — 0.57px at 64pt, which fuses the seven hexagons into one blob. Measured on the rendered
/// icon: at 0.94 the 64pt artwork resolves to a single connected component instead of seven.
/// Parameterised rather than changed in place so the in-app illustration is untouched.
func hiveCells(in size: CGSize, inset: CGFloat = 0.94) -> [(path: Path, lit: Bool)] {
    let radius = min(size.width, size.height) / 5.5
    let stepX = 1.5 * radius
    let stepY = sqrt(3.0) / 2 * radius
    let centre = CGPoint(x: size.width / 2, y: size.height / 2)

    let offsets: [(CGFloat, CGFloat)] = [
        (0, 0),
        (stepX, stepY), (stepX, -stepY),
        (-stepX, stepY), (-stepX, -stepY),
        (0, 2 * stepY), (0, -2 * stepY),
    ]
    return offsets.enumerated().map { index, offset in
        (hexagonPath(centre: CGPoint(x: centre.x + offset.0, y: centre.y + offset.1),
                     radius: radius * inset), index == 0)
    }
}

/// Static comb for compact placements (the title strip, 18pt). At that size seven cells turn to
/// mush, so the mark is a single cell: a filled hexagon carrying a hollow one inside it.
struct HiveMark: View {
    var size: CGFloat = 18

    var body: some View {
        Canvas { context, canvasSize in
            let radius = min(canvasSize.width, canvasSize.height) / 2 * 0.98
            let centre = CGPoint(x: canvasSize.width / 2, y: canvasSize.height / 2)

            // The mark keeps its own two colours under every theme, but a flat tone still drops
            // the ramp between them: "no gradient" is about how a surface is painted, not about
            // which colour the brand is.
            let brand: [Color] = ThemeStore.shared.tone.isLuminous
                ? [Tone.brandGlow, Tone.brandDeep]
                : [Tone.brandGlow, Tone.brandGlow]
            context.fill(hexagonPath(centre: centre, radius: radius), with: .linearGradient(
                Gradient(colors: brand),
                startPoint: CGPoint(x: 0, y: 0),
                endPoint: CGPoint(x: canvasSize.width, y: canvasSize.height)))
            context.stroke(hexagonPath(centre: centre, radius: radius * 0.46),
                           with: .color(Tone.ink.opacity(0.92)), lineWidth: max(1, size * 0.075))
        }
        .frame(width: size, height: size)
    }
}

/// The comb at hero scale, for the workspace's empty state: lit centre cell, six dim neighbours,
/// a glow behind it, and a slow bob that keeps it from reading as a frozen screenshot.
///
/// Drawn in `brandGlow`/`brandDeep` rather than the accent, because this is the same glyph
/// `HiveMark` draws and `make-icon.sh` bakes into the app icon. The mark is the app's identity, so
/// it is the one thing on screen that a theme choice must not move.
struct HiveHero: View {
    var size: CGFloat = 200
    @State private var bob = false

    var body: some View {
        let radius = size / 5.5
        ZStack {
            if ThemeStore.shared.tone.isLuminous {
                Circle().fill(RadialGradient(colors: [Tone.brandGlow.opacity(0.30), .clear], center: .center, startRadius: 0, endRadius: size * 0.55))
            }
            Canvas { context, canvasSize in
                let start = CGPoint(x: 0, y: 0)
                let end = CGPoint(x: canvasSize.width, y: canvasSize.height)
                let lit = ThemeStore.shared.tone.isLuminous
                let litPair: [Color] = lit ? [Tone.brandGlow, Tone.brandDeep] : [Tone.brandGlow, Tone.brandGlow]
                let dimPair: [Color] = lit
                    ? [Tone.brandGlow.opacity(0.20), Tone.brandDeep.opacity(0.10)]
                    : [Tone.brandGlow.opacity(0.16), Tone.brandGlow.opacity(0.16)]
                for cell in hiveCells(in: canvasSize) {
                    if cell.lit {
                        context.fill(cell.path, with: .linearGradient(
                            Gradient(colors: litPair), startPoint: start, endPoint: end))
                        context.stroke(cell.path, with: .color(Tone.ink.opacity(0.45)), lineWidth: max(1, radius * 0.06))
                    } else {
                        context.fill(cell.path, with: .linearGradient(
                            Gradient(colors: dimPair), startPoint: start, endPoint: end))
                        context.stroke(cell.path, with: .color(Tone.ink.opacity(0.20)), lineWidth: max(1, radius * 0.05))
                    }
                }
            }
            .frame(width: size * 0.78, height: size * 0.78)
            .shadow(color: Tone.brandGlow.opacity(ThemeStore.shared.tone.isLuminous ? 0.35 : 0), radius: 20)
            .offset(y: bob ? -4 : 4)
            .animation(.easeInOut(duration: 2.6).repeatForever(autoreverses: true), value: bob)
        }
        .frame(width: size, height: size)
        .onAppear { bob = true }
    }
}

// MARK: Formatting

/// Byte counts the way Finder reports them, for file sizes and export totals.
func byteText(_ bytes: Int) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}

/// `m:ss`, for elapsed run time.
func elapsed(from start: Date, to end: Date) -> String {
    let seconds = max(0, Int(end.timeIntervalSince(start)))
    return String(format: "%d:%02d", seconds / 60, seconds % 60)
}
