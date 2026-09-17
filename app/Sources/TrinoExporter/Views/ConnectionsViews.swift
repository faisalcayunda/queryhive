import SwiftUI

/// 22pt-default rounded tile filled with a connection's colour gradient, carrying the
/// "this is a Trino coordinator" glyph. Shared by the tree, the toolbar picker and the title
/// strip so a connection reads the same way everywhere it appears.
func connectionTile(_ connection: Connection, size: CGFloat = 22) -> some View {
    RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
        .fill(LinearGradient(colors: [connection.color.color, connection.color.color.opacity(0.55)],
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
                    Text(connection.name).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                } else {
                    Text(model.connections.isEmpty ? "No connections" : "Choose a connection…")
                        .font(.system(size: 12))
                        .foregroundStyle(Tone.secondary)
                }
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 8, weight: .bold)).foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(maxWidth: .infinity)
            .frame(height: 28)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(.white.opacity(0.10)))
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
                        .overlay(Circle().strokeBorder(.white.opacity(selection == color ? 0.9 : 0.15), lineWidth: selection == color ? 2 : 1))
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
    @State private var confirmDelete = false
    @State private var testState = TestState.idle
    @State private var testProcess: Process?
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
        .frame(width: 560, height: 640)
        .background(Tone.canvas)
        .preferredColorScheme(.dark)
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
                .font(.system(size: 18, weight: .bold, design: .rounded))
                .padding(.horizontal, 20)
                .frame(height: 84, alignment: .leading)
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            VStack(alignment: .leading, spacing: 14) {
                SectionLabel(text: "Select a connection type")
                HStack(spacing: 12) {
                    ForEach(ConnectionKind.allCases) { option in
                        ConnectionTypeTile(kind: option) { choose(option) }
                    }
                }
                Text("The driver decides the default port, which fields are required, and what the "
                     + "object tree can browse.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(20)
            Spacer()
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            HStack(spacing: 10) {
                Button("New Connection with URI…") { step = .url }
                    .buttonStyle(.pill)
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(.pill)
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
                .font(.system(size: 18, weight: .bold, design: .rounded))
                .padding(.horizontal, 20)
                .frame(height: 84, alignment: .leading)
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            VStack(alignment: .leading, spacing: 12) {
                SectionLabel(text: "Connection URI")
                TextField("postgresql://user:password@host:5432/mydb", text: $urlText)
                    .field(invalid: urlError != nil)
                    .onSubmit { applyURL() }
                if let urlError {
                    Text(urlError).font(.system(size: 11)).foregroundStyle(Tone.coral)
                }
                Text("""
                     trino://user:password@host:8443/hive/analytics
                     postgresql://user:password@host:5432/mydb
                     mysql://user:password@host:3306/mydb
                     """)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.4))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(20)
            Spacer()
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            HStack(spacing: 10) {
                Button("Back") { step = .typePicker }
                    .buttonStyle(.pill)
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
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            ScrollView {
                form.padding(20)
            }
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
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
                    .font(.system(size: 18, weight: .bold, design: .rounded))
                    .lineLimit(1)
                Text(editingID == nil ? "Not saved yet." : "Saved \(kind.label) connection.")
                    .font(.system(size: 12))
                    .foregroundStyle(Tone.secondary)
            }
            Spacer(minLength: 12)
            if isDirty {
                Label("Unsaved changes", systemImage: "exclamationmark.circle")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Tone.amber)
                    .labelStyle(.titleAndIcon)
            }
            Button("Change Type…") { step = .typePicker }
                .buttonStyle(.pill)
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
                    LabeledField("Scheme") {
                        Segmented(selection: $scheme, options: ["https", "http"]) { $0.uppercased() }
                    }
                    .frame(width: 150)
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
            }
            LabeledField("User · Required") {
                TextField("faisal", text: $user).field(invalid: attemptedSave && missingRequired.contains("user"))
                requiredHint("user")
            }
            LabeledField(editingID == nil ? "Password" : "Password · leave blank to keep") {
                SecureField(editingID == nil ? "Stored in your Keychain" : "Unchanged", text: $credential)
                    .field()
                Text(passwordHint)
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(alignment: .top, spacing: 12) {
                LabeledField(kind.databaseLabel + (kind.requiresDatabase ? " · Required" : "")) {
                    TextField(kind == .trino ? "hive" : "mydb", text: $database)
                        .field(invalid: attemptedSave && missingRequired.contains("database"))
                    requiredHint("database")
                }
                if kind.hasSchemaLevel {
                    LabeledField("Schema") { TextField("public", text: $schema).field() }
                }
            }
            if kind.hasSSLModes {
                LabeledField("SSL mode") {
                    Segmented(selection: $sslmode, options: kind.sslModes) { $0 }
                }
            } else {
                ChipToggle(label: "Verify the TLS certificate", isOn: $verifyTLS)
            }

            if case .idle = testState {} else {
                Rectangle().fill(.white.opacity(0.08)).frame(height: 1).padding(.vertical, 2)
                testResultRow
            }
        }
    }

    private var passwordHint: String {
        switch kind {
        case .trino: "Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https."
        case .postgres: "Postgres accepts a password over any SSL mode, including \"prefer\"."
        case .mysql: "MySQL accepts a password over either SSL mode."
        }
    }

    @ViewBuilder private func requiredHint(_ key: String) -> some View {
        if attemptedSave && missingRequired.contains(key) {
            Text("Required").font(.system(size: 11)).foregroundStyle(Tone.coral)
        }
    }

    @ViewBuilder private var testResultRow: some View {
        switch testState {
        case .idle:
            EmptyView()
        case .running:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Testing…").font(.body13).foregroundStyle(Tone.secondary)
            }
        case .success(let level):
            HStack(spacing: 8) {
                Circle().fill(Tone.mint).frame(width: 10, height: 10)
                Text("Connected · \(pluralized(level, kind == .postgres ? "schema" : "catalog")).")
                    .font(.body13)
                    .textSelection(.enabled)
            }
        case .failure(let message):
            HStack(alignment: .top, spacing: 8) {
                Circle().fill(Tone.coral).frame(width: 10, height: 10).padding(.top, 4)
                Text(message).font(.body13).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Button {
                if case .running = testState { return }
                runTest()
            } label: {
                if case .running = testState {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Test Connection")
                }
            }
            .buttonStyle(.pill)
            .keyboardShortcut("t", modifiers: .command)
            .help("Test Connection (⌘T)")

            Spacer()

            if editingID != nil {
                Button("Delete") { confirmDelete = true }.buttonStyle(.coralPill)
            }
            Button("Cancel") { dismiss() }
                .buttonStyle(.pill)
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
        credential = ""
        guard let connection = original else {
            draftID = UUID()
            name = ""; color = .blue; kind = .trino; host = ""
            port = ConnectionKind.trino.defaultPort; scheme = "https"
            sslmode = ConnectionKind.postgres.defaultSSLMode
            user = ""; database = ""; schema = ""; verifyTLS = true
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
                                     verify: verifyTLS)
        // Keychain first: if it throws, the JSON never claims a password exists that isn't there.
        do {
            if !credential.isEmpty {
                try ConnectionKeychain.set(credential, for: id)
            }
            var next = model.connections
            if let index = next.firstIndex(where: { $0.id == id }) {
                next[index] = connection
            } else {
                next.append(connection)
            }
            try ConnectionStore.save(next)
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
        var next = model.connections
        next.removeAll { $0.id == id }
        do {
            try ConnectionStore.save(next)
        } catch {
            model.notice = Notice(title: "Couldn't delete connection", message: error.localizedDescription)
            return
        }
        model.connections = next
        do {
            try ConnectionKeychain.delete(for: id)
        } catch {
            model.notice = Notice(title: "Couldn't delete the Keychain password", message: error.localizedDescription)
        }
        for tab in model.tabs where tab.connectionID == id {
            tab.connectionID = model.connections.first?.id
        }
        model.rebuildTree()
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
        testProcess = Engine.run("test", env: env, onEvent: { event in
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
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(LinearGradient(colors: [.white.opacity(0.35), .clear],
                                                 startPoint: .top, endPoint: .center))
                    }
                    .overlay {
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .strokeBorder(.white.opacity(0.28))
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
                    .font(.system(size: 13, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                Text(verbatim: "port \(kind.defaultPort)")
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(Tone.secondary)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(Color.white.opacity(hovering ? 0.07 : 0.03),
                        in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(.white.opacity(hovering ? 0.18 : 0.09)))
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
            .overlay(tile.fill(LinearGradient(colors: [.white.opacity(0.35), .clear],
                                               startPoint: .top, endPoint: .center)))
            .overlay(tile.strokeBorder(.white.opacity(0.3)))
            .overlay {
                if let logo = DriverLogo.image(for: kind) {
                    Image(nsImage: logo).resizable().scaledToFit()
                        .frame(width: size * 0.56, height: size * 0.56)
                }
            }
            .frame(width: size, height: size)
            .shadow(color: hue.accent.opacity(0.5), radius: size * 0.16, y: size * 0.06)
    }
}
