import SwiftUI

// The connection form's frame: a sidebar of sections, a header, a scrolling pane of cards and rows,
// and a footer (blueprint w13-connection-form §5). The pieces here are plain views over plain
// values, so a test can measure the part of the sheet that does not scroll without the sheet's own
// fixed frame hiding what it asks for. The panes themselves are an extension of
// `ConnectionEditorSheet` further down, because they read and write its state.

// MARK: Pieces

/// A titled card, as the Settings panes draw one: a section label, then the rows. Gaps between
/// cards (18) are visibly larger than the rows inside one, which is what makes a pane read as a
/// few groups. This is a copy of Settings' `SettingsCard`, which is private to that file.
struct ConnectionCard<Content: View>: View {
    var title: String?
    var detail: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let title {
                HStack(spacing: 6) {
                    SectionLabel(text: title)
                    if let detail { HelpHint(text: detail, opensOnHover: true) }
                }
            }
            VStack(alignment: .leading, spacing: 0) { content }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glass(12)
    }
}

/// A hairline between two rows of one card (a copy of Settings' private `RowDivider`).
struct ConnectionRowDivider: View {
    var body: some View {
        Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
    }
}

/// One row: the name of the thing on the left, its control on the right. The label is one line;
/// a label that is cut short has its whole text as the tooltip. Whatever the control builder
/// holds is stacked under the control's own line, so a hint or a "Required" sits in the value
/// column and not under the label.
struct ConnectionFormRow<Control: View>: View {
    static var labelWidth: CGFloat { 128 }

    let label: String
    /// A sentence that explains the row and does not depend on what is chosen. It sits behind a
    /// "?" beside the label (`HelpHint`: a tooltip on hover, a popover on click or Space) instead
    /// of taking a line under the control. What depends on the state, and what is a warning, stays
    /// where it can be read.
    var hint: String?
    @ViewBuilder let control: Control

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            HStack(spacing: 4) {
                Text(label)
                    .font(.ui(13))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                    .help(label)
                    .accessibilityHidden(true)
                if let hint {
                    HelpHint(text: hint, opensOnHover: true)
                        .background(FrameMarker(name: "connection-hint"))
                }
            }
            .frame(width: Self.labelWidth, height: 32, alignment: .leading)
            VStack(alignment: .leading, spacing: 5) { control }
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 5)
        .frame(minHeight: 36, alignment: .top)
        .background(FrameMarker(name: "connection-row-\(label)"))
    }
}

extension View {
    /// What VoiceOver calls a row's control, and whether it must be filled in. The row's visible
    /// label is hidden from VoiceOver so it is not read twice.
    ///
    /// `hint` is the caption that moved behind the row's "?": the control keeps it as its own
    /// accessibility hint, so it is read where the control is focused and not only on the icon.
    func formControl(_ label: String, required: Bool = false, hint: String? = nil) -> some View {
        accessibilityLabel(label)
            .accessibilityHint([required ? "Required." : nil, hint].compactMap { $0 }.joined(separator: " "))
    }
}

/// The one control for how the connection is encrypted, beside the host it is about. It reads and
/// writes through the draft (`onSelect` is `ConnectionDraft.setTLS`), shows the stored choice even
/// when it is not one the menu offers, and is never empty. The glyph is decoration: the title and
/// the spoken value carry the meaning.
struct ConnectionEncryptionMenu: View {
    /// The control is this wide whatever its title says, so the host field beside it is the same
    /// width for every driver and every stored choice. A title that does not fit is cut, and the
    /// whole of it is the tooltip.
    static let width: CGFloat = 184

    let current: ConnectionTLSChoice
    let options: [ConnectionTLSChoice]
    /// Something the choice's own help does not say, for the tooltip and VoiceOver.
    var extraHelp: String?
    let onSelect: (ConnectionTLSChoice) -> Void

    private var help: String { [current.help, extraHelp].compactMap { $0 }.joined(separator: " ") }

    var body: some View {
        Menu {
            ForEach(options) { option in
                Button { onSelect(option) } label: {
                    if option == current { Label(option.title, systemImage: "checkmark") } else { Text(option.title) }
                }
                .help(option.help)
            }
        } label: {
            HStack(spacing: 7) {
                Image(systemName: current.glyph)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                    .accessibilityHidden(true)
                Text(current.title)
                    .font(.ui(12))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 10)
            .frame(width: Self.width, height: 30)
            .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Tone.ink.opacity(0.1)))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(help)
        .accessibilityLabel("Encryption")
        .accessibilityValue(current.spokenDescription)
        .accessibilityHint(help)
        .background(FrameMarker(name: "connection-encryption"))
    }
}

/// The list of sections, with the driver's mark above and, for a saved connection, Delete below.
struct ConnectionFormSidebar: View {
    let kind: ConnectionKind
    let isSaved: Bool
    @Binding var selection: ConnectionFormSection
    /// How many fields need attention in each section; a section that is absent has none.
    let issueCounts: [ConnectionFormSection: Int]
    let onDelete: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                ConnectionBrandTile(kind: kind, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(kind.label).font(.ui(12, weight: .semibold)).lineLimit(1)
                    // One short line either way, so the title above does not move between a new
                    // and a saved connection (the driver's name is the title already).
                    Text(isSaved ? "Saved connection." : "Not saved yet.")
                        .font(.ui(11)).foregroundStyle(Tone.secondary).lineLimit(1)
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 64, alignment: .leading)
            VStack(spacing: 2) {
                ForEach(ConnectionFormSection.allCases) { section in item(section) }
            }
            .padding(.horizontal, 8)
            .padding(.top, 4)
            Spacer(minLength: 0)
            if isSaved {
                PillButton(title: "Delete Connection…", symbol: "trash", role: .destructive, compact: true, action: onDelete)
                    .padding(12)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Tone.ink.opacity(0.03))
    }

    /// What VoiceOver adds to a section's name: how many of its fields need attention, or nothing.
    static func attentionValue(_ count: Int) -> String {
        count > 0 ? "\(count) \(count == 1 ? "field needs" : "fields need") attention" : ""
    }

    private func item(_ section: ConnectionFormSection) -> some View {
        let active = selection == section
        let count = issueCounts[section] ?? 0
        return Button {
            // A screen reader hears the pane change; focus stays on the sidebar.
            if selection != section { Announcer.post(section.title) }
            selection = section
        } label: {
            HStack(spacing: 8) {
                Image(systemName: section.symbol)
                    .font(.system(size: 13, weight: .medium))
                    .frame(width: 18)
                Text(section.title)
                    .font(.ui(13, weight: active ? .semibold : .regular))
                    .lineLimit(1)
                Spacer(minLength: 4)
                if count > 0 {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(Tone.amber)
                        .background(FrameMarker(name: "connection-badge-\(section.rawValue)-\(count)"))
                        .accessibilityHidden(true)
                }
            }
            .foregroundStyle(active ? Tone.ink : Tone.secondary)
            .padding(.horizontal, 8)
            .frame(height: 30)
            .background(Tone.ink.opacity(active ? 0.14 : 0), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .background(FrameMarker(name: "connection-section-\(section.rawValue)"))
        .accessibilityLabel(section.title)
        .accessibilityValue(Self.attentionValue(count))
        .accessibilityAddTraits(active ? [.isSelected] : [])
    }
}

/// The title of the connection, and the mark that it has changes nobody saved.
struct ConnectionFormHeader: View {
    let title: String
    let isDirty: Bool
    /// Where the "Change Type…" button leads: back to the driver grid. `nil` hides it.
    var driverName: String?
    var onChangeType: () -> Void = {}

    var body: some View {
        HStack(spacing: 12) {
            Text(title)
                .font(.ui(17, weight: .bold, rounded: true))
                .lineLimit(1)
            Spacer(minLength: 12)
            if isDirty {
                Label("Unsaved changes", systemImage: "exclamationmark.circle")
                    .font(.ui(11, weight: .medium))
                    .foregroundStyle(Tone.amber)
                    .labelStyle(.titleAndIcon)
            }
            if let driverName {
                PillButton(title: "Change Type…", symbol: "arrow.triangle.2.circlepath", compact: true, action: onChangeType)
                    .help("Pick a different driver — this clears the fields that only applied to \(driverName)")
            }
        }
        .padding(.horizontal, 20)
        .frame(height: 52)
    }
}

/// Left has the action and its outcome; right has the terminal choices, in the order a dialog's
/// footer always puts them.
struct ConnectionFormFooter: View {
    enum Status: Equatable {
        case none
        case success(String)
        case failure(String)
        /// What is missing or wrong in the form, as one line.
        case problem(String)
    }

    let isTesting: Bool
    let status: Status
    let canSave: Bool
    let onTest: () -> Void
    let onCancel: () -> Void
    let onSave: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            // The running state keeps the capsule's shape so the footer does not reflow the
            // moment a test starts — it is the same button, mid-press.
            if isTesting {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.small)
                    Text("Testing…").font(.ui(12.5, weight: .medium))
                }
                .foregroundStyle(Tone.ink)
                .padding(.horizontal, 13)
                .frame(height: 28)
                .background(Tone.ink.opacity(0.10), in: Capsule())
                .overlay(Capsule().strokeBorder(Tone.ink.opacity(0.14)))
            } else {
                PillButton(title: "Test Connection", symbol: "bolt", action: onTest)
                    // ⌘↩, not ⌘T: ⌘T is New Query in the QueryHive scheme, and ⌘↩ belongs to the sheet
                    // alone because the sheet is modal (it is the key window, so its key wins).
                    .keyboardShortcut(SheetShortcut.testConnection.keyboard)
                    .help("Test Connection (\(SheetShortcut.testConnection.display))")
            }
            // The status has all the room between Test and Cancel, so a summary wraps at the width that
            // is there and not at what a spacer left it. Empty while a test runs.
            ZStack(alignment: .leading) {
                // Takes the room even when there is nothing to say, which keeps the buttons where they are.
                Color.clear.frame(height: 1)
                if !isTesting { statusLine }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            PillButton(title: "Cancel", role: .quiet, action: onCancel)
                .keyboardShortcut(.cancelAction)
                .background(FrameMarker(name: "connection-footer-cancel"))
            HubButton(title: "Save", symbol: "checkmark", hue: .connection, action: onSave)
                .disabled(!canSave)
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .frame(height: 56)
        .background(FrameMarker(name: "connection-footer"))
    }

    /// The last test's outcome, on one line next to the button. A failure's message can be long
    /// — the engine prefixes it with the exception class and Trino's connection errors run to a
    /// paragraph — so the class prefix is dropped for reading and the whole thing stays in the
    /// tooltip.
    @ViewBuilder private var statusLine: some View {
        switch status {
        case .none:
            EmptyView()
        case .success(let text):
            line(text, tint: Tone.mint, truncation: .tail)
        case .failure(let message):
            // Middle, not tail: a connection error's cause is at the end of the sentence
            // ("… Connection refused"), and its class prefix is at the start.
            line(Self.shortDiagnosis(message), tint: Tone.coral, truncation: .middle)
                .help(message)
        case .problem(let text):
            HStack(spacing: 7) {
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.amber)
                    .accessibilityHidden(true)
                // Two lines: the first problem and the "Also see ..." that follows it both have to
                // be readable, and the footer is 56 points tall.
                Text(text)
                    .font(.ui(12))
                    .foregroundStyle(Tone.ink.opacity(0.85))
                    .lineLimit(2)
                    .truncationMode(.tail)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .layoutPriority(-1)
            .background(FrameMarker(name: "connection-footer-problem"))
            .help(text)
        }
    }

    private func line(_ text: String, tint: Color, truncation: Text.TruncationMode) -> some View {
        HStack(spacing: 7) {
            Circle().fill(tint).frame(width: 7, height: 7).layoutPriority(1)
            Text(text)
                .font(.ui(12))
                .foregroundStyle(tint == Tone.coral ? Tone.coral : Tone.ink.opacity(0.85))
                .lineLimit(1)
                .truncationMode(truncation)
        }
        // Negative priority: the status yields its width before the button beside it does. A
        // greedy frame here squeezed "Test Connection" down to "Test Conn…".
        .layoutPriority(-1)
        .background(FrameMarker(name: "connection-footer-result"))
    }

    /// Drops the engine's `TypeName: ` prefix. It is useful in a log and noise beside a button.
    static func shortDiagnosis(_ message: String) -> String {
        guard let separator = message.range(of: ": ") else { return message }
        return String(message[separator.upperBound...])
    }
}

// MARK: Panes

extension ConnectionEditorSheet {
    /// The pane the sidebar has selected. Only this one is mounted, so everything typed lives in
    /// the draft and the sheet's state, never in a pane.
    @ViewBuilder var pane: some View {
        switch section {
        case .general: generalPane
        case .sshTunnel: sshPane
        case .limits: limitsPane
        case .appearance: appearancePane
        }
    }

    // MARK: General

    /// General, in the order a person fills it in: where the server is, who is asking, then what is
    /// optional. What the form requires is in the first two cards, which fit the sheet without
    /// scrolling for every driver, so a missing required field is never hidden below the fold.
    @ViewBuilder var generalPane: some View {
        VStack(alignment: .leading, spacing: 18) {
            // What a pasted URL said and this form could not use, behind the "?" on the card it
            // concerns.
            ConnectionCard(title: "Server", detail: urlNote) { serverRows }
            ConnectionCard(title: "Authentication") { authenticationRows }
            // MySQL has no schema level and no CA setting, so it has nothing to put here.
            if draft.kind != .mysql { ConnectionCard(title: "More") { moreRows } }
        }
    }

    @ViewBuilder private var serverRows: some View {
        ConnectionFormRow(label: "Name") {
            TextField("\(draft.kind.label) production", text: $draft.name)
                .field(invalid: attemptedSave && missingRequired.contains("name"))
                .focused($focus, equals: .name)
                .formControl("Name", required: true)
            requiredHint("name")
        }
        ConnectionRowDivider()
        // The encryption control sits beside the host it is about. Trino's four outcomes are four
        // explicit choices (HTTPS verified or not, Prefer, HTTP); PostgreSQL and MySQL keep their
        // own mode words (`ConnectionTLSChoice`).
        ConnectionFormRow(label: "Host") {
            HStack(spacing: 8) {
                TextField("db.internal", text: $draft.host)
                    .field(invalid: attemptedSave && missingRequired.contains("host"))
                    .focused($focus, equals: .host)
                    .background(FrameMarker(name: "connection-host-field"))
                    .formControl("Host", required: true)
                ConnectionEncryptionMenu(current: draft.tls,
                                         options: ConnectionTLSChoice.options(kind: draft.kind, caFile: draft.caFile,
                                                                              dbAuth: draft.dbAuth),
                                         extraHelp: draft.kind == .mysql ? Self.mysqlCAHelp : nil,
                                         onSelect: { draft.setTLS($0) })
            }
            requiredHint("host")
            requiredHint("encryption", "A JWT is only sent over HTTPS: pick an HTTPS choice, or use a password.")
            ForEach(encryptionNotes, id: \.self) { tlsNote($0) }
        }
        ConnectionRowDivider()
        ConnectionFormRow(label: "Port") {
            TextField(String(draft.kind.defaultPort), value: $draft.port, format: .number.grouping(.never))
                .field(invalid: attemptedSave && missingRequired.contains("port"))
                .focused($focus, equals: .port)
                .background(FrameMarker(name: "connection-port-field"))
                .frame(width: 96)
                .formControl("Port", required: true)
            requiredHint("port")
        }
        ConnectionRowDivider()
        // Postgres only, and beside the field it modifies: the database named here is where
        // the tree starts, and "All Databases" puts a level holding every database this user may
        // open in front of it.
        //
        // "All Databases" rather than the menu's "Show All Databases": the row carries a field and
        // a checkbox, and the longer label narrows the field.
        //
        // The checkbox is `fixedSize`, so the flexible field cannot squeeze its label.
        ConnectionFormRow(label: draft.kind.databaseLabel) {
            HStack(spacing: 12) {
                TextField(draft.kind == .trino ? "hive" : "mydb", text: $draft.database)
                    .field(invalid: attemptedSave && missingRequired.contains("database"))
                    .focused($focus, equals: .database)
                    .formControl(draft.kind.databaseLabel, required: draft.kind.requiresDatabase)
                if draft.kind == .postgres {
                    InlineCheckbox(label: "All Databases", isOn: $draft.showAllDatabases)
                        .fixedSize()
                        .layoutPriority(1)
                        .help("List every database on the server in the object tree, not just the one beside it.")
                } else if draft.kind == .mysql {
                    // Same switch as Postgres's "Show System Schemas", opposite default story:
                    // MySQL's system databases are always returned by the server, so this one
                    // starts hidden and the box reveals them.
                    InlineCheckbox(label: "Show All", isOn: $draft.showAllSchemas)
                        .fixedSize()
                        .layoutPriority(1)
                        .help("List information_schema, mysql, performance_schema and sys in the object tree too.")
                }
            }
            requiredHint("database")
        }
    }

    /// What most connections leave alone: the schema it opens in, and a CA bundle of its own.
    @ViewBuilder private var moreRows: some View {
        if draft.kind.hasSchemaLevel {
            // Beside the schema it modifies, not in a section of its own: it is a property of
            // that field, and reading it as anything else was the previous layout's mistake.
            // Postgres only — it is the one driver that hides anything.
            ConnectionFormRow(label: "Schema") {
                HStack(spacing: 12) {
                    TextField("public", text: $draft.schema)
                        .field()
                        .formControl("Schema")
                    if draft.kind == .postgres {
                        InlineCheckbox(label: "Show System Schemas", isOn: $draft.showAllSchemas)
                            .fixedSize()
                            .layoutPriority(1)
                            .help("List pg_catalog, information_schema and pg_* schemas in the object tree too.")
                    }
                }
            }
        }
        if draft.kind.hasSchemaLevel { ConnectionRowDivider() }
        caRow
    }

    /// What the encryption control cannot say in its own label: how the choice behaves where that is
    /// not obvious, and why the menu is short while a CA file is set.
    private var encryptionNotes: [String] {
        var notes: [String] = []
        switch draft.tls {
        case .trinoPrefer, .trinoHTTP, .sslmodeUnverified: notes.append(draft.tls.help)
        default: break
        }
        if draft.kind != .mysql, draft.hasCA {
            notes.append("Only modes that check the certificate are offered while a CA file is set.")
        }
        return notes
    }

    /// PostgreSQL and Trino: a CA bundle trusted instead of the platform store. MySQL has no row: the
    /// fact that it has no CA setting is the encryption control's help (`mysqlCAHelp`).
    @ViewBuilder private var caRow: some View {
        if draft.kind != .mysql {
            ConnectionFormRow(label: "CA certificate", hint: Self.caHint) {
                HStack(spacing: 8) {
                    TextField("Use the system trust store", text: caFileBinding)
                        .field(invalid: attemptedSave && missingRequired.contains("caFile"))
                        .focused($focus, equals: .caFile)
                        .formControl("CA certificate", hint: Self.caHint)
                    PillButton(title: "Choose…", compact: true) { chooseFile(into: caFileBinding, title: "Choose the CA certificate") }
                    PillButton(title: "Clear", role: .quiet, compact: true) { draft.setCAFile("") }
                        .disabled(draft.caFile.isEmpty)
                }
                requiredHint("caFile", "File not found")
            }
        }
    }

    private static let mysqlCAHelp = "MySQL can't check a certificate against a CA file of your own in this build, "
        + "so there is no CA setting for it."
    private static let caHint = "The server certificate is checked against this file only, and its name must match. "
        + "For an IP address the certificate needs an IP SAN."
    private static let jwtHint = "JWT is sent only over HTTPS. The token is stored in your Keychain, and a blank field keeps the one saved."
    private static let fingerprintHint = "The first time you connect, QueryHive shows the server's fingerprint and asks you to verify it."
    private static let agentHint = "Uses the keys your ssh-agent offers. Nothing is stored."
    private static let timeoutHint = "Blank follows Settings. 0 means no bound. A statement that overruns is cancelled "
        + "on the server. The MCP server never runs unbounded: it uses this, or 60 s."
    private static let safeModeHint = "Refused by the engine, not by this window: a `read_only` connection refuses a write "
        + "even when the statement comes from the command line or an MCP client."
    private static let environmentHint = "A label only. It does not change what Safe Mode allows."

    @ViewBuilder private var authenticationRows: some View {
        ConnectionFormRow(label: "User") {
            TextField("faisal", text: $draft.user)
                .field(invalid: attemptedSave && missingRequired.contains("user"))
                .focused($focus, equals: .user)
                .formControl("User", required: true)
            requiredHint("user")
        }
        if draft.kind == .trino {
            ConnectionRowDivider()
            // A password, or a bearer token sent over HTTPS.
            ConnectionFormRow(label: "Method") {
                Segmented(selection: $draft.dbAuth, options: DatabaseAuth.allCases) { $0.title }
                    .formControl("Authentication method")
            }
        }
        ConnectionRowDivider()
        if draft.kind == .trino, draft.dbAuth == .jwt {
            ConnectionFormRow(label: "Token", hint: Self.jwtHint) {
                SecureField(editingID == nil ? "Paste the token" : "Unchanged", text: $draft.secrets.jwt)
                    .field()
                    .formControl("Token", hint: Self.jwtHint)
                removeSavedRow(.jwt)
            }
        } else {
            ConnectionFormRow(label: "Password", hint: passwordHint) {
                SecureField(editingID == nil ? "Stored in your Keychain" : "Unchanged", text: $draft.secrets.password)
                    .field()
                    .formControl("Password", hint: passwordHint)
                removeSavedRow(.database)
            }
        }
    }

    // MARK: SSH Tunnel

    @ViewBuilder var sshPane: some View {
        VStack(alignment: .leading, spacing: 18) {
            ConnectionCard(title: "SSH tunnel") {
                ConnectionFormRow(label: "Tunnel", hint: Self.fingerprintHint) {
                    InlineCheckbox(label: "Connect through an SSH tunnel", isOn: $draft.sshEnabled)
                        .frame(minHeight: 32, alignment: .leading)
                        .accessibilityHint(Self.fingerprintHint)
                }
                if draft.sshEnabled { tunnelRows }
            }
        }
    }

    @ViewBuilder private var tunnelRows: some View {
        ConnectionRowDivider()
        ConnectionFormRow(label: "Host") {
            HStack(spacing: 8) {
                TextField("bastion.corp, or an alias from ~/.ssh/config",
                          text: Binding(get: { draft.sshHost },
                                        // Typing a host of your own is not picking an alias.
                                        set: { draft.sshHost = $0; draft.sshUseConfig = false; refreshAliasPreview() }))
                    .field(invalid: attemptedSave && missingRequired.contains("sshHost"))
                    .focused($focus, equals: .sshHost)
                    .formControl("SSH host", required: true)
                aliasMenu
            }
            requiredHint("sshHost")
            if draft.sshUseConfig, let aliasPreview {
                switch aliasPreview {
                case .success(let info): tlsNote(info.summary)
                case .failure(let failure):
                    Text(failure.message).font(.ui(11)).foregroundStyle(Tone.coral)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        ConnectionRowDivider()
        ConnectionFormRow(label: "User") {
            TextField(resolvedAlias?.user ?? "deploy", text: $draft.sshUser)
                .field(invalid: attemptedSave && missingRequired.contains("sshUser"))
                .focused($focus, equals: .sshUser)
                .formControl("SSH user", required: true)
            requiredHint("sshUser")
        }
        ConnectionRowDivider()
        ConnectionFormRow(label: "Port") {
            TextField("22", value: Binding(get: { draft.sshPort == 0 ? nil : draft.sshPort }, set: { draft.sshPort = $0 ?? 0 }),
                      format: .number.grouping(.never))
                .field(invalid: attemptedSave && missingRequired.contains("sshPort"))
                .focused($focus, equals: .sshPort)
                .background(FrameMarker(name: "connection-ssh-port-field"))
                .frame(width: 96)
                .formControl("SSH port")
            requiredHint("sshPort")
        }
        ConnectionRowDivider()
        // With the agent chosen, what that means is the row's hint; with another method the row has
        // nothing to explain and the next rows are the answer.
        ConnectionFormRow(label: "Authentication", hint: draft.sshAuth == .agent ? Self.agentHint : nil) {
            Segmented(selection: $draft.sshAuth, options: SSHAuthMethod.allCases) { $0.title }
                .formControl("SSH authentication", hint: draft.sshAuth == .agent ? Self.agentHint : nil)
        }
        switch draft.sshAuth {
        case .agent:
            EmptyView()
        case .key:
            ConnectionRowDivider()
            ConnectionFormRow(label: "Key file") {
                HStack(spacing: 8) {
                    TextField(resolvedAlias?.identityFiles.first ?? "~/.ssh/id_ed25519", text: $draft.sshKeyPath)
                        .field(invalid: attemptedSave && missingRequired.contains("sshKeyFile"))
                        .focused($focus, equals: .sshKeyFile)
                        .formControl("SSH key file", required: true)
                    PillButton(title: "Choose…", compact: true) { chooseFile(into: $draft.sshKeyPath, title: "Choose the SSH private key") }
                }
                requiredHint("sshKeyFile")
            }
            ConnectionRowDivider()
            ConnectionFormRow(label: "Passphrase") {
                SecureField("Unchanged", text: $draft.secrets.sshPassphrase)
                    .field()
                    .formControl("SSH key passphrase")
                removeSavedRow(.sshPassphrase)
            }
        case .password:
            ConnectionRowDivider()
            ConnectionFormRow(label: "Password") {
                SecureField(editingID == nil ? "Stored in your Keychain" : "Unchanged", text: $draft.secrets.sshPassword)
                    .field()
                    .formControl("SSH password")
                removeSavedRow(.sshPassword)
            }
        }
    }

    // MARK: Limits

    @ViewBuilder var limitsPane: some View {
        VStack(alignment: .leading, spacing: 18) {
            // About what may run rather than how to reach the server. The engine enforces it and
            // this picker only records the choice: the command line and the MCP server go through
            // the same guard, and neither has this control.
            ConnectionCard(title: "Safe Mode") {
                ConnectionFormRow(label: "Level", hint: Self.safeModeHint) {
                    Segmented(selection: $draft.safeMode, options: ConnectionSafeMode.allCases) { $0.title }
                        .formControl("Safe Mode", hint: Self.safeModeHint)
                    // What the chosen level does follows the choice, and it is what the person is
                    // promising themselves: it stays in view.
                    tlsNote(draft.safeMode.detail)
                }
            }
            ConnectionCard(title: "Statement timeout") {
                ConnectionFormRow(label: "Seconds", hint: Self.timeoutHint) {
                    TextField("Follow the app setting (\(model.statementTimeoutMS / 1000) s)", text: $draft.timeoutText)
                        .field(invalid: attemptedSave && missingRequired.contains("timeout"))
                        .focused($focus, equals: .timeout)
                        .formControl("Statement timeout in seconds", hint: Self.timeoutHint)
                }
            }
        }
    }

    // MARK: Appearance

    @ViewBuilder var appearancePane: some View {
        VStack(alignment: .leading, spacing: 18) {
            ConnectionCard {
                ConnectionFormRow(label: "Color") {
                    ColorSwatchPicker(selection: $draft.color)
                        .frame(minHeight: 32, alignment: .leading)
                }
                ConnectionRowDivider()
                ConnectionFormRow(label: "Environment", hint: Self.environmentHint) {
                    Segmented(selection: $draft.environment,
                              options: [ConnectionEnvironment?.none] + ConnectionEnvironment.allCases.map { Optional($0) }) {
                        $0?.title ?? "None"
                    }
                    .formControl("Environment", hint: Self.environmentHint)
                }
            }
        }
    }

    // MARK: Helpers

    /// The `~/.ssh/config` aliases, when there are any. Choosing one fills the host and turns on
    /// alias mode; the engine then reads the host name, user, port and key from the file.
    @ViewBuilder private var aliasMenu: some View {
        let aliases = SSHConfigAliases.current.hosts()
        if !aliases.isEmpty {
            Menu {
                ForEach(aliases, id: \.self) { alias in
                    Button(alias) { draft.sshHost = alias; draft.sshUseConfig = true; refreshAliasPreview() }
                }
            } label: {
                Text("Aliases")
            }
            .menuStyle(.button)
            .fixedSize()
        }
    }

    func refreshAliasPreview() {
        aliasPreview = draft.sshUseConfig && !draft.sshHost.isEmpty ? SSHConfigAliases.current.resolve(draft.sshHost) : nil
    }

    /// "Remove saved…" for one secret, shown only when the Keychain has one and the person has not
    /// already staged its removal. Nothing is deleted until Save, and an empty field alone never
    /// deletes anything: the form cannot show a saved secret, so blank can only mean "keep".
    @ViewBuilder private func removeSavedRow(_ slot: ConnectionKeychain.Slot) -> some View {
        if let id = editingID, AppModel.secretStore.contains(slot: slot, for: id) {
            if draft.secrets.removals.contains(slot) {
                HStack(spacing: 8) {
                    Text("The saved \(AppModel.secretName(slot)) is removed when you save.")
                        .font(.ui(11)).foregroundStyle(Tone.amber)
                    PillButton(title: "Undo", role: .quiet, compact: true) { draft.secrets.removals.remove(slot) }
                }
            } else {
                PillButton(title: "Remove saved…", role: .destructive, compact: true) { draft.secrets.removals.insert(slot) }
                    .help("Takes the saved \(AppModel.secretName(slot)) out of the Keychain when you save")
            }
        }
    }

    private func chooseFile(into binding: Binding<String>, title: String) {
        let panel = NSOpenPanel()
        panel.title = title
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        if panel.runModal() == .OK, let url = panel.url { binding.wrappedValue = url.path }
    }

    /// The CA field binds through the draft, so the rule that a CA needs a verifying mode holds
    /// however the path changes (typing, Choose…, Clear).
    private var caFileBinding: Binding<String> {
        Binding(get: { draft.caFile }, set: { draft.setCAFile($0) })
    }

    /// One spoken line where a control would be, for a transport with no verification to answer.
    func tlsNote(_ text: String) -> some View {
        Text(text)
            .font(.ui(11))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var passwordHint: String {
        switch draft.kind {
        case .trino: "Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https. Prefer is the exception: it starts on HTTPS and falls back to clear only when the coordinator has no TLS."
        case .postgres: "Postgres accepts a password over any SSL mode, including \"prefer\"."
        case .mysql: "MySQL accepts a password over either SSL mode."
        }
    }

    @ViewBuilder private func requiredHint(_ key: String, _ word: String = "Required") -> some View {
        if attemptedSave && missingRequired.contains(key) {
            Text(word).font(.ui(11)).foregroundStyle(Tone.coral)
        }
    }
}

extension ConnectionKind {
    /// What a Test counts when it succeeds, named as this driver's top level is: Trino lists
    /// catalogs, PostgreSQL the schemas of its database, MySQL databases.
    var contextNoun: String {
        switch self {
        case .trino: "catalog"
        case .postgres: "schema"
        case .mysql: "database"
        }
    }
}

/// Hands over the window its view ends up in, as soon as it has one.
struct ConnectionSheetWindow: NSViewRepresentable {
    let onWindow: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = Probe()
        view.onWindow = onWindow
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}

    final class Probe: NSView {
        var onWindow: ((NSWindow) -> Void)?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let window { onWindow?(window) }
        }
    }
}
