import SwiftUI

/// 22pt-default rounded tile filled with a connection's colour gradient, carrying the
/// "this is a Trino coordinator" glyph. Shared by the tree, the toolbar picker and the title
/// strip so a connection reads the same way everywhere it appears.
func connectionTile(_ connection: Connection, size: CGFloat = 22) -> some View {
    RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
        // The tile's own subtle ramp, flattened under a flat tone like every other gradient.
        .fill(ThemeStore.shared.tone.isLuminous
              ? LinearGradient(colors: [connection.color.color, connection.color.color.opacity(0.55)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
              : LinearGradient(colors: [connection.color.color, connection.color.color],
                               startPoint: .topLeading, endPoint: .bottomTrailing))
        .frame(width: size, height: size)
        .overlay {
            if let logo = DriverLogo.image(for: connection.kind) {
                Image(nsImage: logo).resizable().scaledToFit()
                    .frame(width: size * 0.68, height: size * 0.68)
            } else {
                Image(systemName: connection.kind.symbol)
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
                Button("\(connection.name) — \(connection.displayTarget)") { selection = connection.id }
            }
            Divider()
            Button("Edit Connections…") { model.presentConnectionEditor(selection) }
            Button("New Connection…") { model.presentConnectionEditor(nil) }
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
private enum TestState {
    case idle, running
    case success(Int)
    case failure(String)
}

/// The connection editor, as Navicat presents it: a modal connection-properties dialog with
/// Test, Save and Delete. Nothing behind it changes until Save.
struct ConnectionEditorSheet: View {
    let target: ConnectionEditorTarget
    @Environment(AppModel.self) private var model

    private var connectionID: UUID? { target.connectionID }
    @Environment(\.dismiss) private var dismiss

    /// Which of the sheet's three states is showing. A saved connection starts at `.form`.
    private enum Step { case typePicker, url, form }

    @State private var step = Step.typePicker
    @State private var urlText = ""
    @State private var urlError: String?
    @State private var editingID: UUID?
    @State private var name = ""
    @State private var color = ConnectionColor.blue
    @State private var kind = ConnectionKind.trino
    @State private var host = ""
    @State private var port = ConnectionKind.trino.defaultPort
    @State private var scheme = "https"
    @State private var sslmode = ConnectionKind.postgres.defaultSSLMode
    @State private var user = ""
    @State private var credential = ""
    @State private var database = ""
    @State private var schema = ""
    @State private var verifyTLS = true
    /// Postgres only: list system schemas too. Meaningful nowhere else, so the field is shown
    /// only for that driver, mirroring the tree's own context-menu toggle.
    @State private var showAllSchemas = false
    @State private var confirmDelete = false
    @State private var testState = TestState.idle
    @State private var testProcess: (any EngineRun)?
    @State private var testRun = UUID()
    @State private var attemptedSave = false
    /// The id a brand-new connection will save under, reused across retries so a Save that fails
    /// (a JSON write error) doesn't mint a fresh UUID next try and orphan the Keychain item the
    /// first attempt already wrote.
    @State private var draftID = UUID()

    private var original: Connection? { model.connections.first { $0.id == editingID } }

    private var isDirty: Bool {
        guard let original else {
            return !(name.isEmpty && host.isEmpty && user.isEmpty && credential.isEmpty
                     && database.isEmpty && schema.isEmpty)
        }
        return !credential.isEmpty || name != original.name || color != original.color
            || kind != original.kind || host != original.host || port != original.port
            || scheme != original.scheme || sslmode != original.sslmode
            || user != original.user || database != original.database || schema != original.schema
            || verifyTLS != original.verify
    }

    /// Name, host and user are what the engine cannot invent, and Postgres cannot open a
    /// connection without a database at all. Port and encryption always carry a value; a blank
    /// password simply means "connect without one".
    private var missingRequired: Set<String> {
        var missing = Set<String>()
        if name.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("name") }
        if host.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("host") }
        if user.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("user") }
        if !(1...65535).contains(port) { missing.insert("port") }
        if kind.requiresDatabase, database.trimmingCharacters(in: .whitespaces).isEmpty {
            missing.insert("database")
        }
        return missing
    }

    var body: some View {
        VStack(spacing: 0) {
            switch step {
            case .typePicker: typePicker
            case .url: urlStep
            case .form: formStep
            }
        }
        .frame(width: 620, height: 660)
        .background(Tone.canvas)
        // No `.preferredColorScheme(.dark)` here. It used to pin the sheet dark, and that is what
        // made the editor unreadable in light mode: `Tone.canvas` follows the *stored mode* (light
        // mode picks the light theme) while `Tone.ink` is a dynamic `NSColor` that follows the
        // *effective appearance*. Pinning dark made the sheet's ink resolve white over a canvas the
        // theme had already drawn light, so every label came out white on white. The sheet now
        // inherits the appearance of the window that presents it, which is what every other surface
        // in the app does.
        .confirmationDialog("Delete \(name.isEmpty ? "this connection" : name)?", isPresented: $confirmDelete) {
            Button("Delete", role: .destructive) { delete() }
        } message: {
            Text("This removes the saved connection and its Keychain password. This can't be undone.")
        }
        .onAppear { load() }
        .onDisappear { testProcess?.terminate() }
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
                // This sheet closes first. `presentNavicatImport` runs a modal `NSOpenPanel`, and
                // starting one from inside a sheet being dismissed puts the panel behind the sheet
                // on its way out — the wait is the sheet's own dismissal animation, not a guess.
                PillButton(title: "Import from Navicat…", symbol: "square.and.arrow.down") {
                    dismiss()
                    Task {
                        try? await Task.sleep(for: .milliseconds(350))
                        model.presentNavicatImport()
                    }
                }
                Spacer()
                PillButton(title: "Cancel", role: .quiet) { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 20)
            .frame(height: 62)
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
        VStack(spacing: 0) {
            header
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            ScrollView {
                form.padding(20)
            }
            Rectangle().fill(Tone.ink.opacity(0.08)).frame(height: 1)
            footer
        }
    }

    /// Picking a tile is the only place the driver is chosen; the form then has no type control,
    /// because changing it would invalidate half of what is already filled in.
    private func choose(_ option: ConnectionKind) {
        if port == kind.defaultPort { port = option.defaultPort }
        kind = option
        if option.hasSSLModes, sslmode.isEmpty { sslmode = option.defaultSSLMode }
        step = .form
    }

    private func applyURL() {
        guard let parsed = ConnectionURL.parse(urlText) else {
            urlError = "Couldn't read that. Expected something like postgresql://user:password@host:5432/mydb"
            return
        }
        urlError = nil
        kind = parsed.kind
        host = parsed.host
        port = parsed.port
        scheme = parsed.scheme.isEmpty ? "https" : parsed.scheme
        sslmode = parsed.kind.defaultSSLMode
        user = parsed.user
        credential = parsed.password ?? ""
        database = parsed.database
        schema = parsed.schema
        if name.isEmpty {
            name = parsed.host + (parsed.database.isEmpty ? "" : "/\(parsed.database)")
        }
        step = .form
    }

    private var header: some View {
        HStack(spacing: 14) {
            ConnectionBrandTile(kind: kind, size: 56)
            VStack(alignment: .leading, spacing: 2) {
                Text(editingID == nil ? "New Connection" : (name.isEmpty ? "Connection" : name))
                    .font(.ui(18, weight: .bold, rounded: true))
                    .lineLimit(1)
                Text(editingID == nil ? "Not saved yet." : "Saved \(kind.label) connection.")
                    .font(.ui(12))
                    .foregroundStyle(Tone.secondary)
            }
            Spacer(minLength: 12)
            if isDirty {
                Label("Unsaved changes", systemImage: "exclamationmark.circle")
                    .font(.ui(11, weight: .medium))
                    .foregroundStyle(Tone.amber)
                    .labelStyle(.titleAndIcon)
            }
            PillButton(title: "Change Type…", symbol: "arrow.triangle.2.circlepath", compact: true) { step = .typePicker }
                .help("Pick a different driver — this clears the fields that only applied to \(kind.label)")
        }
        .padding(.horizontal, 20)
        .frame(height: 84)
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 14) {
            LabeledField("Name · Required") {
                TextField("\(kind.label) production", text: $name)
                    .field(invalid: attemptedSave && missingRequired.contains("name"))
                requiredHint("name")
            }
            LabeledField("Color") { ColorSwatchPicker(selection: $color) }
            LabeledField("Host · Required") {
                TextField("db.internal", text: $host).field(invalid: attemptedSave && missingRequired.contains("host"))
                requiredHint("host")
            }
            HStack(alignment: .top, spacing: 12) {
                if kind == .trino {
                    // Three options, not two, because Trino's encryption decision has four
                    // outcomes and the two scheme words can only spell three of them. The
                    // fourth — "https when the coordinator offers it, http otherwise" — is
                    // `prefer`, and it is the shared vocabulary's own word rather than a new
                    // one: the other two drivers' SSL mode pickers already show it for this
                    // exact outcome. See `TrinoTransport` for how it reaches the engine.
                    LabeledField("Transport") {
                        Segmented(selection: trinoTransport, options: TrinoTransport.allCases) { $0.label }
                    }
                    .frame(width: 235)
                    .help("HTTPS encrypts and checks the certificate. HTTP is clear. Prefer tries HTTPS "
                          + "first and keeps plain HTTP only for a coordinator that has no TLS at all — "
                          + "the certificate is not checked either way.")
                }
                LabeledField("Port · Required") {
                    TextField(String(kind.defaultPort), value: $port, format: .number.grouping(.never))
                        .field(invalid: attemptedSave && missingRequired.contains("port"))
                    requiredHint("port")
                }
            }
            .onChange(of: scheme) { old, new in
                // Follow the standard port for the scheme, but only while the user is still on
                // the other scheme's standard one.
                if old == "http", new == "https", port == 8080 { port = 8443 }
                if old == "https", new == "http", port == 8443 { port = 8080 }
                // `prefer` moves nothing. It tries HTTPS and falls back to clear on whatever
                // port it was given, which is the only port that can answer the question the
                // transport exists to leave open; rewriting the port would answer it for the
                // coordinator instead of asking it.
            }
            LabeledField("User · Required") {
                TextField("faisal", text: $user).field(invalid: attemptedSave && missingRequired.contains("user"))
                requiredHint("user")
            }
            LabeledField(editingID == nil ? "Password" : "Password · leave blank to keep") {
                SecureField(editingID == nil ? "Stored in your Keychain" : "Unchanged", text: $credential)
                    .field()
                Text(passwordHint)
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(alignment: .bottom, spacing: 12) {
                LabeledField(kind.databaseLabel + (kind.requiresDatabase ? " · Required" : "")) {
                    TextField(kind == .trino ? "hive" : "mydb", text: $database)
                        .field(invalid: attemptedSave && missingRequired.contains("database"))
                    requiredHint("database")
                }
                if kind.hasSchemaLevel {
                    LabeledField("Schema") { TextField("public", text: $schema).field() }
                }
                // Beside the schema it modifies, not in a section of its own: it is a property of
                // that field, and reading it as anything else was the previous layout's mistake.
                // Postgres only — it is the one driver that hides anything.
                if kind == .postgres {
                    InlineCheckbox(label: "Show All", isOn: $showAllSchemas)
                        .padding(.bottom, 5)
                        .help("List pg_catalog, information_schema and pg_* schemas in the object tree too.")
                } else if kind == .mysql {
                    // Same switch, opposite default story: MySQL's system databases are always
                    // returned by the server, so this one starts hidden and the box reveals them.
                    //
                    // `fixedSize` and a layout priority, because MySQL has no schema field and a
                    // lone Database field expands to the full row width -- which pushed the
                    // checkbox off the edge entirely rather than sharing the row with it.
                    InlineCheckbox(label: "Show All", isOn: $showAllSchemas)
                        .fixedSize()
                        .layoutPriority(1)
                        .padding(.bottom, 5)
                        .help("List information_schema, mysql, performance_schema and sys in the object tree too.")
                }
            }
            if kind.hasSSLModes {
                LabeledField("SSL mode") {
                    Segmented(selection: $sslmode, options: kind.sslModes) { $0 }
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
                switch TrinoTransport(stored: scheme) {
                case .https:
                    ChipToggle(label: "Verify the TLS certificate", isOn: $verifyTLS)
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

    /// The picker's own binding over the string the connection stores, so the third option
    /// needs no second field: `TrinoTransport` already reads and writes the scheme slot.
    private var trinoTransport: Binding<TrinoTransport> {
        Binding(get: { TrinoTransport(stored: scheme) },
                set: { scheme = $0.rawValue })
    }

    /// One spoken line where a control would be, for a transport with no verification to answer.
    private func tlsNote(_ text: String) -> some View {
        Text(text)
            .font(.ui(11))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var passwordHint: String {
        switch kind {
        case .trino: "Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https. Prefer is the exception: it starts on HTTPS and falls back to clear only when the coordinator has no TLS."
        case .postgres: "Postgres accepts a password over any SSL mode, including \"prefer\"."
        case .mysql: "MySQL accepts a password over either SSL mode."
        }
    }

    @ViewBuilder private func requiredHint(_ key: String) -> some View {
        if attemptedSave && missingRequired.contains(key) {
            Text("Required").font(.ui(11)).foregroundStyle(Tone.coral)
        }
    }

    /// The last test's outcome, on one line next to the button. A failure's message can be long
    /// — the engine prefixes it with the exception class and Trino's connection errors run to a
    /// paragraph — so the class prefix is dropped for reading and the whole thing stays in the
    /// tooltip.
    @ViewBuilder private var testStatus: some View {
        switch testState {
        case .idle, .running:
            // Nothing to add: the button beside this says "Testing…" while it works.
            EmptyView()
        case .success(let level):
            statusLine("Connected · \(pluralized(level, kind == .postgres ? "schema" : "catalog"))",
                       tint: Tone.mint, truncation: .tail)
        case .failure(let message):
            // Middle, not tail: a connection error's cause is at the end of the sentence
            // ("… Connection refused"), and its class prefix is at the start.
            statusLine(Self.shortDiagnosis(message), tint: Tone.coral, truncation: .middle)
                .help(message)
        }
    }

    private func statusLine(_ text: String, tint: Color,
                            truncation: Text.TruncationMode) -> some View {
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
    private static func shortDiagnosis(_ message: String) -> String {
        guard let separator = message.range(of: ": ") else { return message }
        return String(message[separator.upperBound...])
    }

    /// Left has the action and its outcome; right has the terminal choices, in the order a
    /// dialog's footer always puts them.
    private var footer: some View {
        HStack(spacing: 10) {
            // The running state keeps the capsule's shape so the footer does not reflow the
            // moment a test starts — it is the same button, mid-press.
            if case .running = testState {
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
                PillButton(title: "Test Connection", symbol: "bolt") { runTest() }
                    .keyboardShortcut("t", modifiers: .command)
                    .help("Test Connection (⌘T)")
            }
            testStatus

            Spacer()

            if editingID != nil {
                PillButton(title: "Delete", symbol: "trash", role: .destructive) { confirmDelete = true }
            }
            PillButton(title: "Cancel", role: .quiet) { dismiss() }
                .keyboardShortcut(.cancelAction)
            HubButton(title: "Save", symbol: "checkmark", hue: .connection) { save() }
                .disabled(!isDirty)
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .frame(height: 62)
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
        credential = ""
        guard let connection = original else {
            draftID = UUID()
            name = ""; color = .blue; kind = .trino; host = ""
            port = ConnectionKind.trino.defaultPort; scheme = "https"
            sslmode = ConnectionKind.postgres.defaultSSLMode
            user = ""; database = ""; schema = ""; verifyTLS = true
            showAllSchemas = false
            return
        }
        name = connection.name
        color = connection.color
        kind = connection.kind
        host = connection.host
        port = connection.port
        scheme = connection.scheme
        sslmode = connection.sslmode.isEmpty ? connection.kind.defaultSSLMode : connection.sslmode
        user = connection.user
        database = connection.database
        schema = connection.schema
        verifyTLS = connection.verify
        showAllSchemas = connection.showAllSchemas
    }

    private func save() {
        attemptedSave = true
        guard missingRequired.isEmpty else { return }
        let id = editingID ?? draftID
        let connection = Connection(id: id,
                                     name: name.trimmingCharacters(in: .whitespaces),
                                     color: color,
                                     kind: kind,
                                     host: host.trimmingCharacters(in: .whitespaces),
                                     port: port,
                                     scheme: scheme,
                                     sslmode: sslmode,
                                     user: user.trimmingCharacters(in: .whitespaces),
                                     database: database.trimmingCharacters(in: .whitespaces),
                                     schema: schema.trimmingCharacters(in: .whitespaces),
                                     verify: verifyTLS,
                                     showAllSchemas: showAllSchemas)
        // Keychain first: if it throws, the JSON never claims a password exists that isn't there.
        do {
            if !credential.isEmpty {
                try ConnectionKeychain.set(credential, for: id)
            }
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
        testProcess?.terminate()
        testState = .running
        let run = UUID()
        testRun = run
        let storedPassword: String?
        do {
            storedPassword = try editingID.flatMap { try ConnectionKeychain.get(for: $0) }
        } catch {
            testState = .failure("Couldn't read the password for \(name.isEmpty ? "this connection" : name) from Keychain: \(error.localizedDescription)")
            return
        }
        let env = AppModel.connectionEnvironment(
            kind: kind,
            host: host.trimmingCharacters(in: .whitespaces),
            port: port,
            user: user.trimmingCharacters(in: .whitespaces),
            password: credential.isEmpty ? storedPassword : credential,
            database: database.trimmingCharacters(in: .whitespaces),
            schema: schema.trimmingCharacters(in: .whitespaces),
            scheme: scheme,
            sslmode: sslmode,
            verify: verifyTLS
        )
        var catalogs = 0
        var message: String?
        testProcess = Engine.current.run("test", env: env, onEvent: { event in
            guard testRun == run else { return }
            if event.event == "test" { catalogs = event.catalogCount ?? 0 }
            if event.event == "error" { message = event.message }
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
                    .font(.code(10.5))
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
