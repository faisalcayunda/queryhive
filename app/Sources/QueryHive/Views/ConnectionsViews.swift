import SwiftUI

/// 22pt-default rounded tile filled with a connection's colour gradient, carrying that driver's
/// brand mark. Shared by the tree, the toolbar picker and the title strip so a connection reads the
/// same way everywhere it appears.
func connectionTile(_ connection: Connection, size: CGFloat = 22) -> some View {
    connectionTile(colour: connection.color, kind: connection.kind, size: size)
}

/// The same tile built from a **tree node's own values**.
///
/// Split out because a tree row must not read the model's `connections`: that would subscribe every
/// visible row to the array, so one edit — a colour, a rename, a move between groups — would re-run
/// all of them. This is the same trap the selection hit. A node already carries its colour and its
/// driver, so the row can draw the tile without asking anyone anything.
func connectionTile(colour: ConnectionColor, kind: ConnectionKind, size: CGFloat = 22) -> some View {
    RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
        // The tile's own subtle ramp, flattened under a flat tone like every other gradient.
        .fill(ThemeStore.shared.tone.isLuminous
              ? LinearGradient(colors: [colour.color, colour.color.opacity(0.55)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
              : LinearGradient(colors: [colour.color, colour.color],
                               startPoint: .topLeading, endPoint: .bottomTrailing))
        .frame(width: size, height: size)
        .overlay {
            if let logo = DriverLogo.image(for: kind) {
                Image(nsImage: logo).resizable().scaledToFit()
                    .frame(width: size * 0.68, height: size * 0.68)
            } else {
                Image(systemName: kind.symbol)
                    .font(.system(size: size * 0.46, weight: .semibold)).foregroundStyle(.white)
            }
        }
}

/// The toolbar's connection dropdown. `.menuStyle(.borderlessButton)` drops any custom label in
/// favour of a plain system title, so the colour/name/target never show — fixed by pairing
/// `.menuStyle(.button)` with `.buttonStyle(.plain)` and hiding the built-in indicator in favour
/// of our own chevron.
struct ConnectionPickerButton: View {
    @Environment(AppModel.self) private var model
    @Binding var selection: UUID?

    var body: some View {
        Menu {
            ForEach(model.connections) { connection in
                // The tag and any Safe Mode limit ride on the item, so choosing the connection to
                // run against is never done without seeing which one is production.
                let marks = BadgeSpec.badges(for: connection, in: .breadcrumb).map(\.label)
                Button("\(connection.name) — \(connection.displayTarget)"
                       + (marks.isEmpty ? "" : "  [\(marks.joined(separator: " · "))]")) {
                    selection = connection.id
                }
            }
            Divider()
            Button("Edit Connections…") { model.presentConnectionEditor(selection) }
            Button("New Connection…") { model.presentConnectionEditor(nil) }
            Button("Export Connection List…") { model.presentConnectionListExport() }
                .disabled(model.connections.isEmpty)
        } label: {
            HStack(spacing: 7) {
                if let connection = model.connections.first(where: { $0.id == selection }) {
                    connectionTile(connection, size: 18)
                    Text(connection.name).font(.ui(12, weight: .semibold)).lineLimit(1)
                } else {
                    Text(model.connections.isEmpty ? "No connections" : "Choose a connection…")
                        .font(.ui(12))
                        .foregroundStyle(Tone.secondary)
                }
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 8, weight: .bold)).foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(maxWidth: .infinity)
            .frame(height: 28)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Tone.ink.opacity(0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .help(model.connections.first { $0.id == selection }?.displayTarget ?? "Choose a connection")
    }
}

struct ColorSwatchPicker: View {
    @Binding var selection: ConnectionColor

    var body: some View {
        HStack(spacing: 10) {
            ForEach(ConnectionColor.allCases) { color in
                Button { selection = color } label: {
                    Circle()
                        .fill(color.color)
                        .frame(width: 22, height: 22)
                        .overlay(Circle().strokeBorder(Tone.ink.opacity(selection == color ? 0.9 : 0.15), lineWidth: selection == color ? 2 : 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(color.rawValue.capitalized)
                .help(color.rawValue.capitalized)
            }
        }
    }
}

/// Runs `test` with unsaved draft values, independent of anything a query tab is doing.
enum TestState {
    case idle, running
    case success(Int)
    case failure(String)
}

/// The connection editor, as Navicat presents it: a modal connection-properties dialog with
/// Test, Save and Delete. Nothing behind it changes until Save.
struct ConnectionEditorSheet: View {
    let target: ConnectionEditorTarget
    @Environment(AppModel.self) var model

    private var connectionID: UUID? { target.connectionID }
    @Environment(\.dismiss) private var dismiss

    /// Which of the sheet's three states is showing. A saved connection starts at `.form`.
    enum Step { case typePicker, url, form }

    @State var step = Step.typePicker
    @State private var urlText = ""
    @State private var urlError: String?
    @State var editingID: UUID?
    /// Every field of the connection and the secrets typed beside it (`ConnectionDraft`).
    @State var draft = ConnectionDraft.new()
    @State var aliasPreview: Result<SSHAliasInfo, SSHAliasError>?
    /// What the URL step read and left out, for one line on the form.
    @State var urlNote: String?
    /// Whether the last Test failed on the host key, so trusting the key can run it again.
    @State var lastTestWasHostKey = false
    @State private var confirmDelete = false
    @State var testState = TestState.idle
    @State private var testProcess: (any EngineRun)?
    @State private var testRun = UUID()
    @State var attemptedSave = false
    /// The id a brand-new connection will save under, reused across retries so a Save that fails
    /// (a JSON write error) doesn't mint a fresh UUID next try and orphan the Keychain item the
    /// first attempt already wrote.
    @State private var draftID = UUID()
    /// The section the sidebar has selected.
    @State var section = ConnectionFormSection.general
    @FocusState var focus: ConnectionField?
    /// A field to focus once the section that holds it is on screen: `FocusState` ignores a field
    /// that is not mounted, so the jump to another section hands it over to that section.
    @State private var pendingFocus: ConnectionField?

    /// Set by the snapshot scenes that show a section other than General. Never set by the app itself.
    nonisolated(unsafe) static var initialSectionForSnapshot: ConnectionFormSection?
    /// Set by the snapshot scene that shows a new connection's form, not the type grid. Never set
    /// by the app itself.
    nonisolated(unsafe) static var newKindForSnapshot: ConnectionKind?

    private var original: Connection? { model.connections.first { $0.id == editingID } }

    private var isDirty: Bool { draft.isDirty(against: original) }


    var resolvedAlias: SSHAliasInfo? {
        if case .success(let info)? = aliasPreview { return info }
        return nil
    }

    /// What is missing or wrong, by field. Name, host and user are what the engine cannot invent,
    /// and Postgres cannot open a connection without a database at all; the tunnel's host, user and
    /// key file count when it is on (an alias may supply the user and the key). Port and encryption
    /// always carry a value; a blank password simply means "connect without one".
    var issues: [ConnectionField] {
        ConnectionFormIssues.fields(for: draft.formState, resolved: resolvedAlias)
    }

    var missingRequired: Set<String> { Set(issues.map(\.rawValue)) }

    var body: some View {
        VStack(spacing: 0) {
            switch step {
            // The type grid and the URL step keep the 620 pt column they have always had, centred
            // in the wider frame the form needs.
            case .typePicker: typePicker.frame(maxWidth: 620)
            case .url: urlStep.frame(maxWidth: 620)
            case .form: formStep
            }
        }
        // The size of the form with its sidebar. Every pane inside is allowed to be shorter than
        // it asks for (`minHeight: 0`), so this frame is never the thing hiding a taller minimum.
        .frame(width: 820, height: 620)
        .background(Tone.canvas)
        // No `.preferredColorScheme(.dark)` here. It used to pin the sheet dark, and that is what
        // made the editor unreadable in light mode: `Tone.canvas` follows the *stored mode* (light
        // mode picks the light theme) while `Tone.ink` is a dynamic `NSColor` that follows the
        // *effective appearance*. Pinning dark made the sheet's ink resolve white over a canvas the
        // theme had already drawn light, so every label came out white on white. The sheet now
        // inherits the appearance of the window that presents it, which is what every other surface
        // in the app does.
        .confirmationDialog("Delete \(draft.name.isEmpty ? "this connection" : draft.name)?", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive) { delete() }
        } message: {
            Text("This removes the saved connection and its Keychain password. This can't be undone.")
        }
        .onAppear { load(); applySnapshotHooks() }
        .onDisappear { testProcess?.terminate() }
        // A host key refused during Test is asked about here, over this sheet: a sheet cannot be
        // presented over a sheet from the view that presented the first one.
        .hostKeySheet(active: true)
        // Trusting the key and running Test again are separate acts, but this one is what the
        // person was in the middle of.
        .onChange(of: HostKeyCenter.shared.trustedCount) { _, _ in
            if lastTestWasHostKey, step == .form { runTest() }
        }
    }

    /// Step one, and only for a connection that does not exist yet: pick the type from a grid of
    /// tiles, the way Navicat does. Choosing the driver first is not decoration — the three have
    /// different default ports, different required fields and different tree shapes, so the form
    /// cannot be drawn until the driver is known. Editing an existing connection skips straight
    /// to the form.
    private var typePicker: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("New Connection")
                .font(.ui(18, weight: .bold, rounded: true))
                .padding(.horizontal, 20)
                .frame(height: 84, alignment: .leading)
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            VStack(alignment: .leading, spacing: 14) {
                SectionLabel(text: "Select a connection type")
                HStack(spacing: 12) {
                    ForEach(ConnectionKind.allCases) { option in
                        ConnectionTypeTile(kind: option) { choose(option) }
                    }
                }
                Text("The driver decides the default port, which fields are required, and what the "
                     + "object tree can browse.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(20)
            Spacer()
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            HStack(spacing: 10) {
                PillButton(title: "New Connection with URI…", symbol: "link") { step = .url }
                // Each of these closes this sheet first. They run a modal `NSOpenPanel`, and
                // starting one from inside a sheet being dismissed puts the panel behind the sheet
                // on its way out — the wait is the sheet's own dismissal animation, not a guess.
                Menu {
                    Button("From Navicat…") { importAfterDismiss { model.presentNavicatImport() } }
                    Button("From DBeaver…") { importAfterDismiss { model.presentDBeaverImport() } }
                    Button("From DataGrip…") { importAfterDismiss { model.presentDataGripImport() } }
                    Divider()
                    Button("QueryHive Connection List…") { importAfterDismiss { model.presentConnectionListImport() } }
                    Button("Export Connection List…") { importAfterDismiss { model.presentConnectionListExport() } }
                        .disabled(model.connections.isEmpty)
                } label: {
                    Label("Import…", systemImage: "square.and.arrow.down")
                        .font(.ui(12.5, weight: .medium))
                        .foregroundStyle(Tone.ink)
                        .padding(.horizontal, 13)
                        .frame(height: 28)
                        .background(Tone.ink.opacity(0.10), in: Capsule())
                        .overlay(Capsule().strokeBorder(Tone.ink.opacity(0.14)))
                        .contentShape(Capsule())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                Spacer()
                PillButton(title: "Cancel", role: .quiet) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 20)
            .frame(height: 62)
        }
    }

    private func importAfterDismiss(_ present: @escaping @MainActor () -> Void) {
        dismiss()
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(350))
            present()
        }
    }

    /// Step two, only reached from "New Connection with URI…". It parses into the *form* rather
    /// than saving straight away, so the user sees what the URL actually meant before committing
    /// to it — a URL with an unexpected port or database is easy to paste without reading.
    private var urlStep: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("New Connection with URI")
                .font(.ui(18, weight: .bold, rounded: true))
                .padding(.horizontal, 20)
                .frame(height: 84, alignment: .leading)
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            VStack(alignment: .leading, spacing: 12) {
                SectionLabel(text: "Connection URI")
                TextField("postgresql://user:password@host:5432/mydb", text: $urlText)
                    .field(invalid: urlError != nil)
                    .onSubmit { applyURL() }
                if let urlError {
                    Text(urlError).font(.ui(11)).foregroundStyle(Tone.coral)
                }
                Text("""
                     trino://user:password@host:8443/hive/analytics
                     postgresql://user:password@host:5432/mydb
                     mysql://user:password@host:3306/mydb
                     """)
                    .font(.code(11))
                    .foregroundStyle(Tone.ink.opacity(0.4))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(20)
            Spacer()
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            HStack(spacing: 10) {
                PillButton(title: "Back", symbol: "chevron.left", role: .quiet) { step = .typePicker }
                Spacer()
                HubButton(title: "Continue", symbol: "arrow.right", hue: .connection) { applyURL() }
                    .disabled(urlText.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(.horizontal, 20)
            .frame(height: 62)
        }
    }

    private var formStep: some View {
        HStack(spacing: 0) {
            ConnectionFormSidebar(kind: draft.kind, isSaved: editingID != nil, selection: $section,
                                  issueCounts: [:]) { confirmDelete = true }
                .frame(width: 200)
                .frame(minHeight: 0, maxHeight: .infinity)
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(width: 1)
            VStack(spacing: 0) {
                ConnectionFormHeader(title: editingID == nil ? "New Connection" : (draft.name.isEmpty ? "Connection" : draft.name),
                                     isDirty: isDirty)
                Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
                ScrollView {
                    pane
                        .background(FrameMarker(name: "connection-pane-\(section.rawValue)"))
                        .padding(20)
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                }
                .id(section)
                .frame(minHeight: 0, maxHeight: .infinity)
                .task(id: section) {
                    guard let field = pendingFocus else { return }
                    pendingFocus = nil
                    await Task.yield()
                    focus = field
                }
                Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
                ConnectionFormFooter(isTesting: isTesting, status: footerStatus, canSave: isDirty,
                                     onTest: runTest, onCancel: { dismiss() }, onSave: save)
            }
            .frame(minWidth: 0, maxWidth: .infinity)
        }
    }

    private var isTesting: Bool {
        if case .running = testState { return true }
        return false
    }

    private var footerStatus: ConnectionFormFooter.Status {
        switch testState {
        case .idle, .running: .none
        case .success(let level):
            .success("Connected · \(pluralized(level, draft.kind == .postgres ? "schema" : "catalog"))")
        case .failure(let message): .failure(message)
        }
    }

    /// Save and Test with something missing: the first section that has a problem comes up (the one
    /// in view first), and the first field in it takes the focus. A field in a section that is not
    /// on screen yet cannot be focused, so it waits for its section.
    private func reveal(_ problems: [ConnectionField]) {
        attemptedSave = true
        let grouped = ConnectionFormIssues.bySection(problems)
        let order = [section] + ConnectionFormSection.allCases.filter { $0 != section }
        if let target = order.first(where: { grouped[$0] != nil }), let field = grouped[target]?.first {
            if target == section { focus = field } else { pendingFocus = field; section = target }
        }
        Announcer.post(ConnectionFormIssues.summary(problems, visible: section))
    }

    /// The snapshot scenes that open on another section, or on a new connection's form.
    private func applySnapshotHooks() {
        section = Self.initialSectionForSnapshot ?? .general
        if editingID == nil, let kind = Self.newKindForSnapshot {
            draft.choose(kind)
            step = .form
        }
    }

    /// Picking a tile is the only place the driver is chosen; the form then has no type control,
    /// because changing it would invalidate half of what is already filled in.
    private func choose(_ option: ConnectionKind) {
        draft.choose(option)
        step = .form
        if Self.newKindForSnapshot == nil { pendingFocus = .name }
    }


    private func applyURL() {
        let parsed: ConnectionURL.Parsed
        do {
            guard let read = try ConnectionURL.parse(urlText) else {
                urlError = "Couldn't read that. Expected something like postgresql://user:password@host:5432/mydb"
                return
            }
            parsed = read
        } catch {
            // A TLS word this app can't honour is named, never dropped: the form would otherwise
            // open on the default and a certificate the URL asked to verify would not be.
            urlError = error.localizedDescription
            return
        }
        urlError = nil
        urlNote = parsed.ignored.isEmpty ? nil : "Ignored from the URL: " + parsed.ignored.joined(separator: ", ") + "."
        draft.apply(parsed)
        step = .form
        if Self.newKindForSnapshot == nil { pendingFocus = .name }
    }

    private func load() {
        testProcess?.terminate()
        testProcess = nil
        testRun = UUID()
        editingID = connectionID
        // Editing an existing connection goes straight to its fields; only a new one needs the
        // type chosen first.
        step = connectionID != nil ? .form : (target.startAtURL ? .url : .typePicker)
        testState = .idle
        // After the reset, not before: this line used to sit above it and was silently wiped.
        if let count = target.previewTestCount { testState = .success(count) }
        // Nothing typed and nothing staged for removal carries over: the form never holds a saved
        // secret, so there is nothing else it could start with.
        aliasPreview = nil; urlNote = nil; lastTestWasHostKey = false
        guard let connection = original else {
            draftID = UUID()
            draft = .new()
            return
        }
        draft = ConnectionDraft(connection)
        refreshAliasPreview()
    }

    private func save() {
        attemptedSave = true
        let problems = issues
        guard problems.isEmpty else {
            reveal(problems)
            testState = .failure(ConnectionFormIssues.message(problems.map(\.label)))
            return
        }
        let id = editingID ?? draftID
        let connection = draft.connection(id: id, group: original?.group)
        let operations = ConnectionSecretPlan.operations(draft: draft.secrets, old: original, new: connection)
        // Keychain first: if it throws, the JSON never claims a secret exists that isn't there.
        do {
            try AppModel.apply(operations, for: id)
            var next = model.connections
            if let index = next.firstIndex(where: { $0.id == id }) {
                next[index] = connection
            } else {
                // A connection created from a group's menu is filed into it on creation. Set here
                // rather than on the form, because which folder it belongs in is the tree's
                // business and not a field of the connection — and read once, then cleared, so
                // creating a second connection from the header does not inherit the first's group.
                var created = connection
                created.group = model.newConnectionGroup
                model.newConnectionGroup = nil
                next.append(created)
            }
            try ConnectionStore.save(ConnectionsDocument(groups: model.groups, connections: next))
            model.connections = next
        } catch {
            model.notice = Notice(title: "Couldn't save connection", message: error.localizedDescription)
            return
        }
        // An item an older build left with an access list that still prompts is rewritten now, while
        // the person is saving and expects dialogs (C-6). Best effort: a failure here is not a
        // failed save, except one that deleted the secret and could not put it back, which is
        // reported so it is not found missing later.
        if editingID != nil, AppModel.secretStore is SystemKeychain {
            var lost: [String] = []
            for slot in ConnectionKeychain.Slot.allCases where ConnectionSecretPlan.uses(slot, connection) {
                do { try ConnectionKeychain.renewIfPrompting(slot: slot, for: id) } catch let error as ConnectionKeychain.SecretLost {
                    lost.append(AppModel.secretName(error.slot))
                } catch {}
            }
            if !lost.isEmpty {
                model.notice = Notice(title: "Saved secret lost",
                                      message: ConnectionKeychain.lostMessage(connection: connection.name, secrets: lost))
            }
        }
        model.rebuildTree()
        // A brand-new connection becomes the destination of the tab that opened the sheet; an
        // edit never steals the selection.
        if editingID == nil, let tab = model.selectedTab, tab.connectionID == nil {
            tab.connectionID = id
        }
        dismiss()
    }

    private func delete() {
        guard let id = editingID else { return }
        model.deleteConnection(id)
        dismiss()
    }

    private func runTest() {
        // The same named check Save uses, in the same words, before anything is run.
        let problems = issues
        guard problems.isEmpty else {
            reveal(problems)
            testState = .failure(ConnectionFormIssues.message(problems.map(\.label)))
            return
        }
        testProcess?.terminate()
        testState = .running
        lastTestWasHostKey = false
        let run = UUID()
        testRun = run
        let candidate = draft.connection(id: editingID ?? draftID, group: original?.group)
        // What was typed wins over what is saved (and a secret staged for removal is not saved any
        // more); a slot the connection does not use is not read at all.
        var secrets = ConnectionSecrets()
        do {
            for slot in ConnectionKeychain.Slot.allCases where ConnectionSecretPlan.uses(slot, candidate) {
                let typed = draft.secrets.typed(slot)
                if !typed.isEmpty {
                    secrets[slot] = typed
                } else if let id = editingID, !draft.secrets.removals.contains(slot) {
                    secrets[slot] = try AppModel.storedSecret(slot, for: id)
                }
            }
        } catch {
            testState = .failure("Couldn't read the saved secret for \(draft.name.isEmpty ? "this connection" : draft.name) from Keychain: \(error.localizedDescription)")
            return
        }
        let vars = AppModel.connectionEnvironment(candidate, secrets: secrets)
        var catalogs = 0
        var message: String?
        testProcess = Engine.current.run("test", env: vars, onEvent: { event in
            guard testRun == run else { return }
            if event.event == "test" { catalogs = event.catalogCount ?? 0 }
            if event.event == "error" {
                message = event.message
                lastTestWasHostKey = event.hostKey != nil
            }
        }, onExit: { status, log in
            guard testRun == run else { return }
            testProcess = nil
            if status == 0 {
                testState = .success(catalogs)
            } else {
                testState = .failure(message ?? log.split(separator: "\n").last.map(String.init) ?? "exit status \(status)")
            }
        })
    }
}

/// One tile in the new-connection grid. Navicat's picker is the reference: the type is a
/// picture with a name, not an entry in a dropdown, because it is the one choice that decides
/// everything after it.
struct ConnectionTypeTile: View {
    let kind: ConnectionKind
    let action: () -> Void
    @State private var hovering = false

    /// Each driver's own colour, so the grid reads at a glance. These are the projects' brand
    /// hues, not their logos — the glyph stays this app's own.
    private var hue: Hue {
        switch kind {
        case .trino: Hue(glow: Tone.ice, accent: Tone.violet)
        case .postgres: Hue(glow: Color(hex: 0x5B9BEE), accent: Color(hex: 0x2C5C9E))
        case .mysql: Hue(glow: Color(hex: 0x2BB7E0), accent: Color(hex: 0x00698C))
        }
    }

    var body: some View {
        Button(action: action) {
            VStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(hue.gradient)
                    .overlay {
                        if hue.isLuminous {
                            RoundedRectangle(cornerRadius: 14, style: .continuous)
                                .fill(LinearGradient(colors: [.white.opacity(0.35), .clear],
                                                     startPoint: .top, endPoint: .center))
                        }
                    }
                    .overlay {
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .strokeBorder(hue.isLuminous ? Tone.ink.opacity(0.28) : hue.stroke.opacity(0.6))
                    }
                    .overlay {
                        if let logo = DriverLogo.image(for: kind) {
                            Image(nsImage: logo).resizable().scaledToFit()
                                .frame(width: 44, height: 44)
                                .shadow(color: .black.opacity(0.3), radius: 3, y: 2)
                        } else {
                            Image(systemName: kind.symbol)
                                .font(.system(size: 26, weight: .semibold))
                                .foregroundStyle(.white)
                                .shadow(color: .black.opacity(0.25), radius: 3, y: 2)
                        }
                    }
                    .frame(width: 74, height: 74)
                    .shadow(color: hue.accent.opacity(hovering ? 0.6 : 0.35), radius: hovering ? 16 : 10, y: 4)

                Text(kind.label)
                    .font(.ui(13, weight: .semibold, rounded: true))
                    .foregroundStyle(Tone.ink)
                Text(verbatim: "port \(kind.defaultPort)")
                    .font(.code(11))
                    .foregroundStyle(Tone.secondary)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(Tone.ink.opacity(hovering ? 0.07 : 0.03),
                        in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(Tone.ink.opacity(hovering ? 0.18 : 0.09)))
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

/// The editor header's tile: the driver's own mark on its own colour, so the sheet says which
/// database it is configuring before the form says anything.
struct ConnectionBrandTile: View {
    let kind: ConnectionKind
    var size: CGFloat = 56

    private var hue: Hue {
        switch kind {
        case .trino: Hue(glow: Tone.ice, accent: Tone.violet)
        case .postgres: Hue(glow: Color(hex: 0x5B9BEE), accent: Color(hex: 0x2C5C9E))
        case .mysql: Hue(glow: Color(hex: 0x2BB7E0), accent: Color(hex: 0x00698C))
        }
    }

    var body: some View {
        let tile = RoundedRectangle(cornerRadius: size * 0.26, style: .continuous)
        tile.fill(hue.gradient)
            .overlay {
                if hue.isLuminous {
                    tile.fill(LinearGradient(colors: [.white.opacity(0.35), .clear],
                                             startPoint: .top, endPoint: .center))
                }
            }
            .overlay(tile.strokeBorder(hue.isLuminous ? Tone.ink.opacity(0.3) : hue.stroke.opacity(0.6)))
            .overlay {
                if let logo = DriverLogo.image(for: kind) {
                    Image(nsImage: logo).resizable().scaledToFit()
                        .frame(width: size * 0.56, height: size * 0.56)
                }
            }
            .frame(width: size, height: size)
            .shadow(color: hue.accent.opacity(hue.isLuminous ? 0.5 : 0), radius: size * 0.16, y: size * 0.06)
    }
}
