import SwiftUI

/// The Settings window, reached with ⌘,.
///
/// Two panes, switched at the top: **Appearance**, which is the palette, and **Keyboard**, which
/// is the shortcut scheme. They are the two things this app asks the user to decide rather than
/// merely fill in, and neither belongs in a query tab.
///
/// The panes are drawn with this app's own controls rather than `TabView`. A system tab bar would
/// sit on the window's own material in the system's own colours, which is the one surface in the
/// app that would then not be wearing the palette the user just chose two inches below it.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var pane: Pane = .appearance

    private enum Pane: String, CaseIterable, Hashable {
        case appearance, keyboard

        var title: String {
            switch self {
            case .appearance: "Appearance"
            case .keyboard: "Keyboard"
            }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Outside the scroll view on purpose: the pane switch stays put while a pane scrolls,
            // so it cannot end up off-screen behind the content it switches.
            Segmented(selection: $pane, options: Pane.allCases) { $0.title }
                .frame(width: 240)

            // The pane scrolls and the window does not grow with it.
            //
            // This window had no height of its own, so it sized itself to its content — which was
            // fine while the Appearance pane was short and stopped being fine when the Mode section
            // added a picker, two paragraphs and a divider. The window grew taller than the screen,
            // macOS pinned its top to the top edge, and Backdrop Glow and Reset — the last two
            // things in the pane — were left below the bottom with no way to reach them. Sizing to
            // content cannot stay out of that state, because every section added later pushes the
            // bottom further off, so the height is pinned here and the overflow scrolls.
            ScrollView {
                switch pane {
                case .appearance: AppearanceSettings()
                case .keyboard: KeyboardSettings()
                }
            }
            // No rubber-banding when the pane already fits, which would otherwise let a short pane
            // bounce against edges that have nothing beyond them.
            .scrollBounceBehavior(.basedOnSize)
        }
        // 560, not 480: three preset tiles share this row and each one carries a
        // "theme · accent · tone" line. At 480 that line truncated to "Midnight ·…", which is
        // worse than not showing it.
        //
        // 640 tall: enough for the longest pane to show its first sections without scrolling on a
        // laptop display, and small enough to fit above the Dock with the menu bar on the shortest
        // screen this app supports.
        .padding(20)
        .frame(width: 560, height: 640)
        .background(Tone.canvas)
    }
}

// MARK: Appearance

/// The palette: which canvas, which accent, and how hard the backdrop glows.
struct AppearanceSettings: View {
    @Bindable private var store = ThemeStore.shared

    var body: some View {
        // The pane is laid out as sections, not as one long list: a small gap ties a section's
        // label to its control (10), a large one separates sections (24) with the divider sitting
        // in that gap. When both distances were 16 the eye got no grouping and the pane read as a
        // single cramped column.
        VStack(alignment: .leading, spacing: 24) {
            VStack(alignment: .leading, spacing: 10) {
                sectionHeader("Mode", text:
                    "System follows macOS and keeps following it — switch the system appearance with "
                    + "this window open and everything below repaints. Light and Dark pin one instead, "
                    + "whatever the system is set to.")
                Segmented(selection: $store.mode, options: AppearanceMode.allCases) { $0.title }
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            VStack(alignment: .leading, spacing: 10) {
                sectionHeader("Preset", text:
                    "Three combinations worth a single click. Each one names a canvas for the dark "
                    + "and another for the light, so switching mode stays inside the preset. "
                    + "Everything below still works on its own.")
                presetButtons
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            VStack(alignment: .leading, spacing: 10) {
                sectionHeader("Theme", text:
                    "The canvas, for the appearance you are in. Each is listed on its own side — "
                    + "the dark ones in the dark, the light ones in the light — because a canvas "
                    + "belongs to one appearance and there is no such thing as a light Midnight.")
                themeTiles
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            VStack(alignment: .leading, spacing: 10) {
                sectionHeader("Accent", text:
                    "Paints the interactive chrome: the Run capsule, focus rings, the selected tab "
                    + "and tile. The object tree's own colours do not move — they are how the tree "
                    + "tells a table from a column, and the app's mark keeps its own pair.")
                accentSwatches
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            VStack(alignment: .leading, spacing: 10) {
                sectionHeader("Tone", text:
                    "How the coloured surfaces are painted, independent of the colours themselves. "
                    + "Plain and Soft draw no gradient at all — no ramp on the Run capsule, no "
                    + "sheen, no coloured shadow, and a flat backdrop.")
                tonePicker
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    SectionLabel(text: "Backdrop glow")
                    Spacer()
                    Text(glowLabel)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(Tone.secondary)
                }
                Slider(value: $store.glow, in: 0...1.5)
                    .tint(Tone.accent)
                    .disabled(!store.tone.isLuminous)
                Text(store.tone.isLuminous
                     ? "The two radial glows behind the workspace. 0 leaves a flat canvas."
                     : "The \(store.tone.title) tone draws a flat backdrop, so there is nothing to scale.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Divider().overlay(Tone.ink.opacity(0.08))

            HStack {
                Text("Reset returns to System appearance and the Classic preset: midnight in the dark, "
                     + "daylight in the light, ice, glow, 100%.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                Spacer(minLength: 12)
                PillButton(title: "Reset", symbol: "arrow.uturn.backward", role: .quiet) {
                    store.reset()
                }
                .disabled(store.theme == .midnight && store.accent == .ice
                          && store.tone == .glow && store.glow == 1.0)
            }
        }
    }

    private var glowLabel: String {
        "\(Int((store.glow * 100).rounded()))%"
    }

    /// A section's label and its one-paragraph explanation, kept together so the gap below the
    /// text is the section gap (10), never the between-sections gap (24).
    private func sectionHeader(_ title: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            SectionLabel(text: title)
            Text(text)
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// How an accent looks under a given tone, for a swatch or a tile preview.
    ///
    /// Shared by all three previews so none of them can promise a gradient the tone would not
    /// draw. A Settings pane whose swatches disagree with the window behind them is worse than one
    /// with no swatches at all.
    private func previewFill(_ accent: AccentChoice, tone: SurfaceTone) -> LinearGradient {
        switch tone {
        case .glow:
            LinearGradient(colors: [accent.glow, accent.deep], startPoint: .topLeading, endPoint: .bottomTrailing)
        case .plain:
            LinearGradient(colors: [accent.deep, accent.deep], startPoint: .top, endPoint: .bottom)
        case .soft:
            LinearGradient(colors: [accent.glow.opacity(0.22), accent.glow.opacity(0.22)],
                           startPoint: .top, endPoint: .bottom)
        }
    }

    /// One chip per tone, each drawn in the current accent so the choice is visible rather than
    /// only named.
    private var tonePicker: some View {
        HStack(spacing: 8) {
            ForEach(SurfaceTone.allCases) { tone in
                let active = store.tone == tone
                Button { store.tone = tone } label: {
                    VStack(spacing: 6) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(Tone.canvas)
                            Capsule()
                                .fill(previewFill(store.accent, tone: tone))
                                .frame(width: 34, height: 12)
                                .overlay(Capsule().strokeBorder(
                                    tone == .soft ? store.accent.glow.opacity(0.55) : .clear))
                        }
                        .frame(height: 34)
                        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .strokeBorder(active ? Tone.accent.opacity(0.85) : Tone.ink.opacity(0.12),
                                          lineWidth: active ? 1.5 : 1))
                        Text(tone.title)
                            .font(.system(size: 11, weight: active ? .semibold : .regular))
                            .foregroundStyle(active ? Tone.ink : Tone.secondary)
                    }
                    .frame(maxWidth: .infinity)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(tone.detail)
                .accessibilityLabel(tone.title)
                .accessibilityAddTraits(active ? [.isSelected] : [])
            }
        }
    }

    /// The named combinations. Each preview is drawn in that preset's *own* tone, so a flat preset
    /// shows a flat bar rather than a gradient one it would not produce.
    private var presetButtons: some View {
        HStack(spacing: 8) {
            ForEach(ThemePreset.all) { preset in
                Button { store.apply(preset) } label: {
                    presetTile(preset)
                }
                .buttonStyle(.plain)
                .help(preset.detail)
                .accessibilityLabel(preset.title)
                .accessibilityAddTraits(store.matches(preset) ? [.isSelected] : [])
            }
        }
    }

    /// One preset, drawn on **its own** canvas for the appearance in effect — which is what makes
    /// `Tone.ink` the right text colour here rather than a colour chosen from the tile's palette.
    /// Because a preset now carries a canvas per appearance, the tile it paints always matches the
    /// window it is drawn in, so the chrome ink is already the contrasting one.
    ///
    /// Split out from `presetButtons` rather than inlined: as a single expression the type checker
    /// refused it ("unable to type-check this expression in reasonable time").
    private func presetTile(_ preset: ThemePreset) -> some View {
        let active = store.matches(preset)
        let canvas = preset.theme(isDark: store.isDarkAppearance).canvas
        return HStack(spacing: 9) {
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(previewFill(preset.accent, tone: preset.tone))
                .frame(width: 30, height: 16)
                .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous)
                    .strokeBorder(Tone.ink.opacity(0.22)))
            VStack(alignment: .leading, spacing: 1) {
                Text(preset.title)
                    .font(.system(size: 12, weight: active ? .semibold : .medium))
                    .foregroundStyle(active ? Tone.ink : Tone.secondary)
                // "Blue · Glow", not "Graphite · Blue · Glow": the theme is already shown by the
                // tile's own background, and the bar beside this line shows the accent under the
                // tone. Spelling all three out at 10pt monospaced needs ~191pt inside a ~168pt
                // tile, so it truncated to "Graphite · Blue · Gl…" — a label that names nothing.
                Text("\(preset.accent.title) · \(preset.tone.title)")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Tone.ink.opacity(0.62))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity)
        .frame(height: 40)
        .background(canvas, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous)
            .strokeBorder(active ? Tone.accent.opacity(0.85) : Tone.ink.opacity(0.12),
                          lineWidth: active ? 1.5 : 1))
        .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
    }

    private var themeTiles: some View {
        // Filtered, not every case: offering a near-black Midnight tile while the app is light
        // would be offering a canvas that cannot be painted in this appearance. `store.theme` is
        // resolved per appearance too, so the selected ring always lands on one of these.
        let tiles = AppTheme.allCases.filter { $0.isDark == store.isDarkAppearance }
        return HStack(spacing: 8) {
            ForEach(tiles) { theme in
                Button { store.theme = theme } label: {
                    VStack(spacing: 6) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(theme.canvas)
                            // A miniature of the shell: a lit capsule and two chrome rows, so the
                            // tile previews the palette rather than just swatching one colour.
                            VStack(alignment: .leading, spacing: 4) {
                                Capsule()
                                    .fill(previewFill(store.accent, tone: store.tone))
                                    .frame(width: 30, height: 7)
                                RoundedRectangle(cornerRadius: 2).fill(Tone.ink.opacity(0.22)).frame(height: 3)
                                RoundedRectangle(cornerRadius: 2).fill(Tone.ink.opacity(0.12)).frame(width: 34, height: 3)
                            }
                            .padding(9)
                            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        }
                        .frame(height: 52)
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .strokeBorder(store.theme == theme ? Tone.accent.opacity(0.85) : Tone.ink.opacity(0.12),
                                          lineWidth: store.theme == theme ? 1.5 : 1))

                        Text(theme.title)
                            .font(.system(size: 11, weight: store.theme == theme ? .semibold : .regular))
                            .foregroundStyle(store.theme == theme ? Tone.ink : Tone.secondary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(theme.detail)
                .accessibilityLabel(theme.title)
                .accessibilityAddTraits(store.theme == theme ? [.isSelected] : [])
            }
        }
    }

    private var accentSwatches: some View {
        HStack(spacing: 10) {
            ForEach(AccentChoice.allCases) { choice in
                Button { store.accent = choice } label: {
                    Circle()
                        .fill(previewFill(choice, tone: store.tone))
                        .frame(width: 26, height: 26)
                        .overlay(Circle().strokeBorder(Tone.ink.opacity(store.accent == choice ? 0.9 : 0.15),
                                                       lineWidth: store.accent == choice ? 2 : 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(choice.title)
                .accessibilityAddTraits(store.accent == choice ? [.isSelected] : [])
                .help(choice.title)
            }
            Spacer(minLength: 0)
        }
    }
}

// MARK: Keyboard

/// Whose keys this app answers to.
///
/// The shortcut table is drawn rather than described: a scheme is a promise about which key does
/// what, and a promise the user cannot read is one they cannot check.
struct KeyboardSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                SectionLabel(text: "Keyboard")
                Text("Choose whose keys this app answers to. Every binding below comes from the "
                     + "scheme's own definitions, so the two never drift apart.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Picker("", selection: $model.shortcutScheme) {
                ForEach(ShortcutScheme.allCases) { scheme in
                    Text(scheme.title).tag(scheme)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            Text(model.shortcutScheme.detail)
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Divider().overlay(Tone.ink.opacity(0.08))

            bindingTable
        }
    }

    /// Every action the app has, with the key the current scheme gives it.
    ///
    /// An unbound row says so instead of being hidden. A missing shortcut is something the user
    /// needs to know about — it is why "Run" under DBeaver is ⌘↩ and not the ⌘R they are reaching
    /// for — and silently omitting the row would make the scheme look complete when it is not.
    private var bindingTable: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(ShortcutAction.allCases) { action in
                HStack(spacing: 8) {
                    Text(action.title)
                        .font(.system(size: 11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    if let shortcut = model.shortcutScheme.shortcut(for: action) {
                        Text(shortcut.display)
                            .font(.system(size: 11, weight: .medium, design: .monospaced))
                            .foregroundStyle(Tone.ink.opacity(0.85))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Tone.ink.opacity(0.07),
                                        in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    } else {
                        Text("not bound")
                            .font(.system(size: 10.5))
                            .foregroundStyle(Tone.secondary.opacity(0.7))
                    }
                }
                .padding(.vertical, 3)
            }
        }
    }
}
