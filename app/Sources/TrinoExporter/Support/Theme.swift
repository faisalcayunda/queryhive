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

enum Tone {
    static let canvas = Color(hex: 0x0A0B1E)
    static let ice = Color(hex: 0x4FD8FF)
    static let violet = Color(hex: 0x7B61FF)
    static let magenta = Color(hex: 0xFF4FA3)
    static let mint = Color(hex: 0x3EE6A8)
    static let amber = Color(hex: 0xFFB547)
    static let coral = Color(hex: 0xFF5E6C)
    static let gray = Color(hex: 0x9AA0A8)
    static let blue = Color(hex: 0x4F8DFF)
    static let secondary = Color.white.opacity(0.68)
}

extension Color {
    init(hex: UInt32) {
        self.init(red: Double(hex >> 16 & 0xFF) / 255, green: Double(hex >> 8 & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
    }
}

extension Font {
    static let heroTitle = Font.system(size: 30, weight: .bold, design: .rounded)
    static let cardTitle = Font.system(size: 14, weight: .semibold, design: .rounded)
    static let body13 = Font.system(size: 13)
    static let mono12 = Font.system(size: 12, design: .monospaced)
    static let mono13 = Font.system(size: 13, design: .monospaced)
}

/// The colour pair of a module or result: the backdrop glows and every accent derive from it.
struct Hue {
    let glow: Color
    let accent: Color

    var gradient: LinearGradient { LinearGradient(colors: [glow, accent], startPoint: .topLeading, endPoint: .bottomTrailing) }

    static let exporter = Hue(glow: Tone.ice, accent: Tone.violet)
    static let connection = Hue(glow: Tone.magenta, accent: Tone.violet)
    static let success = Hue(glow: Tone.mint, accent: Color(hex: 0x12A886))
    static let failure = Hue(glow: Tone.amber, accent: Tone.coral)
}

/// The workspace backdrop: one radial glow per module colour, drawn behind the panels so the
/// glass has something to sit on. Radial gradients, never `.blur` (a snapshot capture does not
/// render it) and far cheaper than a 190pt blur.
struct Backdrop: View {
    let hue: Hue

    var body: some View {
        ZStack {
            Tone.canvas
            RadialGradient(colors: [hue.glow, .clear], center: .center, startRadius: 0, endRadius: 340)
                .frame(width: 680, height: 680)
                .opacity(0.30)
                .offset(x: 90, y: -280)
            RadialGradient(colors: [hue.accent, .clear], center: .center, startRadius: 0, endRadius: 280)
                .frame(width: 560, height: 560)
                .opacity(0.22)
                .offset(x: -260, y: 320)
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
            .strokeBorder(tint?.opacity(tintOpacity) ?? .white.opacity(0.12), lineWidth: lineWidth))
    }

    func field(invalid: Bool = false) -> some View {
        textFieldStyle(.plain)
            .font(.body13)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(Color.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(invalid ? Tone.coral.opacity(0.8) : .white.opacity(0.1), lineWidth: invalid ? 1.5 : 1))
    }

    /// A toolbar-height recessed control (the folder chip, the output-name field).
    func toolbarField() -> some View {
        textFieldStyle(.plain)
            .font(.system(size: 12))
            .padding(.horizontal, 9)
            .frame(height: 28)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(.white.opacity(0.10)))
    }

    /// The SQL editor's inset: recessed black, block-shaped, and focus shown with the module
    /// accent rather than the system focus ring.
    func editorBox(focused: Bool) -> some View {
        background(Color.black.opacity(0.34), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(focused ? Tone.ice.opacity(0.45) : .white.opacity(0.08), lineWidth: 1))
    }

    /// Hairline panel edge used between the sidebar, the toolbar and the panel.
    func panelEdge(_ edge: Edge) -> some View {
        overlay(alignment: edge == .trailing ? .trailing : edge == .leading ? .leading : edge == .top ? .top : .bottom) {
            Rectangle().fill(.white.opacity(0.07)).frame(width: edge == .leading || edge == .trailing ? 1 : nil,
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
    static let paneHeader: CGFloat = 32
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
                Text(title).font(.system(size: 13, weight: .semibold))
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 14)
            .frame(height: 28)
            .background {
                Capsule().fill(hue.gradient)
                Capsule().fill(LinearGradient(colors: [.white.opacity(0.28), .clear], startPoint: .top, endPoint: .center))
            }
            .overlay(Capsule().strokeBorder(.white.opacity(0.25)))
            .opacity(enabled ? 1 : 0.4)
            .saturation(enabled ? 1 : 0.2)
            .shadow(color: hue.accent.opacity(enabled ? (hovering ? 0.65 : 0.45) : 0), radius: hovering ? 14 : 9, y: 3)
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
    var tint: Color = .white
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
                .background(Color.white.opacity(hovering && enabled ? 0.13 : 0.06),
                            in: RoundedRectangle(cornerRadius: diameter * 0.26, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: diameter * 0.26, style: .continuous)
                    .strokeBorder(.white.opacity(0.08)))
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

struct PillButtonStyle: ButtonStyle {
    var tint: Color = .white

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(tint)
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
            .background(Color.white.opacity(configuration.isPressed ? 0.22 : 0.11), in: Capsule())
            .overlay(Capsule().strokeBorder(tint.opacity(tint == .white ? 0.12 : 0.4)))
            .contentShape(Capsule())
    }
}

extension ButtonStyle where Self == PillButtonStyle {
    static var pill: PillButtonStyle { PillButtonStyle() }
    /// Destructive pill: coral text on a coral 0.4 stroke, used by Delete.
    static var coralPill: PillButtonStyle { PillButtonStyle(tint: Tone.coral) }
}

/// Vertical hairline between toolbar groups.
struct ToolbarSeparator: View {
    var body: some View {
        Rectangle().fill(.white.opacity(0.10)).frame(width: 1, height: 18).padding(.horizontal, 3)
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
                        .font(.system(size: 12, weight: .medium))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                        .background(Color.white.opacity(selection == option ? 0.18 : 0), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(Color.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
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
                .font(.system(size: 11, weight: .semibold))
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
                    .foregroundStyle(isOn ? Tone.ice : .white.opacity(0.35))
                Text(label).font(.system(size: 12))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(Color.black.opacity(0.24), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(isOn ? Tone.ice.opacity(0.45) : .white.opacity(hovering ? 0.18 : 0.09)))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
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
            .font(.system(size: 11, weight: .semibold, design: .monospaced))
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

    var body: some View {
        HStack(spacing: 0) {
            TextField(placeholder, text: $text)
                .textFieldStyle(.plain)
                .font(monospaced ? .system(size: 12, design: .monospaced) : .system(size: 12))
                .padding(.horizontal, 9)
            if !options.isEmpty {
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
        .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(.white.opacity(0.10)))
    }
}

/// Uppercase section label, the one piece of chrome every Navicat pane header shares.
struct SectionLabel: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 10.5, weight: .semibold))
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
            if halo {
                Circle().fill(RadialGradient(colors: [hue.glow.opacity(0.35), .clear], center: .center, startRadius: 0, endRadius: size * 0.7))
            }
            tile.fill(hue.gradient)
                .overlay(tile.fill(LinearGradient(colors: [.white.opacity(0.4), .clear], startPoint: .top, endPoint: .center)))
                .overlay(tile.strokeBorder(.white.opacity(0.35), lineWidth: 1))
                .frame(width: size * 0.62, height: size * 0.62)
                .shadow(color: hue.accent.opacity(0.6), radius: size * 0.14, y: size * 0.07)
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

func hiveCells(in size: CGSize) -> [(path: Path, lit: Bool)] {
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
                     radius: radius * 0.94), index == 0)
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

            context.fill(hexagonPath(centre: centre, radius: radius), with: .linearGradient(
                Gradient(colors: [Tone.ice, Tone.violet]),
                startPoint: CGPoint(x: 0, y: 0),
                endPoint: CGPoint(x: canvasSize.width, y: canvasSize.height)))
            context.stroke(hexagonPath(centre: centre, radius: radius * 0.46),
                           with: .color(.white.opacity(0.92)), lineWidth: max(1, size * 0.075))
        }
        .frame(width: size, height: size)
    }
}

/// The comb at hero scale, for the workspace's empty state: lit centre cell, six dim neighbours,
/// a glow behind it, and a slow bob that keeps it from reading as a frozen screenshot.
struct HiveHero: View {
    var size: CGFloat = 200
    @State private var bob = false

    var body: some View {
        let radius = size / 5.5
        ZStack {
            Circle().fill(RadialGradient(colors: [Tone.ice.opacity(0.30), .clear], center: .center, startRadius: 0, endRadius: size * 0.55))
            Canvas { context, canvasSize in
                let start = CGPoint(x: 0, y: 0)
                let end = CGPoint(x: canvasSize.width, y: canvasSize.height)
                for cell in hiveCells(in: canvasSize) {
                    if cell.lit {
                        context.fill(cell.path, with: .linearGradient(
                            Gradient(colors: [Tone.ice, Tone.violet]), startPoint: start, endPoint: end))
                        context.stroke(cell.path, with: .color(.white.opacity(0.45)), lineWidth: max(1, radius * 0.06))
                    } else {
                        context.fill(cell.path, with: .linearGradient(
                            Gradient(colors: [Tone.ice.opacity(0.20), Tone.violet.opacity(0.10)]),
                            startPoint: start, endPoint: end))
                        context.stroke(cell.path, with: .color(.white.opacity(0.20)), lineWidth: max(1, radius * 0.05))
                    }
                }
            }
            .frame(width: size * 0.78, height: size * 0.78)
            .shadow(color: Tone.ice.opacity(0.35), radius: 20)
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
