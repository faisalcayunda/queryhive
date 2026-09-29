import SwiftUI

/// The Settings window, reached with ⌘,.
///
/// A bar of five panes across the top — **General**, **Appearance**, **Editor**, **Data**,
/// **Keyboard** — each an SF Symbol above its label, switched by clicking one. The pane below it
/// scrolls; the bar does not.
///
/// The vocabulary and the shape of the bar are TablePro's: an icon above each label on a rounded
/// highlight, centred, scrolling once the items outgrow the window, with a hairline between the bar
/// and the pane. TablePro draws that bar as a native `NSToolbar` (an `NSTabViewController` with
/// `tabStyle = .toolbar`, one `NSToolbarItem` per pane), which is why its icon sits above its
/// label; this app draws its own bar instead of wearing a system tab bar, so the one surface that
/// would otherwise not be wearing the palette the user just chose two inches below stays in the
/// palette. The shape is taken; the code is this tree's own.
///
/// Only panes this app can fill are here. TablePro's AI, Integrations, Plugins, Sync and License
/// panes have no QueryHive content: the adoption plan refuses AI, plugins, sync and licensing
/// (§9), and MCP's token management is CLI-only today. A pane that exists but is empty, or that
/// pretends to configure something the app cannot do, is worse than not having it.
///
/// ## Structure
///
/// The pane is **grouped rows inside titled cards**, not a flat list. Apple's HIG for settings on
/// macOS asks for a stable pane switcher that always marks the active pane, settings organised in
/// groups, and as few controls on screen at once as the job allows. Grouping puts the relationship
/// on screen — pick a preset, or open the Appearance card and set the three parts yourself.
struct SettingsView: View {
    @State private var pane: Pane

    /// The Sparkle controller the app scene owns, handed in so the General pane can offer **Check
    /// for Updates…**. It is `nil` in a snapshot: constructing an `Updater` starts Sparkle, which is
    /// a side effect a render must not have, so the pane draws the same control disabled instead.
    private let updater: Updater?

    /// The pane is settable at init so a snapshot render can photograph one without a person
    /// clicking over to it, and so the app can open on a chosen pane. In the running app nothing
    /// passes a pane, so Settings opens on General.
    init(pane: Pane = .general, updater: Updater? = nil) {
        _pane = State(initialValue: pane)
        self.updater = updater
    }

    /// One pane per settings area, with the titles and symbols this app has content for.
    ///
    /// `general` is first because it is where the app's own settings live, and because the bar
    /// opens there. The raw values are also the snapshot scene suffixes (`settings-appearance`, …).
    enum Pane: String, CaseIterable, Hashable {
        case general, account, appearance, editor, data, keyboard

        /// The panes the bar shows.
        ///
        /// `account` is deliberately not one of them: signing in needs a Google client id this build
        /// does not ship, so a pane whose only action says "not configured" is noise in the bar. The
        /// pane itself, its snapshot scene and the engine commands behind it all stay, so putting it
        /// back is one line once a client id is set.
        static let visible: [Pane] = allCases.filter { $0 != .account }

        var title: String {
            switch self {
            case .general: "General"
            case .account: "Account"
            case .appearance: "Appearance"
            case .editor: "Editor"
            case .data: "Data"
            case .keyboard: "Keyboard"
            }
        }

        /// The SF Symbol the bar draws above the label. The names are TablePro's own for these
        /// ideas — `gearshape`, `paintbrush`, `doc.text`, `tablecells`, `keyboard` — so the bar
        /// reads the same way in both apps. `person.crop.circle` is this app's own, because the
        /// pane is about who is using it rather than about TablePro's credential profiles.
        var symbol: String {
            switch self {
            case .general: "gearshape"
            case .account: "person.crop.circle"
            case .appearance: "paintbrush"
            case .editor: "doc.text"
            case .data: "tablecells"
            case .keyboard: "keyboard"
            }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Outside the scroll view on purpose: the pane switch stays put while a pane scrolls,
            // so it cannot end up off-screen behind the content it switches.
            paneBar

            // The hairline TablePro's toolbar draws under itself: the bar and the pane are two
            // surfaces, and the line is what says so without introducing a second material.
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)

            // The pane scrolls and the window does not grow with it.
            //
            // This window had no height of its own, so it sized itself to its content — which was
            // fine while the Appearance pane was short and stopped being fine when the Mode section
            // added a picker, two paragraphs and a divider. The window grew taller than the screen,
            // macOS pinned its top to the top edge, and the last controls in the pane were left
            // below the bottom with no way to reach them. Sizing to content cannot stay out of that
            // state, because every section added later pushes the bottom further off, so the height
            // is pinned here and the overflow scrolls.
            ScrollView {
                paneContent
                    .padding(20)
            }
            // No rubber-banding when the pane already fits, which would otherwise let a short pane
            // bounce against edges that have nothing beyond them.
            .scrollBounceBehavior(.basedOnSize)
        }
        // 560, not 480: three preset tiles share this row and each one carries a
        // "theme · accent · tone" line. At 480 that line truncated to "Midnight ·…", which is
        // worse than not showing it. The five-item bar fits this width with room to spare — five
        // 92pt items and their spacing come to 488pt against the 536pt inside the 12pt gutters — so
        // the window did not have to grow for it.
        //
        // 640 tall: enough for the longest pane to show its first sections without scrolling on a
        // laptop display, and small enough to fit above the Dock with the menu bar on the shortest
        // screen this app supports.
        .frame(width: 560, height: 640)
        .background(Tone.canvas)
    }

    @ViewBuilder
    private var paneContent: some View {
        switch pane {
        case .general: GeneralSettings(updater: updater)
        case .account: AccountSettings()
        case .appearance: AppearanceSettings()
        case .editor: EditorSettings()
        case .data: DataSettings()
        case .keyboard: KeyboardSettings()
        }
    }

    /// The pane switcher: TablePro's shape, this app's drawing.
    ///
    /// One item per pane, its SF Symbol above its label, the selected one on a rounded highlight.
    /// The row is centred while it fits and scrolls once it does not, so a sixth pane would not
    /// push the last one off the edge the way the old equal-width track did.
    private var paneBar: some View {
        GeometryReader { geometry in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(Pane.visible, id: \.self) { option in
                        paneItem(option)
                    }
                }
                .padding(.horizontal, 12)
                // At least the bar's own width, so the row sits in the middle while it fits; wider
                // once it does not, which is what turns the ScrollView into scrolling.
                .frame(minWidth: geometry.size.width, alignment: .center)
            }
            .scrollBounceBehavior(.basedOnSize)
        }
        .frame(height: 62)
    }

    /// One item of the bar: icon over label, on the highlight when it is the active pane.
    ///
    /// A `Button`, not a tap gesture, so it stays in the key-view loop and answers to the keyboard
    /// the way the previous segmented bar did. The active trait is what tells a screen reader which
    /// pane is showing, the same way `isSelected` does on the theme tiles below.
    private func paneItem(_ option: Pane) -> some View {
        let active = pane == option
        return Button {
            pane = option
        } label: {
            VStack(spacing: 3) {
                Image(systemName: option.symbol)
                    .font(.system(size: 16, weight: .medium))
                    .frame(height: 17)
                Text(option.title)
                    .font(.ui(10.5, weight: active ? .semibold : .regular))
                    .lineLimit(1)
            }
            .foregroundStyle(active ? Tone.ink : Tone.secondary)
            .frame(width: 92, height: 52)
            .background(Tone.ink.opacity(active ? 0.14 : 0),
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .help(option.title)
        .accessibilityLabel(option.title)
        .accessibilityAddTraits(active ? [.isSelected] : [])
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
                        .font(.ui(11))
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
                    .font(.ui(12, weight: .medium))
                    .foregroundStyle(Tone.ink)
                Spacer(minLength: 8)
                if let trailing {
                    Text(trailing)
                        .font(.code(11))
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

// MARK: General

/// The app's own settings: the software update, and the one button that puts every appearance
/// choice back where it started.
///
/// The Account pane: who this application is running as, and what is saved for them.
///
/// Named Account rather than Profiles on purpose. TablePro's Profiles pane manages reusable
/// credential and SSH sets, which this app does not have; what this pane manages is the identity
/// itself and the rows that belong to it. Calling it Profiles would promise the other feature.
struct AccountSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsCard(title: "Sign in",
                         detail: "Signing in is what makes a profile belong to someone. It uses the "
                             + "browser you already have, through Google's authorization-code flow with "
                             + "PKCE, and this app keeps no token afterwards: what it stores is the "
                             + "provider, the account's own id at the provider, and optionally an email "
                             + "and a name. It never touches a database connection.") {
                identityRow
                if let notice = model.accountNotice {
                    RowDivider()
                    Text(notice)
                        .font(.ui(10.5))
                        .foregroundStyle(Tone.ink.opacity(0.75))
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.vertical, 6)
                }
            }

            SettingsCard(title: "Profiles",
                         detail: "What this account has saved. A profile is owned by the account "
                             + "above, so it belongs to the identity rather than to the machine.") {
                if model.profiles.isEmpty {
                    Text("Nothing saved yet. A profile is written by whichever feature owns its "
                         + "kind, and none does yet.")
                        .font(.ui(11))
                        .foregroundStyle(Tone.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.vertical, 6)
                } else {
                    ForEach(model.profiles) { profile in
                        profileRow(profile)
                    }
                }
            }
        }
        .onAppear {
            // One call: `loadAccount` reads the profiles once it has the account, so the pane does
            // not start two local commands at once.
            model.loadAccount()
        }
    }

    private var signedIn: Bool { model.account?.provider != nil }

    private var identityRow: some View {
        HStack(spacing: 10) {
            Image(systemName: signedIn ? "person.crop.circle.fill" : "person.crop.circle.badge.questionmark")
                .font(.system(size: 22))
                .foregroundStyle(signedIn ? Tone.accent : Tone.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(identityTitle)
                    .font(.ui(12, weight: .semibold))
                    .foregroundStyle(Tone.ink)
                Text(identityDetail)
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 12)
            if signedIn {
                PillButton(title: "Sign out", symbol: "rectangle.portrait.and.arrow.right",
                           role: .quiet) {
                    model.signOut()
                }
            } else {
                PillButton(title: model.signingIn ? "Waiting for the browser…" : "Sign in with Google",
                           symbol: "arrow.up.right.square") {
                    model.signInWithGoogle()
                }
                .disabled(model.signingIn)
            }
        }
        .padding(.vertical, 6)
    }

    private var identityTitle: String {
        guard let account = model.account, account.provider != nil else { return "Not signed in" }
        return account.displayName ?? account.email ?? "Signed in"
    }

    private var identityDetail: String {
        guard let account = model.account, let provider = account.provider else {
            return "A local account already owns anything saved here."
        }
        let who = account.email ?? account.subject ?? "no subject"
        return "\(provider.capitalized) · \(who)"
    }

    private func profileRow(_ profile: Event.Profile) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "square.stack.3d.up")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(profile.name)
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.ink)
                Text(profile.kind)
                    .font(.code(10))
                    .foregroundStyle(Tone.secondary)
            }
            Spacer(minLength: 12)
            PillButton(title: "Delete", symbol: "trash", role: .destructive) {
                model.deleteProfile(profile.id)
            }
        }
        .padding(.vertical, 6)
    }
}

/// The reset lives here rather than beside the palette it resets because it is not a palette
/// control: `ThemeStore.reset()` also returns both fonts to the system face, so it is the app's
/// "start over" button and belongs with the other whole-app setting. TablePro's General pane keeps
/// its reset in the same place.
struct GeneralSettings: View {
    /// The app scene's Sparkle controller, or `nil` in a snapshot. See `SoftwareUpdateRow`.
    let updater: Updater?

    @Environment(AppModel.self) private var model
    @Bindable private var store = ThemeStore.shared

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 18) {
            SettingsCard(title: "Startup",
                         detail: "What the workspace holds when QueryHive opens: the tabs you left, "
                             + "with their statements and their results, or one empty tab. The session is "
                             + "written either way, so turning this back on brings back the workspace you "
                             + "had rather than the one from whenever it was last on.") {
                Toggle(isOn: $model.restoreTabsOnLaunch) {
                    Text("Reopen the tabs from last time")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                }
                .toggleStyle(.switch)
                .controlSize(.small)
                .padding(.vertical, 6)
            }

            SettingsCard(title: "Software update",
                         detail: "QueryHive updates through Sparkle: it reads an appcast over HTTPS, "
                             + "checks the archive against the signing key in the bundle, then swaps the app and "
                             + "relaunches. This button runs the same check the QueryHive menu offers.") {
                SoftwareUpdateRow(updater: updater)
            }

            SettingsCard(title: "Reset settings",
                         detail: "Puts the appearance and both fonts back where they started: System "
                             + "appearance, the Classic preset — Midnight in the dark, Daylight in the light — "
                             + "the Ice accent, Glow at 100%, and the system interface and code fonts.") {
                HStack {
                    Spacer(minLength: 0)
                    PillButton(title: "Reset", symbol: "arrow.uturn.backward", role: .quiet) {
                        store.reset()
                    }
                    .disabled(isAtDefaults)
                }
            }
        }
    }

    /// Whether every value the reset touches is already at its default, so the button can say
    /// "there is nothing to undo" rather than offering a click that changes nothing.
    ///
    /// Both stored themes are checked, not just the one in effect: the reset writes both halves, so
    /// a dark theme already at Midnight with a light theme that is not is still a reset waiting to
    /// happen. The previous check looked only at the theme in effect and ignored the fonts the same
    /// reset also moves.
    private var isAtDefaults: Bool {
        store.mode == .system
            && store.darkTheme == .midnight && store.lightTheme == .daylight
            && store.accent == .ice && store.tone == .glow && store.glow == 1.0
            && store.uiFontFamily == FontChoice.system && store.codeFontFamily == FontChoice.system
    }
}

/// The update card's one row: the version this build is, and the button that asks for a newer one.
///
/// Two shapes because the controller is optional. With one, the row observes it — `canCheckForUpdates`
/// flips while a check runs, and the button has to follow, or a second click during a check would be
/// accepted and Sparkle would have to ignore it. Without one (a snapshot) the row draws the same
/// controls disabled, which is the honest state for "there is no updater in this process".
private struct SoftwareUpdateRow: View {
    let updater: Updater?

    var body: some View {
        if let updater {
            LiveSoftwareUpdateRow(updater: updater)
        } else {
            SoftwareUpdateControls(version: Self.version, canCheck: false, check: {})
        }
    }

    /// The bundle's version, or `nil` when this is a bare executable with no `Info.plist` — which is
    /// what a `--snapshot` run is. The row then says nothing about a version rather than showing a
    /// placeholder it cannot back up.
    static var version: String? {
        let short = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        guard let short, !short.isEmpty else { return nil }
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        guard let build, !build.isEmpty else { return short }
        return "\(short) (\(build))"
    }
}

private struct LiveSoftwareUpdateRow: View {
    @ObservedObject var updater: Updater

    var body: some View {
        SoftwareUpdateControls(version: SoftwareUpdateRow.version,
                               canCheck: updater.canCheckForUpdates,
                               check: updater.checkForUpdates)
    }
}

private struct SoftwareUpdateControls: View {
    let version: String?
    let canCheck: Bool
    let check: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if let version {
                Text("QueryHive \(version)")
                    .font(.code(11))
                    .foregroundStyle(Tone.secondary)
            }
            Spacer(minLength: 12)
            PillButton(title: "Check for Updates…", symbol: "arrow.triangle.2.circlepath",
                       role: .secondary, action: check)
                .disabled(!canCheck)
        }
    }
}

// MARK: Appearance

/// The palette: which canvas, which accent, and how hard the backdrop glows.
struct AppearanceSettings: View {
    @Bindable private var store = ThemeStore.shared
    /// What the list's `+` will call the appearance it keeps.
    @State private var newLookName = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            // One card, because there is one question: what does this app look like. The list on the
            // left is whole appearances — the presets, and anything the user kept — and the detail on
            // the right is the parts they are made of. Change any part and the list reads Custom,
            // which is the honest name for "this is yours and it is not one of these yet".
            SettingsCard(title: "Appearance",
                         detail: "The list on the left is whole appearances: a preset names both canvases, the accent and the tone in one click. Change any part and it reads Custom, which is what you are on until the + keeps it under a name.") {
                SettingsRow(label: "Mode") {
                    Segmented(selection: $store.mode, options: AppearanceMode.allCases) { $0.title }
                }
                RowDivider()
                lookMasterDetail
            }
        }
    }

    private var glowLabel: String {
        "\(Int((store.glow * 100).rounded()))%"
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
                            .font(.ui(11, weight: active ? .semibold : .regular))
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

    /// The list on the left and the detail on the right.
    ///
    /// The list is whole appearances and the detail is the parts they are made of, which is the
    /// shape that makes "Custom" mean something: a row can only stand for the current appearance if
    /// it names all of it, and a canvas alone does not.
    private var lookMasterDetail: some View {
        HStack(alignment: .top, spacing: 14) {
            lookList
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1)
            lookDetail
        }
        .padding(.vertical, 6)
    }

    private var lookList: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Above the presets, because it is the state you are in rather than one of the choices.
            if store.isCustomLook { customRow }
            lookGroup("Presets") {
                ForEach(ThemePreset.all) { preset in
                    Button { store.apply(preset) } label: {
                        lookRow(title: preset.title,
                                detail: "\(preset.accent.title) · \(preset.tone.title)",
                                look: preset.look,
                                active: store.currentPreset == preset)
                    }
                    .buttonStyle(.plain)
                    .help(preset.detail)
                    .accessibilityLabel(preset.title)
                    .accessibilityAddTraits(store.currentPreset == preset ? [.isSelected] : [])
                }
            }
            if !store.savedLooks.isEmpty {
                lookGroup("Saved") {
                    ForEach(store.savedLooks) { saved in
                        savedRow(saved)
                    }
                }
            }
            Spacer(minLength: 0)
            lookToolbar
        }
        .frame(width: 168, alignment: .leading)
    }

    /// The two actions the list cannot express as a row: keep the appearance you are on, and throw
    /// away one you kept. Below the list, which is where TablePro puts the same pair.
    private var lookToolbar: some View {
        HStack(spacing: 2) {
            Button { saveCurrentLook() } label: {
                Image(systemName: "plus")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(store.isCustomLook ? Tone.ink : Tone.secondary)
                    .frame(width: 24, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(store.isCustomLook
                  ? "Save this appearance under the name on the right"
                  : "This appearance is already one of the rows above")
            .disabled(!store.isCustomLook)

            Button { deleteSelectedLook() } label: {
                Image(systemName: "minus")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(selectedLook == nil ? Tone.secondary : Tone.ink)
                    .frame(width: 24, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Delete the saved appearance this one is")
            .disabled(selectedLook == nil)

            Spacer(minLength: 0)
        }
    }

    /// The saved look the current appearance is exactly, which is what `−` deletes.
    private var selectedLook: SavedLook? {
        store.savedLooks.first { $0.look == store.currentLook }
    }

    /// Keep the current appearance. A blank name gets one, so the button is never a dead click:
    /// a look nobody named is a look nobody can pick again.
    private func saveCurrentLook() {
        let trimmed = newLookName.trimmingCharacters(in: .whitespacesAndNewlines)
        let name = trimmed.isEmpty ? "Appearance \(store.savedLooks.count + 1)" : trimmed
        if store.saveLook(named: name) != nil { newLookName = "" }
    }

    private func deleteSelectedLook() {
        guard let selected = selectedLook else { return }
        store.deleteLook(selected.id)
    }

    @ViewBuilder
    private func lookGroup<Content: View>(_ title: String,
                                          @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title.uppercased())
                .font(.ui(9.5, weight: .semibold))
                .foregroundStyle(Tone.secondary)
            content()
        }
    }

    /// One appearance in the list: a thumbnail of it, what it is called, and where it came from.
    ///
    /// The thumbnail is a miniature of the shell rather than a colour swatch, because what a look
    /// decides is the whole surface — the canvas, the accent under the tone, the chrome rows — and
    /// one colour cannot say that.
    private func lookRow(title: String, detail: String, look: AppearanceLook,
                         active: Bool) -> some View {
        HStack(spacing: 9) {
            lookThumbnail(look)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.ui(11.5, weight: active ? .semibold : .regular))
                    .foregroundStyle(active ? Tone.ink : Tone.ink.opacity(0.85))
                    .lineLimit(1)
                Text(detail)
                    .font(.ui(9.5))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 6)
        .background(active ? Tone.accent.opacity(0.14) : .clear,
                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle())
    }

    /// The shell in miniature, drawn on the canvas for the appearance in effect. The ink comes from
    /// that canvas's own darkness rather than from the window's, because the thumbnail can be
    /// showing the other appearance from the one it sits in.
    private func lookThumbnail(_ look: AppearanceLook) -> some View {
        let isDark = store.isDarkAppearance
        let canvas = isDark ? look.darkTheme.canvas : look.lightTheme.canvas
        let ink: Color = isDark ? .white : .black
        return ZStack {
            RoundedRectangle(cornerRadius: 4, style: .continuous).fill(canvas)
            VStack(alignment: .leading, spacing: 2) {
                Capsule()
                    .fill(previewFill(look.accent, tone: look.tone))
                    .frame(width: 18, height: 4)
                RoundedRectangle(cornerRadius: 1).fill(ink.opacity(0.28)).frame(height: 2)
                RoundedRectangle(cornerRadius: 1).fill(ink.opacity(0.16)).frame(width: 22, height: 2)
            }
            .padding(5)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(width: 40, height: 26)
        .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous)
            .strokeBorder(Tone.ink.opacity(0.18)))
    }

    /// The state you are in when the appearance is none of the named ones. Deliberately not a
    /// button: there is nothing to go back to, and a click that does nothing is worse than a label.
    private var customRow: some View {
        lookRow(title: "Custom", detail: "not a preset",
                look: store.currentLook, active: true)
            .accessibilityElement(children: .combine)
    }

    private func savedRow(_ saved: SavedLook) -> some View {
        let active = saved.look == store.currentLook
        return HStack(spacing: 0) {
            Button { store.apply(saved.look) } label: {
                lookRow(title: saved.name,
                        detail: "\(saved.look.accent.title) · \(saved.look.tone.title)",
                        look: saved.look, active: active)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(saved.name)
            .accessibilityAddTraits(active ? [.isSelected] : [])
            Button { store.deleteLook(saved.id) } label: {
                Image(systemName: "trash")
                    .font(.system(size: 9))
                    .foregroundStyle(Tone.secondary)
                    .frame(width: 18, height: 18)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Delete \(saved.name)")
        }
    }

    /// The parts the list is made of. Both canvases are here rather than in the list, because a
    /// canvas is one axis of an appearance and not an appearance: picking Midnight alone does not
    /// say which light canvas or accent goes with it.
    private var lookDetail: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(detailTitle)
                .font(.ui(12, weight: .semibold))
                .foregroundStyle(Tone.ink)
            themePreview
            SettingsRow(label: "Dark canvas") { canvasChips(dark: true) }
            RowDivider()
            SettingsRow(label: "Light canvas") { canvasChips(dark: false) }
            RowDivider()
            SettingsRow(label: "Accent") { accentSwatches }
            RowDivider()
            SettingsRow(label: "Tone",
                        trailing: store.tone.isLuminous ? nil : "flat") { tonePicker }
            RowDivider()
            SettingsRow(label: "Glow", trailing: glowLabel) {
                Slider(value: $store.glow, in: 0...1.5)
                    .tint(Tone.accent)
                    .disabled(!store.tone.isLuminous)
            }
            RowDivider()
            // The name the list's `+` saves under. Beside the appearance it names rather than in a
            // dialog, so what is being saved and what it will be called are read together.
            SettingsRow(label: "Name") {
                TextField("Optional", text: $newLookName)
                    .textFieldStyle(.plain)
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.ink)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// What the detail is showing: the preset or saved look it equals, or Custom.
    private var detailTitle: String {
        if let preset = store.currentPreset { return preset.title }
        if let saved = store.savedLooks.first(where: { $0.look == store.currentLook }) {
            return saved.name
        }
        return "Custom"
    }

    /// One chip per canvas for one appearance. The chip is the canvas's own colour, so the row reads
    /// as a set of canvases rather than as a set of names.
    private func canvasChips(dark isDark: Bool) -> some View {
        let themes = AppTheme.allCases.filter { $0.isDark == isDark }
        return HStack(spacing: 6) {
            ForEach(themes) { theme in
                let active = (isDark ? store.darkTheme : store.lightTheme) == theme
                Button {
                    if isDark { store.darkTheme = theme } else { store.lightTheme = theme }
                } label: {
                    VStack(spacing: 3) {
                        RoundedRectangle(cornerRadius: 5, style: .continuous)
                            .fill(theme.canvas)
                            .frame(width: 32, height: 20)
                            .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous)
                                .strokeBorder(active ? Tone.accent.opacity(0.9) : Tone.ink.opacity(0.16),
                                              lineWidth: active ? 2 : 1))
                        Text(theme.title)
                            .font(.ui(9))
                            .foregroundStyle(active ? Tone.ink : Tone.secondary)
                            .lineLimit(1)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(theme.detail)
                .accessibilityLabel(theme.title)
                .accessibilityAddTraits(active ? [.isSelected] : [])
            }
            Spacer(minLength: 0)
        }
    }

    /// The shell in miniature: the chosen canvas, the accent under the current tone, and three
    /// chrome rows. The ink comes from the theme's own darkness rather than from the window's,
    /// because the canvas painted here can belong to the other appearance.
    private var themePreview: some View {
        let ink: Color = store.theme.isDark ? .white : .black
        return ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous).fill(store.theme.canvas)
            VStack(alignment: .leading, spacing: 6) {
                Capsule()
                    .fill(previewFill(store.accent, tone: store.tone))
                    .frame(width: 56, height: 9)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.24)).frame(height: 4)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.13)).frame(width: 92, height: 4)
                RoundedRectangle(cornerRadius: 2).fill(ink.opacity(0.13)).frame(width: 68, height: 4)
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(height: 86)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .strokeBorder(Tone.ink.opacity(0.12)))
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

// MARK: Editor

/// The Editor pane, which today is the editor's typography: which family the chrome and the code
/// draw in.
///
/// Two choices rather than one, because they are two jobs. The chrome wants whatever the user reads
/// labels in; the code wants a fixed-pitch face, and every client this app is measured against
/// (DataGrip, Navicat) separates the two. The lists are read from the families actually installed
/// on this machine — see `FontChoice` — so a picker never offers a font that would silently fall
/// back to the system one. TablePro's Editor pane is where SQL-editor preferences live; when this
/// app grows any, this is where they go.
struct EditorSettings: View {
    @Bindable private var store = ThemeStore.shared
    /// The editor's own switches. Not appearance, which is why they are not on `ThemeStore`: they
    /// change how the text view lays out and what it draws over the text.
    @Bindable private var prefs = EditorPreferences.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            SettingsCard(title: "SQL Editor",
                         detail: "What the editor does with the text: the gutter, the bands behind the caret, wrapping and folding. These are not the palette — the fonts below are — so they change nothing outside the text view.") {
                editorToggle("Show line numbers", isOn: $prefs.showLineNumbers)
                RowDivider()
                editorToggle("Highlight current line", isOn: $prefs.highlightCurrentLine)
                RowDivider()
                editorToggle("Highlight current statement", isOn: $prefs.highlightCurrentStatement)
                RowDivider()
                editorToggle("Word wrap", isOn: $prefs.wordWrap)
                RowDivider()
                editorToggle("Code folding", isOn: $prefs.codeFolding)
                RowDivider()
                editorToggle("Show invisible characters", isOn: $prefs.showInvisibles)
                RowDivider()
                editorToggle("Auto-uppercase keywords", isOn: $prefs.autoUppercaseKeywords)
                RowDivider()
                editorToggle("Run button beside each statement", isOn: $prefs.runButtonPerStatement)
                RowDivider()
                editorToggle("Query parameters (:name)", isOn: $prefs.queryParameters)
                RowDivider()
                HStack(spacing: 8) {
                    Text("Tab width")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    Text("\(prefs.tabWidth) spaces")
                        .font(.code(11))
                        .foregroundStyle(Tone.secondary)
                    Stepper("", value: $prefs.tabWidth, in: 1...8)
                        .labelsHidden()
                }
                .padding(.vertical, 6)
            }

            SettingsCard(title: "Interface",
                         detail: "The family the chrome draws in: labels, tabs, buttons and every panel. System means the macOS system font, which is what the app ships with.") {
                SettingsRow(label: "Family", trailing: uiLabel) {
                    familyPicker(selection: $store.uiFontFamily,
                                 families: FontChoice.uiFamilies,
                                 fallback: "System")
                }
                RowDivider()
                // A sample set in the chosen family, at the size the chrome actually uses. The
                // point of a font picker is to see the font, and a picker that only lists names
                // makes the user pick blind.
                SettingsRow(label: "Sample") {
                    Text("The quick brown fox jumps over the lazy dog · 0123456789")
                        .font(FontChoice.sample(family: store.uiFontFamily, size: 12))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }

            SettingsCard(title: "Code",
                         detail: "The family the code draws in: the SQL editor, the result grid, and every monospaced value. System means the system's fixed-pitch font.") {
                SettingsRow(label: "Family", trailing: codeLabel) {
                    familyPicker(selection: $store.codeFontFamily,
                                 families: FontChoice.codeFamilies,
                                 fallback: "System")
                }
                RowDivider()
                SettingsRow(label: "Sample") {
                    Text("SELECT id, name FROM warehouse.orders WHERE total > 100;")
                        .font(FontChoice.sample(family: store.codeFontFamily, size: 12))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                // The two counts are the honest statement of what the picker can offer: a family
                // that is not fixed-pitch is not in the code list, and saying how many were dropped
                // is better than a list that silently looks short.
                Text("\(FontChoice.codeFamilies.count) of \(FontChoice.uiFamilies.count) installed families are fixed-pitch and offered here.")
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary.opacity(0.85))
            }
        }
    }

    private var uiLabel: String {
        store.uiFontFamily.isEmpty ? "System" : store.uiFontFamily
    }

    private var codeLabel: String {
        store.codeFontFamily.isEmpty ? "System" : store.codeFontFamily
    }

    /// A `Menu` rather than a `Picker`: the list is every font on the machine, which is hundreds of
    /// rows, and a menu scrolls and searches without turning the settings window into a list of
    /// fonts. Each row is drawn *in its own family* so the list previews what it offers.
    private func familyPicker(selection: Binding<String>, families: [String], fallback: String) -> some View {
        Menu {
            Button(fallback) { selection.wrappedValue = FontChoice.system }
            Divider()
            ForEach(families, id: \.self) { family in
                Button(family) { selection.wrappedValue = family }
            }
        } label: {
            HStack(spacing: 6) {
                Text(selection.wrappedValue.isEmpty ? fallback : selection.wrappedValue)
                    .font(FontChoice.sample(family: selection.wrappedValue, size: 11.5))
                    .lineLimit(1)
                Spacer(minLength: 8)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(height: Metrics.control)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(Tone.ink.opacity(0.10)))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// One switch on a card, in the shape the other panes use: the label at the leading edge and
    /// the switch at the trailing one, which is where TablePro puts it and where the eye looks.
    private func editorToggle(_ title: String, isOn: Binding<Bool>) -> some View {
        HStack(spacing: 12) {
            Text(title)
                .font(.ui(11.5))
                .foregroundStyle(Tone.ink.opacity(0.9))
            Spacer(minLength: 12)
            Toggle("", isOn: isOn)
                .toggleStyle(.switch)
                .controlSize(.small)
                .labelsHidden()
        }
        .padding(.vertical, 6)
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
                    .font(.ui(11))
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
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    if let shortcut = model.shortcutScheme.shortcut(for: action) {
                        Text(shortcut.display)
                            .font(.code(11, weight: .medium))
                            .foregroundStyle(Tone.ink.opacity(0.85))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Tone.ink.opacity(0.07),
                                        in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    } else {
                        Text("not bound")
                            .font(.ui(10.5))
                            .foregroundStyle(Tone.secondary.opacity(0.7))
                    }
                }
                .padding(.vertical, 6)
            }
        }
    }
}

/// What the app keeps, and for how long.
///
/// The history is the only thing this app writes without being asked, so the two controls that
/// govern it belong where they can be reached rather than as constants in the code.
struct DataSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 18) {
            SettingsCard(title: "Query history",
                         detail: "Every Run is written down with its statement, its connection, how long it took, and how it ended. Export and Explain are not recorded: a plan is not a result, and a history that mixed looking at rows with writing them somewhere would answer neither question.") {
                Toggle(isOn: $model.recordsHistory) {
                    Text("Record every Run")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                }
                .toggleStyle(.switch)
                .controlSize(.small)
                .padding(.vertical, 6)

                RowDivider()

                HStack(spacing: 8) {
                    Text("Rows the panel reads")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    TextField("", value: $model.historyLimit, format: .number)
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .font(.code(11.5))
                        .frame(width: 64)
                    Stepper("", value: $model.historyLimit, in: 50...5000, step: 50)
                        .labelsHidden()
                }
                .padding(.vertical, 6)

                Text("A cap rather than a filter. The engine reads newest first, so lowering it hides the oldest entries and keeps the recent ones. Turning recording off leaves what is already written alone; clearing it happens in the History panel.")
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            SettingsCard(title: "Statement timeout",
                         detail: "How long a statement may run before the server stops it. The engine puts the bound in force with each server's own mechanism — PostgreSQL's statement_timeout, Trino's query_max_run_time, MySQL's max_execution_time — so a statement that overruns is cancelled and releases its resources even if this window is gone. A local timer would only stop the reading, not the work.") {
                HStack(spacing: 8) {
                    Text("Stop a statement after")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    TextField("", value: $model.statementTimeoutMS, format: .number)
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .font(.code(11.5))
                        .frame(width: 72)
                    Text("ms")
                        .font(.ui(11))
                        .foregroundStyle(Tone.secondary)
                    Stepper("", value: $model.statementTimeoutMS, in: 0...600_000, step: 5_000)
                        .labelsHidden()
                }
                .padding(.vertical, 6)

                Text("Zero means no bound. The setting applies to every run that reaches a server — Preview, Count, Explain and Export alike — and survives a restart. What a connection refuses outright is its Safe Mode, chosen per connection in its editor.")
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            SettingsCard(title: "Result rows",
                         detail: "How many rows a new query fetches before it stops. A Run reads the first page and writes nothing, so this bounds how much is looked at rather than what a statement could return. Each tab keeps its own number once you change it in the grid; this is only what a new tab starts with.") {
                HStack(spacing: 8) {
                    Text("A new query fetches")
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink.opacity(0.9))
                    Spacer(minLength: 12)
                    TextField("", value: $model.defaultRowLimit, format: .number.grouping(.never))
                        .textFieldStyle(.plain)
                        .multilineTextAlignment(.trailing)
                        .font(.code(11.5))
                        .frame(width: 72)
                    Text("rows")
                        .font(.ui(11))
                        .foregroundStyle(Tone.secondary)
                    Stepper("", value: $model.defaultRowLimit, in: 1...1_000_000, step: 100)
                        .labelsHidden()
                }
                .padding(.vertical, 6)

                Text("The grid's own field overrides this for the tab in front, and a restored session keeps each tab's number. Export is not bounded by it: an export streams the whole statement to a file, which is the point of it.")
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
