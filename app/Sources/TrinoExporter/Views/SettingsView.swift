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
///
/// ## Structure
///
/// The pane is **grouped rows inside titled cards**, not a flat list. Apple's HIG for settings on
/// macOS asks for a stable pane switcher that always marks the active pane, settings organised in
/// groups, and as few controls on screen at once as the job allows. The previous layout was one
/// `VStack` of seven equally weighted sections separated by nothing but a divider: Preset, Theme,
/// Accent and Tone all read as siblings even though Preset *is* a theme, an accent and a tone
/// chosen together. Grouping them puts the relationship on screen — pick a preset, or open the
/// Appearance card and set the three parts yourself.
struct SettingsView: View {
    @State private var pane: Pane

    /// The pane is settable at init only so a snapshot render can photograph the Keyboard pane
    /// without a person clicking over to it. In the running app nothing passes this.
    init(pane: Pane = .appearance) {
        _pane = State(initialValue: pane)
    }

    enum Pane: String, CaseIterable, Hashable {
        case appearance, keyboard

        var title: String {
            switch self {
            case .appearance: "Appearance"
            case .keyboard: "Keyboard"
            }
        }

        /// A symbol per pane, so the switcher reads as navigation rather than as a segmented
        /// filter. Both are SF Symbols the platform already uses for these ideas.
        var symbol: String {
            switch self {
            case .appearance: "paintpalette"
            case .keyboard: "keyboard"
            }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Outside the scroll view on purpose: the pane switch stays put while a pane scrolls,
            // so it cannot end up off-screen behind the content it switches.
            paneBar

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

    /// The pane switcher, drawn as a bar of equal-width buttons rather than a small segmented
    /// control pinned to the left.
    ///
    /// Two reasons it is its own control. HIG asks a settings window for a switcher that stays
    /// visible and *always marks the active pane*, and it asks for the switcher to read as
    /// navigation between areas — which is what the symbol plus the recessed track says, and what
    /// a 240pt segmented control floating above a 520pt pane did not.
    private var paneBar: some View {
        HStack(spacing: 3) {
            ForEach(Pane.allCases, id: \.self) { option in
                let active = pane == option
                Button { pane = option } label: {
                    HStack(spacing: 6) {
                        Image(systemName: option.symbol)
                            .font(.system(size: 11.5, weight: .medium))
                        Text(option.title)
                            .font(.system(size: 12, weight: active ? .semibold : .regular))
                    }
                    .foregroundStyle(active ? Tone.ink : Tone.secondary)
                    .frame(maxWidth: .infinity)
                    .frame(height: 28)
                    .background(Tone.ink.opacity(active ? 0.14 : 0),
                                in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(active ? [.isSelected] : [])
            }
        }
        .padding(3)
        .background(Tone.recess.opacity(0.32), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous)
            .strokeBorder(Tone.ink.opacity(0.08)))
    }
}

// MARK: Grouping

/// A titled card: a section label, an optional one-line explanation, then the controls.
///
/// The card is what gives the pane its hierarchy. Every group is the same shape, so the eye reads
/// the pane as "a few groups" rather than as "a list of things", and the gap between cards (18)
/// is visibly larger than the gap inside one (12) — which is the whole difference between a
/// deliberate layout and a stack.
private struct SettingsCard<Content: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                SectionLabel(text: title)
                if let detail {
                    Text(detail)
                        .font(.system(size: 11))
                        .foregroundStyle(Tone.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            content
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glass(12)
    }
}

/// One labelled row inside a card: the name of the thing, then the control that sets it.
///
/// Used where the control is short enough to sit under its own label without crowding, which is
/// every control in the Appearance card. The label is what the previous layout lacked — its
/// controls were separated by nothing but whitespace, so a tile row and a swatch row read as the
/// same kind of thing.
private struct SettingsRow<Control: View>: View {
    let label: String
    var trailing: String?
    @ViewBuilder let control: Control

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                Text(label)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(Tone.ink)
                Spacer(minLength: 8)
                if let trailing {
                    Text(trailing)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(Tone.secondary)
                }
            }
            control
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A hairline between two rows of the same card, so the rows read as a group rather than as one
/// paragraph of controls.
private struct RowDivider: View {
    var body: some View {
        Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
    }
}

// MARK: Appearance

/// The palette: which canvas, which accent, and how hard the backdrop glows.
struct AppearanceSettings: View {
    @Bindable private var store = ThemeStore.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            // The named combinations first, because choosing one is the whole decision for most
            // people. The card below is where the same choice is taken apart.
            SettingsCard(title: "Preset",
                         detail: "Three combinations worth a single click. Each names a canvas for the dark and another for the light, so switching mode stays inside the preset. Everything below still works on its own.") {
                presetButtons
            }

            // Mode, Theme, Accent and Tone in one card: they are four parts of one question, and
            // the divider between rows says so without the weight of four separate sections.
            SettingsCard(title: "Appearance",
                         detail: "The canvas, the accent that paints the chrome, and how the coloured surfaces are filled.") {
                SettingsRow(label: "Mode") {
                    Segmented(selection: $store.mode, options: AppearanceMode.allCases) { $0.title }
                }
                RowDivider()
                SettingsRow(label: "Theme") { themeTiles }
                RowDivider()
                SettingsRow(label: "Accent") { accentSwatches }
                RowDivider()
                SettingsRow(label: "Tone",
                            trailing: store.tone.isLuminous ? nil : "flat") { tonePicker }
            }

            SettingsCard(title: "Backdrop",
                         detail: store.tone.isLuminous
                            ? "How hard the two radial glows behind the workspace burn. 0 leaves a flat canvas."
                            : "The \(store.tone.title) tone draws a flat backdrop, so there is nothing to scale.") {
                SettingsRow(label: "Glow", trailing: glowLabel) {
                    Slider(value: $store.glow, in: 0...1.5)
                        .tint(Tone.accent)
                        .disabled(!store.tone.isLuminous)
                }
            }

            resetFooter
        }
    }

    private var glowLabel: String {
        "\(Int((store.glow * 100).rounded()))%"
    }

    /// Reset sits apart from the cards, under its own hairline: it is not a setting, it is the
    /// way back from all of them, and drawing it as a fourth card would give it the same weight as
    /// the choices it undoes.
    private var resetFooter: some View {
        HStack(alignment: .center, spacing: 12) {
            Text("Reset returns to System appearance and the Classic preset: midnight in the dark, "
                 + "daylight in the light, ice, glow, 100%.")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            PillButton(title: "Reset", symbol: "arrow.uturn.backward", role: .quiet) {
                store.reset()
            }
            .disabled(store.theme == .midnight && store.accent == .ice
                      && store.tone == .glow && store.glow == 1.0)
        }
        .padding(.top, 2)
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
        .frame(height: 44)
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
                        .frame(height: 46)
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
        VStack(alignment: .leading, spacing: 18) {
            SettingsCard(title: "Scheme",
                         detail: "Choose whose keys this app answers to. Every binding below comes from the scheme's own definitions, so the two never drift apart.") {
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
            }

            SettingsCard(title: "Bindings") { bindingTable }
        }
    }

    /// Every action the app has, with the key the current scheme gives it.
    ///
    /// An unbound row says so instead of being hidden. A missing shortcut is something the user
    /// needs to know about — it is why "Run" under DBeaver is ⌘↩ and not the ⌘R they are reaching
    /// for — and silently omitting the row would make the scheme look complete when it is not.
    private var bindingTable: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(ShortcutAction.allCases.enumerated()), id: \.element) { index, action in
                if index > 0 { RowDivider() }
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
                .padding(.vertical, 6)
            }
        }
    }
}
