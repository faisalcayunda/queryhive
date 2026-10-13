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
                    if let detail { HelpHint(text: detail) }
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
    @ViewBuilder let control: Control

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text(label)
                .font(.ui(13))
                .foregroundStyle(Tone.ink)
                .lineLimit(1)
                .frame(width: Self.labelWidth, height: 32, alignment: .leading)
                .help(label)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 5) { control }
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 5)
        .frame(minHeight: 36, alignment: .top)
    }
}

extension View {
    /// What VoiceOver calls a row's control, and whether it must be filled in. The row's visible
    /// label is hidden from VoiceOver so it is not read twice.
    func formControl(_ label: String, required: Bool = false) -> some View {
        accessibilityLabel(label).accessibilityHint(required ? "Required" : "")
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
                    Text(isSaved ? "Saved \(kind.label) connection." : "Not saved yet.")
                        .font(.ui(11)).foregroundStyle(Tone.secondary).lineLimit(2)
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

    private func item(_ section: ConnectionFormSection) -> some View {
        let active = selection == section
        let count = issueCounts[section] ?? 0
        return Button { selection = section } label: {
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
        .accessibilityValue(count > 0 ? "\(count) \(count == 1 ? "field needs" : "fields need") attention" : "")
        .accessibilityAddTraits(active ? [.isSelected] : [])
    }
}

/// The title of the connection, and the mark that it has changes nobody saved.
struct ConnectionFormHeader: View {
    let title: String
    let isDirty: Bool

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
            if !isTesting { statusLine }

            Spacer()

            PillButton(title: "Cancel", role: .quiet, action: onCancel)
                .keyboardShortcut(.cancelAction)
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
                Text(text)
                    .font(.ui(12))
                    .foregroundStyle(Tone.ink.opacity(0.85))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .layoutPriority(-1)
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

    @ViewBuilder var generalPane: some View {
        VStack(alignment: .leading, spacing: 18) {
            if let urlNote { tlsNote(urlNote) }
            ConnectionCard {
                ConnectionFormRow(label: "Name") {
                    TextField("\(draft.kind.label) production", text: $draft.name)
                        .field(invalid: attemptedSave && missingRequired.contains("name"))
                        .focused($focus, equals: .name)
                        .formControl("Name", required: true)
                    requiredHint("name")
                }
                ConnectionRowDivider()
                ConnectionFormRow(label: "Type") {
                    HStack(spacing: 10) {
                        ConnectionBrandTile(kind: draft.kind, size: 18)
                        Text(draft.kind.label).font(.ui(13))
                        Spacer(minLength: 8)
                        PillButton(title: "Change Type…", symbol: "arrow.triangle.2.circlepath", compact: true) { step = .typePicker }
                            .help("Pick a different driver — this clears the fields that only applied to \(draft.kind.label)")
                    }
                    .frame(minHeight: 32)
                }
            }
            ConnectionCard(title: "Server") { serverRows }
            ConnectionCard(title: "Authentication") { authenticationRows }
        }
    }

    @ViewBuilder private var serverRows: some View {
        ConnectionFormRow(label: "Host") {
            TextField("db.internal", text: $draft.host)
                .field(invalid: attemptedSave && missingRequired.contains("host"))
                .focused($focus, equals: .host)
                .background(FrameMarker(name: "connection-host-field"))
                .formControl("Host", required: true)
            requiredHint("host")
        }
        if draft.kind == .trino {
            ConnectionRowDivider()
            // Three options, not two, because Trino's encryption decision has four
            // outcomes and the two scheme words can only spell three of them. The
            // fourth — "https when the coordinator offers it, http otherwise" — is
            // `prefer`, and it is the shared vocabulary's own word rather than a new
            // one: the other two drivers' SSL mode pickers already show it for this
            // exact outcome. See `TrinoTransport` for how it reaches the engine.
            ConnectionFormRow(label: "Transport") {
                Segmented(selection: trinoTransport,
                          options: draft.hasCA ? [.https] : TrinoTransport.allCases) { $0.label }
                    .formControl("Transport")
                    .help("HTTPS encrypts and checks the certificate. HTTP is clear. Prefer tries HTTPS "
                          + "first and keeps plain HTTP only for a coordinator that has no TLS at all — "
                          + "the certificate is not checked either way.")
            }
        }
        ConnectionRowDivider()
        ConnectionFormRow(label: "Port") {
            TextField(String(draft.kind.defaultPort), value: $draft.port, format: .number.grouping(.never))
                .field(invalid: attemptedSave && missingRequired.contains("port"))
                .focused($focus, equals: .port)
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
        if draft.kind.hasSchemaLevel {
            ConnectionRowDivider()
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
        ConnectionRowDivider()
        oldEncryptionRow
        ConnectionRowDivider()
        caRow
    }

    /// The step-3 encryption rows, as the form had them: the SSL mode for Postgres and MySQL, and
    /// for Trino a verify box that exists only on HTTPS.
    @ViewBuilder private var oldEncryptionRow: some View {
        if draft.kind.hasSSLModes {
            ConnectionFormRow(label: "SSL mode") {
                // With a CA file chosen only the modes that check the certificate make sense.
                Segmented(selection: Binding(get: { draft.sslmode }, set: { draft.setTLS(.sslmode($0)) }),
                          options: draft.kind == .postgres && draft.hasCA ? ["verify-ca", "verify-full"] : draft.kind.sslModes) { $0 }
                    .formControl("SSL mode")
            }
        } else {
            // HTTPS is the only Trino transport with a verification answer to give, so it is
            // the only one that shows the box. `prefer` never checks the certificate — the
            // mode means "encrypt if you can", both on the attempt and on the fallback —
            // and in clear there is nothing to check, so neither is offered a control that
            // could not change the connection.
            //
            // The absence is *said* rather than left blank: a control that simply vanishes
            // reads as a mistake, and an unchecked box reads as an answer. The stored value
            // stays stored either way, so going back to HTTPS offers the user's last choice
            // again.
            ConnectionFormRow(label: "Certificate") {
                switch TrinoTransport(stored: draft.scheme) {
                case .https:
                    ChipToggle(label: "Verify the TLS certificate", isOn: $draft.verifyTLS)
                        .disabled(draft.hasCA)
                case .prefer:
                    tlsNote("Prefer: HTTPS first, plain HTTP only when the coordinator has no TLS. "
                            + "The certificate is not checked.")
                        .help("`prefer` never verifies: it encrypts when it can and falls back to clear "
                              + "only when the coordinator answers the handshake with something that is not "
                              + "TLS. HTTPS with this box off is the transport that says so outright.")
                case .http:
                    tlsNote("Plain HTTP: nothing to verify. A password still forces HTTPS — "
                            + "pick HTTPS to choose how that is verified.")
                        .help("In clear there is no certificate to check. The engine raises this connection "
                              + "to HTTPS anyway when a password is stored, which is why the verification "
                              + "answer lives on the HTTPS transport rather than here.")
                }
            }
        }
    }

    /// PostgreSQL and Trino: a CA bundle trusted instead of the platform store.
    @ViewBuilder private var caRow: some View {
        if draft.kind == .mysql {
            ConnectionFormRow(label: "CA certificate") {
                tlsNote("MySQL can't check a certificate against a CA file of your own in this build, "
                        + "so there is no CA setting for it.")
            }
        } else {
            ConnectionFormRow(label: "CA certificate") {
                HStack(spacing: 8) {
                    TextField("Use the system trust store", text: caFileBinding)
                        .field(invalid: attemptedSave && missingRequired.contains("caFile"))
                        .focused($focus, equals: .caFile)
                        .formControl("CA certificate")
                    PillButton(title: "Choose…", compact: true) { chooseFile(into: caFileBinding, title: "Choose the CA certificate") }
                    PillButton(title: "Clear", role: .quiet, compact: true) { draft.setCAFile("") }
                        .disabled(draft.caFile.isEmpty)
                }
                requiredHint("caFile", "File not found")
                tlsNote("The server certificate is checked against this file only, and its name must match. "
                        + "For an IP address the certificate needs an IP SAN.")
            }
        }
    }

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
            ConnectionFormRow(label: "Token") {
                SecureField(editingID == nil ? "Paste the token" : "Unchanged", text: $draft.secrets.jwt)
                    .field()
                    .formControl("Token")
                removeSavedRow(.jwt)
                tlsNote("JWT is sent only over HTTPS. The token is stored in your Keychain, and a blank field keeps the one saved.")
            }
        } else {
            ConnectionFormRow(label: "Password") {
                SecureField(editingID == nil ? "Stored in your Keychain" : "Unchanged", text: $draft.secrets.password)
                    .field()
                    .formControl("Password")
                tlsNote(passwordHint)
                removeSavedRow(.database)
            }
        }
    }

    // MARK: SSH Tunnel

    @ViewBuilder var sshPane: some View {
        VStack(alignment: .leading, spacing: 18) {
            ConnectionCard(title: "SSH tunnel") {
                ConnectionFormRow(label: "Tunnel") {
                    InlineCheckbox(label: "Connect through an SSH tunnel", isOn: $draft.sshEnabled)
                        .frame(minHeight: 32, alignment: .leading)
                    tlsNote("The first time you connect, QueryHive shows the server's fingerprint and asks you to verify it.")
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
                .frame(width: 96)
                .formControl("SSH port")
            requiredHint("sshPort")
        }
        ConnectionRowDivider()
        ConnectionFormRow(label: "Authentication") {
            Segmented(selection: $draft.sshAuth, options: SSHAuthMethod.allCases) { $0.title }
                .formControl("SSH authentication")
        }
        switch draft.sshAuth {
        case .agent:
            ConnectionRowDivider()
            ConnectionFormRow(label: "Agent") {
                tlsNote("Uses the keys your ssh-agent offers. Nothing is stored.")
            }
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
                ConnectionFormRow(label: "Level") {
                    Segmented(selection: $draft.safeMode, options: ConnectionSafeMode.allCases) { $0.title }
                        .formControl("Safe Mode")
                    tlsNote(draft.safeMode.detail)
                }
                .help("Refused by the engine, not by this window: a `read_only` connection refuses a write "
                      + "even when the statement comes from the command line or an MCP client.")
            }
            ConnectionCard(title: "Statement timeout") {
                ConnectionFormRow(label: "Seconds") {
                    TextField("Follow the app setting (\(model.statementTimeoutMS / 1000) s)", text: $draft.timeoutText)
                        .field(invalid: attemptedSave && missingRequired.contains("timeout"))
                        .focused($focus, equals: .timeout)
                        .formControl("Statement timeout in seconds")
                    tlsNote("Blank follows Settings. 0 means no bound. A statement that overruns is cancelled "
                            + "on the server. The MCP server never runs unbounded: it uses this, or 60 s.")
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
                ConnectionFormRow(label: "Environment") {
                    Segmented(selection: $draft.environment,
                              options: [ConnectionEnvironment?.none] + ConnectionEnvironment.allCases.map { Optional($0) }) {
                        $0?.title ?? "None"
                    }
                    .formControl("Environment")
                    tlsNote("A label only. It does not change what Safe Mode allows.")
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

    /// The picker's own binding over the string the connection stores, so the third option
    /// needs no second field: `TrinoTransport` already reads and writes the scheme slot.
    private var trinoTransport: Binding<TrinoTransport> {
        Binding(get: { TrinoTransport(stored: draft.scheme) },
                set: { draft.setScheme($0.rawValue) })
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
