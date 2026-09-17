import SwiftUI

/// 22pt-default rounded tile filled with a connection's colour gradient, carrying the
/// "this is a Trino coordinator" glyph. Shared by the tree, the toolbar picker and the title
/// strip so a connection reads the same way everywhere it appears.
func connectionTile(_ connection: Connection, size: CGFloat = 22) -> some View {
    RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
        .fill(LinearGradient(colors: [connection.color.color, connection.color.color.opacity(0.55)],
                              startPoint: .topLeading, endPoint: .bottomTrailing))
        .frame(width: size, height: size)
        .overlay(Image(systemName: "server.rack").font(.system(size: size * 0.46, weight: .semibold)).foregroundStyle(.white))
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
    let connectionID: UUID?
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var editingID: UUID?
    @State private var name = ""
    @State private var color = ConnectionColor.blue
    @State private var host = ""
    @State private var port = 8443
    @State private var httpScheme = "https"
    @State private var user = ""
    @State private var credential = ""
    @State private var catalog = ""
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
                     && catalog.isEmpty && schema.isEmpty && port == 8443 && httpScheme == "https")
        }
        return !credential.isEmpty || name != original.name || color != original.color
            || host != original.host || port != original.port || httpScheme != original.httpScheme
            || user != original.user || catalog != original.catalog || schema != original.schema
            || verifyTLS != original.verify
    }

    /// Name, host and user are what the engine cannot invent. Port and scheme always carry a
    /// value; a blank password simply means "connect without BasicAuth".
    private var missingRequired: Set<String> {
        var missing = Set<String>()
        if name.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("name") }
        if host.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("host") }
        if user.trimmingCharacters(in: .whitespaces).isEmpty { missing.insert("user") }
        if !(1...65535).contains(port) { missing.insert("port") }
        return missing
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            ScrollView {
                form.padding(20)
            }
            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)
            footer
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

    private var header: some View {
        HStack(spacing: 14) {
            SymbolHero(symbol: "server.rack", hue: .connection, size: 56, halo: false)
            VStack(alignment: .leading, spacing: 2) {
                Text(editingID == nil ? "New Connection" : (name.isEmpty ? "Connection" : name))
                    .font(.system(size: 18, weight: .bold, design: .rounded))
                    .lineLimit(1)
                Text(editingID == nil ? "Not saved yet." : "Saved coordinator.")
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
        }
        .padding(.horizontal, 20)
        .frame(height: 84)
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 14) {
            LabeledField("Name · Required") {
                TextField("Trino production", text: $name).field(invalid: attemptedSave && missingRequired.contains("name"))
                requiredHint("name")
            }
            LabeledField("Color") { ColorSwatchPicker(selection: $color) }
            LabeledField("Host · Required") {
                TextField("trino.internal", text: $host).field(invalid: attemptedSave && missingRequired.contains("host"))
                requiredHint("host")
            }
            HStack(alignment: .top, spacing: 12) {
                LabeledField("Scheme") {
                    Segmented(selection: $httpScheme, options: ["https", "http"]) { $0.uppercased() }
                }
                .frame(width: 160)
                LabeledField("Port · Required") {
                    TextField("8443", value: $port, format: .number.grouping(.never))
                        .field(invalid: attemptedSave && missingRequired.contains("port"))
                    requiredHint("port")
                }
            }
            .onChange(of: httpScheme) { old, new in
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
                Text("Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(alignment: .top, spacing: 12) {
                LabeledField("Catalog") { TextField("hive", text: $catalog).field() }
                LabeledField("Schema") { TextField("analytics", text: $schema).field() }
            }
            ChipToggle(label: "Verify the TLS certificate", isOn: $verifyTLS)

            if case .idle = testState {} else {
                Rectangle().fill(.white.opacity(0.08)).frame(height: 1).padding(.vertical, 2)
                testResultRow
            }
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
        case .success(let catalogs):
            HStack(spacing: 8) {
                Circle().fill(Tone.mint).frame(width: 10, height: 10)
                Text("Connected · \(pluralized(catalogs, "catalog")).")
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
        testState = .idle
        credential = ""
        guard let connection = original else {
            draftID = UUID()
            name = ""; color = .blue; host = ""; port = 8443; httpScheme = "https"
            user = ""; catalog = ""; schema = ""; verifyTLS = true
            return
        }
        name = connection.name
        color = connection.color
        host = connection.host
        port = connection.port
        httpScheme = connection.httpScheme
        user = connection.user
        catalog = connection.catalog
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
                                     host: host.trimmingCharacters(in: .whitespaces),
                                     port: port,
                                     httpScheme: httpScheme,
                                     user: user.trimmingCharacters(in: .whitespaces),
                                     catalog: catalog.trimmingCharacters(in: .whitespaces),
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
            host: host.trimmingCharacters(in: .whitespaces),
            port: port,
            scheme: httpScheme,
            user: user.trimmingCharacters(in: .whitespaces),
            password: credential.isEmpty ? storedPassword : credential,
            catalog: catalog.trimmingCharacters(in: .whitespaces),
            schema: schema.trimmingCharacters(in: .whitespaces),
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
