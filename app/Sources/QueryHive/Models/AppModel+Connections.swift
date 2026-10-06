import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The bastion settings of one run, in the engine's vocabulary (`SSH_*`).
struct SSHSettings: Equatable {
    var host = ""
    var useConfig = false
    /// `0` means not set.
    var port = 0
    var user = ""
    var auth = SSHAuthMethod.agent
    var keyPath = ""

    static let none = SSHSettings()

    init() {}

    init(_ connection: Connection) {
        host = connection.sshHost
        useConfig = connection.sshUseConfig
        port = connection.sshPort
        user = connection.sshUser
        auth = connection.sshAuth
        keyPath = connection.sshKeyPath
    }

    init(host: String, useConfig: Bool, port: Int, user: String, auth: SSHAuthMethod, keyPath: String) {
        self.host = host; self.useConfig = useConfig; self.port = port
        self.user = user; self.auth = auth; self.keyPath = keyPath
    }
}

/// Every secret one run may need. A value type that prints nothing: a `\(secrets)` in a log line or a
/// failed assertion shows its shape and no value.
struct ConnectionSecrets: CustomStringConvertible, CustomDebugStringConvertible {
    var password: String?
    var sshPassword: String?
    var sshPassphrase: String?
    var jwt: String?

    init(password: String? = nil, sshPassword: String? = nil, sshPassphrase: String? = nil, jwt: String? = nil) {
        self.password = password
        self.sshPassword = sshPassword
        self.sshPassphrase = sshPassphrase
        self.jwt = jwt
    }

    subscript(slot: ConnectionKeychain.Slot) -> String? {
        get {
            switch slot {
            case .database: password
            case .sshPassword: sshPassword
            case .sshPassphrase: sshPassphrase
            case .jwt: jwt
            }
        }
        set {
            switch slot {
            case .database: password = newValue
            case .sshPassword: sshPassword = newValue
            case .sshPassphrase: sshPassphrase = newValue
            case .jwt: jwt = newValue
            }
        }
    }

    var description: String { "ConnectionSecrets(…)" }
    var debugDescription: String { description }
}

extension AppModel {
    /// What the status bar names on the left, and what the dot beside it reports.
    ///
    /// This follows the connection the user is *looking at*, which is not the same as the one the
    /// active tab would run against. It used to read `selectedTab?.connectionID`, so clicking a
    /// connection in the tree changed nothing until a query tab happened to point at it — and with
    /// no tab open it reported "No query open" over a tree full of connections. Selecting a row is
    /// the user saying which connection they mean; the status bar should agree with the tree they
    /// are looking at.
    ///
    /// The tree selection wins, and the active tab is the fallback for when nothing is selected —
    /// so a tab opened from the menu (which selects its connection's root, see `selectTab`) and a
    /// row clicked by hand both land here, and there is no state where the bar names one connection
    /// while the tree highlights another.
    var statusConnection: String {
        if let connection = statusConnectionTarget {
            // Name and whether it is answering, not where it lives: the host and port were already
            // on the title strip, and the status bar is the one place that has to answer "is this
            // thing talking to the server?" at a glance.
            return "\(connection.name) · \(connectionState(for: connection.id).label)"
        }
        if connections.isEmpty { return "No connections" }
        return "No connection selected"
    }

    /// Which connection the status bar is about: the tree's selection when there is one, the
    /// active tab's otherwise.
    var statusConnectionTarget: Connection? {
        if let id = selectedNodeID,
           let parsed = connectionID(fromNodeID: id),
           let connection = connections.first(where: { $0.id == parsed }) {
            return connection
        }
        return selectedConnection
    }

    /// The connection a node id belongs to, read from the id itself rather than by walking.
    ///
    /// Every id is built from its parent's: a connection is `c:<uuid>`, and each level below appends
    /// `/cat:`, `/db:`, `/sch:` or `/tab:`. The owning connection is therefore the leading
    /// `c:<uuid>` segment, so this is a string parse instead of `allNodes()` — a full recursive
    /// walk. That matters because the status bar asks for this twice per redraw (label and dot),
    /// and DESIGN.md already records that flattening the whole tree on every redraw is one of the
    /// three mistakes that made this window slow once.
    func connectionID(fromNodeID id: String) -> UUID? {
        guard id.hasPrefix("c:") else { return nil }
        return UUID(uuidString: String(id.dropFirst(2).prefix { $0 != "/" }))
    }

    /// Whether the app is talking to a server right now.
    ///
    /// Derived from the connection's root node rather than tracked in a flag of its own: a browse
    /// command in flight *is* `node.loading`, a failed one leaves `node.error`, and a node with
    /// children has answered a browse at least once. A second flag could disagree with the tree the
    /// user is looking at; this cannot.
    func connectionState(for connectionID: UUID?) -> ConnectionState {
        guard let connectionID,
              let root = connectionNode(for: connectionID)
        else { return .disconnected }
        if root.loading { return .connecting }
        if root.error != nil { return .disconnected }
        // Not yet expanded is **idle**, not broken. Reporting `.disconnected` here announced a
        // failure that had not happened: a connection the user had simply not opened yet was
        // labelled the same as one that had just refused a connection, and on first launch that was
        // every connection in the tree.
        return root.children == nil ? .idle : .connected
    }

    /// `--bench` only: the password of its throwaway connection, so a benchmark never reads or
    /// writes a Keychain item. Nil in the real app.
    static var benchPassword: String?

    /// Where secrets are read and written. The Keychain; a test replaces it with a dictionary.
    nonisolated(unsafe) static var secretStore: any SecretStoring = SystemKeychain()

    /// The one place a saved connection's password is read, so `--bench` can stand in for it.
    static func storedPassword(for id: UUID) throws -> String? {
        try benchPassword ?? secretStore.get(slot: .database, for: id)
    }

    /// One slot of a saved connection, with the same `--bench` stand-in: a benchmark's throwaway
    /// connection has a password and nothing else, and never touches a Keychain.
    static func storedSecret(_ slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
        if let benchPassword { return slot == .database ? benchPassword : nil }
        return try secretStore.get(slot: slot, for: id)
    }

    /// Performs what `ConnectionSecretPlan` decided, in order, stopping at the first failure so the
    /// caller can say which write did not happen.
    static func apply(_ operations: [ConnectionSecretPlan.Operation], for id: UUID) throws {
        for operation in operations {
            switch operation {
            case .set(let slot, let value): try secretStore.set(value, slot: slot, for: id)
            case .remove(let slot): try secretStore.delete(slot: slot, for: id)
            }
        }
    }

    func presentConnectionEditor(_ connectionID: UUID?, startAtURL: Bool = false,
                                 previewTestCount: Int? = nil, inGroup: UUID? = nil) {
        editingConnection = ConnectionEditorTarget(connectionID, startAtURL: startAtURL,
                                                   previewTestCount: previewTestCount)
        // Filing is remembered here rather than passed through the sheet: the sheet's job is to
        // produce a connection, and which folder it belongs in is the tree's business, not a field
        // on the form. Cleared on every other way in, so editing an existing connection from the
        // header cannot silently move it.
        newConnectionGroup = inGroup
    }

    var pendingDeletionName: String {
        guard let id = pendingDeletion else { return "this connection" }
        return connections.first { $0.id == id }?.name ?? "this connection"
    }

    func requestDelete(_ id: UUID) {
        pendingDeletion = id
    }

    /// A group name being typed: a new group, or a rename of an existing one.
    ///
    /// On the model rather than in a row, for the same reason as `pendingDeletion`: the sidebar
    /// menu, the connection's own menu and any future entry point all have to open the same prompt.
    struct GroupNaming: Identifiable {
        let id = UUID()
        /// nil while creating; the group being renamed otherwise.
        var groupID: UUID?
        var name: String
        /// The connection to file into the new group, when the prompt was opened from a
        /// connection's own menu. "New Group…" there means "put this one in a new group", not
        /// "make an empty folder somewhere".
        var fileConnectionID: UUID?
    }

    /// Prompt for a new group; `connectionID` files that connection into it once named.
    func presentNewGroup(with connectionID: UUID? = nil) {
        groupNaming = GroupNaming(groupID: nil, name: "New Group", fileConnectionID: connectionID)
    }

    func presentRenameGroup(_ groupID: UUID?) {
        guard let groupID, let group = groups.first(where: { $0.id == groupID }) else { return }
        groupNaming = GroupNaming(groupID: groupID, name: group.name, fileConnectionID: nil)
    }

    /// Commits the name being typed. A blank name is refused rather than saved: a group with no
    /// name is a row the user cannot identify, and it is one keystroke to avoid.
    func commitGroupNaming() {
        guard let naming = groupNaming else { return }
        groupNaming = nil
        let name = naming.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }

        if let groupID = naming.groupID {
            guard let index = groups.firstIndex(where: { $0.id == groupID }) else { return }
            groups[index].name = name
        } else {
            let group = ConnectionGroup(name: name)
            groups.append(group)
            if let connectionID = naming.fileConnectionID,
               let index = connections.firstIndex(where: { $0.id == connectionID }) {
                connections[index].group = group.id
            }
        }
        persistConnections()
        rebuildTree()
    }

    /// Removes a group, keeping every connection that was in it.
    ///
    /// The connections move to the top level instead of being deleted. A folder that took its
    /// contents with it would be the most dangerous item in the sidebar, and nothing about the
    /// gesture says "and the servers as well".
    func deleteGroup(_ groupID: UUID?) {
        guard let groupID, let index = groups.firstIndex(where: { $0.id == groupID }) else { return }
        groups.remove(at: index)
        for position in connections.indices where connections[position].group == groupID {
            connections[position].group = nil
        }
        persistConnections()
        rebuildTree()
    }

    /// Files a connection under a group, or at the top level when `groupID` is nil.
    func move(_ connectionID: UUID, toGroup groupID: UUID?) {
        guard let index = connections.firstIndex(where: { $0.id == connectionID }) else { return }
        guard connections[index].group != groupID else { return }
        connections[index].group = groupID
        persistConnections()
        rebuildTree()
    }

    /// Writes connections.json. Failures become a notice rather than an error thrown into a menu
    /// action, which has nowhere to put one.
    func persistConnections() {
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: connections))
        } catch {
            notice = Notice(title: "Couldn't save connections", message: error.localizedDescription)
        }
    }

    /// Removes a connection, its Keychain item, and its hold on any open tab. The JSON is written
    /// before the Keychain so a failure never leaves a saved connection whose password is gone.
    func deleteConnection(_ id: UUID) {
        var next = connections
        next.removeAll { $0.id == id }
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't delete connection", message: error.localizedDescription)
            return
        }
        connections = next
        // All four slots: the database password, the bastion's password and key passphrase, the JWT.
        // Each is tried even when one fails, so a stuck item does not leave the others behind.
        var failure: Error?
        for slot in ConnectionKeychain.Slot.allCases {
            do { try Self.secretStore.delete(slot: slot, for: id) } catch { failure = failure ?? error }
        }
        if let failure {
            notice = Notice(title: "Couldn't delete the Keychain items", message: failure.localizedDescription)
        }
        for tab in tabs where tab.connectionID == id {
            tab.connectionID = connections.first?.id
        }
        rebuildTree()
    }

    /// Copies a connection under a new name. The secrets come along: a duplicate that silently
    /// connects as nobody would be a worse outcome than not duplicating at all. All four slots, so a
    /// tunnelled or JWT connection still connects.
    ///
    /// **Under `--bench` nothing is written to the Keychain** (B-6): a benchmark's connection has a
    /// stand-in password and no items, and copying that stand-in into a real login keychain would
    /// leave a stranger's secret in the owner's own.
    func duplicateConnection(_ id: UUID) {
        guard let original = connections.first(where: { $0.id == id }) else { return }
        var copy = original
        copy.id = UUID()
        copy.name = "\(original.name) copy"
        var next = connections
        next.append(copy)
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't duplicate the connection", message: error.localizedDescription)
            return
        }
        connections = next
        if Self.benchPassword == nil {
            var missed: [String] = []
            var firstError: Error?
            for slot in ConnectionKeychain.Slot.allCases {
                guard let secret = try? Self.secretStore.get(slot: slot, for: id), !secret.isEmpty else { continue }
                do {
                    try Self.secretStore.set(secret, slot: slot, for: copy.id)
                } catch {
                    missed.append(Self.secretName(slot))
                    firstError = firstError ?? error
                }
            }
            if let firstError {
                notice = Notice(title: "Duplicated, but without \(missed.joined(separator: " and "))",
                                 message: "Couldn't write it for \(copy.name) to Keychain: \(firstError.localizedDescription). Set it in the connection editor.")
            }
        }
        rebuildTree()
    }

    /// Asks for Navicat's exported `.ncx` and adds what is in it.
    ///
    /// A panel rather than a remembered path: the file is wherever the user's own export put it,
    /// and naming a location from here would be a guess about their disk.
    func presentNavicatImport() {
        let panel = NSOpenPanel()
        panel.title = "Import Connections from Navicat"
        panel.message = "Choose the .ncx file Navicat wrote for File ▸ Export Connections."
        panel.prompt = "Import"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        // A filter is applied only if the system actually has a type registered for `.ncx`. A panel
        // whose content types resolve to nothing would allow nothing, which is worse than allowing
        // too much — the parse either recognises the file or says so.
        if let ncx = UTType(filenameExtension: "ncx") { panel.allowedContentTypes = [ncx] }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        importNavicatConnections(from: url)
    }

    /// Adds every connection in a Navicat export that this app has a driver for, with the password
    /// Navicat saved alongside it.
    ///
    /// **Adds, and never replaces.** An import is a bulk edit to a hand-curated list, so an
    /// existing connection is never touched: a name already in use gets a numeric suffix and the
    /// original keeps its host and its stored password. Matching by name and updating would
    /// silently rewrite a connection the user is working in — the host and the credential — which
    /// is a far worse outcome than a duplicate row they can delete.
    ///
    /// Two invariants hold for a connection that already exists (PF-6). **An import never lowers
    /// TLS**: an entry with SSL on raises the mode to at least `require` and an entry without it
    /// changes nothing. **An import never overwrites a saved secret**: a password, bastion password
    /// or key passphrase that is already in the Keychain stays, and the summary says it was kept, so
    /// re-importing a stale file cannot swap in an old credential. A tunnel is filled in only where
    /// the connection has none.
    ///
    /// Keychain is written before the JSON, the same order the connection editor uses: if a write
    /// fails, the saved list never claims a password that is not there.
    func importNavicatConnections(from url: URL) {
        let export: NavicatImport.Export
        do {
            export = try NavicatImport.read(url)
        } catch {
            notice = Notice(title: "Couldn't read the Navicat export", message: error.localizedDescription)
            return
        }
        applyImport(ImportBatch(connections: export.connections, skipped: export.skipped), source: "Navicat")
    }

    /// Adds a batch of imported connections under the rules `importNavicatConnections` documents.
    /// One body for every source (Navicat, DBeaver, DataGrip), so a rule about TLS or secrets cannot
    /// be right for one importer and forgotten in another.
    func applyImport(_ batch: ImportBatch, source: String) {
        var next = connections
        var nextGroups = groups
        var createdGroups: [String] = []
        var taken = Set(next.map(\.name))
        var added: [String] = []
        var refreshed: [String] = []
        var renamed: [String] = []
        var noPassword: [String] = []
        var noDatabase: [String] = []
        var tunnelled: [String] = []
        var keptSecrets: [String] = []
        var notImported: [String] = []
        var keychainFailures: [String] = []

        /// Writes one imported secret under the rules above. `overwrite` is false for a connection
        /// that already exists: a slot with something in it is left alone and reported.
        func save(_ secret: String?, slot: ConnectionKeychain.Slot, for id: UUID, name: String, overwrite: Bool) {
            guard let secret, !secret.isEmpty else { return }
            if !overwrite, Self.secretStore.contains(slot: slot, for: id) {
                keptSecrets.append("\(name) (\(Self.secretName(slot)))")
                return
            }
            do {
                try Self.secretStore.set(secret, slot: slot, for: id)
            } catch {
                keychainFailures.append(name)
            }
        }

        /// The flat group an imported folder path becomes ("A / B"), made when it is first needed.
        func groupID(named name: String) -> UUID? {
            guard !name.isEmpty else { return nil }
            if let found = nextGroups.first(where: { $0.name == name }) { return found.id }
            let made = ConnectionGroup(name: name)
            nextGroups.append(made)
            createdGroups.append(name)
            return made.id
        }

        for imported in batch.connections {
            if !imported.notImported.isEmpty {
                notImported.append("\(imported.name): " + imported.notImported.joined(separator: ", "))
            }
            // Same name **and** same host means this is the very connection the export describes,
            // so it is refreshed in place instead of duplicated. Without this, re-importing — which
            // is exactly what someone does when the first import turns out to have got something
            // wrong — would append forty near-copies of connections they already have, and the only
            // way back would be deleting forty rows by hand.
            //
            // Matching on the host as well is what keeps that safe: a same-named entry pointing at a
            // different server is a different connection, and it still gets a suffix rather than
            // having its host rewritten underneath it.
            if let index = next.firstIndex(where: {
                $0.name == imported.name && $0.host == imported.host && $0.kind == imported.kind
            }) {
                let id = next[index].id
                next[index].port = imported.port
                next[index].user = imported.user
                next[index].database = imported.database
                // `schema` is deliberately not refreshed. The export has no schema to refresh it
                // with — see `NavicatImport` — so the only thing this could do is overwrite a value
                // the user set by hand with nothing.
                Self.applyTLS(imported, to: &next[index], isNew: false)
                // Filing and the label are the person's once set; an import only fills a blank.
                if next[index].group == nil { next[index].group = groupID(named: imported.group) }
                if next[index].environment == nil { next[index].environment = imported.environment }
                refreshed.append(imported.name)
                if imported.database.isEmpty { noDatabase.append(imported.name) }
                save(imported.password, slot: .database, for: id, name: imported.name, overwrite: false)
                if imported.password?.isEmpty ?? true { noPassword.append(imported.name) }
                // The tunnel only where there is none: one the person set up is theirs.
                if !imported.sshHost.isEmpty, !next[index].usesTunnel {
                    Self.applyTunnel(imported, to: &next[index])
                    tunnelled.append(imported.name)
                    save(imported.sshPassword, slot: .sshPassword, for: id, name: imported.name, overwrite: false)
                    save(imported.sshPassphrase, slot: .sshPassphrase, for: id, name: imported.name, overwrite: false)
                }
                continue
            }

            // A name that is already here on a *different* host gets " 2", " 3", … rather than
            // overwriting. The loop is bounded by the list itself, so a file full of one repeated
            // name cannot spin.
            var name = imported.name
            if taken.contains(name) {
                var suffix = 2
                while taken.contains("\(name) \(suffix)") { suffix += 1 }
                let unique = "\(name) \(suffix)"
                renamed.append("\(name) → \(unique)")
                name = unique
            }
            taken.insert(name)

            let id = UUID()
            var connection = Connection(
                id: id,
                name: name,
                // The colour is this app's own tag, not something Navicat has. Cycling the palette
                // keeps a freshly imported list visually separable instead of uniformly grey.
                color: ConnectionColor.allCases[next.count % ConnectionColor.allCases.count],
                kind: imported.kind,
                host: imported.host,
                port: imported.port,
                scheme: "https",
                user: imported.user,
                database: imported.database,
                schema: imported.schema,
                verify: false,
                showAllSchemas: false
            )
            // Navicat's `SSL` is a yes or no, and this app's modes are a ladder: yes becomes
            // `require` (encrypted, certificate not checked, the lowest rung that is TLS at all)
            // and no leaves the driver's own default. A source that names a mode (DBeaver,
            // DataGrip) gets that mode. Guessing a stricter mapping would be a connection failure,
            // and a looser one would be the downgrade PF-6 forbids.
            Self.applyTLS(imported, to: &connection, isNew: true)
            connection.group = groupID(named: imported.group)
            connection.environment = imported.environment
            if !imported.sshHost.isEmpty {
                Self.applyTunnel(imported, to: &connection)
                tunnelled.append(name)
            }

            save(imported.password, slot: .database, for: id, name: name, overwrite: true)
            if imported.password?.isEmpty ?? true { noPassword.append(name) }
            if !imported.sshHost.isEmpty {
                save(imported.sshPassword, slot: .sshPassword, for: id, name: name, overwrite: true)
                save(imported.sshPassphrase, slot: .sshPassphrase, for: id, name: name, overwrite: true)
            }
            if imported.database.isEmpty { noDatabase.append(name) }
            added.append(name)
            next.append(connection)
        }

        do {
            try ConnectionStore.save(ConnectionsDocument(groups: nextGroups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the imported connections",
                            message: error.localizedDescription)
            return
        }
        connections = next
        groups = nextGroups
        rebuildTree()
        notice = Notice(title: noticeTitle(added: added.count, refreshed: refreshed.count,
                                            skipped: batch.skipped.count, source: source),
                        message: importSummary(added: added, refreshed: refreshed, renamed: renamed,
                                               noPassword: noPassword,
                                               noDatabase: noDatabase, tunnelled: tunnelled,
                                               keptSecrets: keptSecrets, notImported: notImported,
                                               keychainFailures: keychainFailures,
                                               skipped: batch.skipped,
                                               createdGroups: createdGroups, notes: batch.notes,
                                               source: source))
    }

    /// The TLS an import writes. Never lower than what the connection has (PF-6, DBX-29): a source
    /// that names a mode raises to it, one that names nothing changes nothing, and for Trino an
    /// existing plain `http` connection is lifted to `https` while an `https` one is left alone.
    static func applyTLS(_ imported: ImportedConnection, to connection: inout Connection, isNew: Bool) {
        if connection.kind == .trino {
            guard let scheme = imported.trinoScheme else { return }
            if isNew {
                connection.scheme = scheme
                connection.verify = imported.trinoVerify ?? true
            } else if scheme == "https", TrinoTransport(stored: connection.scheme) == .http {
                connection.scheme = "https"
                connection.verify = imported.trinoVerify ?? true
            }
            return
        }
        connection.sslmode = NavicatImport.tlsMode(kind: connection.kind,
                                                   requested: imported.sslmode ?? (imported.ssl ? "require" : nil),
                                                   current: connection.sslmode)
    }

    private func noticeTitle(added: Int, refreshed: Int, skipped: Int, source: String) -> String {
        var parts: [String] = []
        if added > 0 { parts.append("Imported \(added) \(added == 1 ? "connection" : "connections")") }
        if refreshed > 0 { parts.append("refreshed \(refreshed)") }
        if parts.isEmpty { parts.append("Nothing new") }
        var title = parts.joined(separator: ", ")
        if skipped > 0 { title += ", skipped \(skipped)" }
        return title + " from \(source)"
    }

    /// Every line here exists because silence about it would be a lie of omission — a connection
    /// that arrived without its password, or with a tunnel this app cannot open, will not connect,
    /// and the user would otherwise be left to find that out one failed test at a time.
    private func importSummary(added: [String], refreshed: [String], renamed: [String],
                               noPassword: [String], noDatabase: [String], tunnelled: [String],
                               keptSecrets: [String], notImported: [String],
                               keychainFailures: [String],
                               skipped: [NavicatImport.Skipped],
                               createdGroups: [String], notes: [String], source: String) -> String {
        var lines: [String] = []
        if !added.isEmpty { lines.append("Added: " + added.joined(separator: ", ") + ".") }
        if !refreshed.isEmpty {
            lines.append("Updated in place, because the name and host already matched: "
                         + refreshed.joined(separator: ", ") + ".")
        }
        if lines.isEmpty { lines.append("Nothing was added or updated.") }
        if !createdGroups.isEmpty { lines.append("New groups: " + createdGroups.joined(separator: ", ") + ".") }
        if !renamed.isEmpty {
            lines.append("Renamed, because the name was already in use: " + renamed.joined(separator: ", ") + ".")
        }
        if !noPassword.isEmpty {
            lines.append("No password in the \(source) import for: " + noPassword.joined(separator: ", ") + ". Set one in the connection editor.")
        }
        if !keychainFailures.isEmpty {
            lines.append("Password couldn't be saved to Keychain for: " + keychainFailures.joined(separator: ", ") + ".")
        }
        if !noDatabase.isEmpty {
            lines.append("No database in the export for: " + noDatabase.joined(separator: ", ") + ". Pick one in the connection editor before browsing.")
        }
        if !tunnelled.isEmpty {
            lines.append("SSH tunnel imported for: " + tunnelled.joined(separator: ", ")
                         + ". The first time you connect, QueryHive shows the server's fingerprint and asks you to verify it.")
        }
        if !keptSecrets.isEmpty {
            lines.append("Kept the secret already saved for: " + keptSecrets.joined(separator: ", ")
                         + ". An import never replaces one; edit the connection to change it.")
        }
        if !notImported.isEmpty {
            lines.append("Not imported: " + notImported.joined(separator: "; ") + ". Set these in the connection editor.")
        }
        if !skipped.isEmpty {
            lines.append("Skipped: " + skipped.map { "\($0.name) (\($0.reason))" }.joined(separator: "; ") + ".")
        }
        lines += notes
        return lines.joined(separator: "\n\n")
    }

    // MARK: DBeaver, DataGrip and QueryHive's own list (W13-T16)

    /// Asks for DBeaver's `data-sources.json` (or the folder around it) and adds what is in it.
    ///
    /// The panel opens on DBeaver's usual folder when it exists, which is a path check and not a
    /// read; the files are read only once the person has chosen them, and the message says that the
    /// credentials file beside `data-sources.json` is part of the choice.
    func presentDBeaverImport() {
        let panel = NSOpenPanel()
        panel.title = "Import Connections from DBeaver"
        panel.message = "Choose DBeaver's data-sources.json. The credentials-config.json next to it is read too, for saved user names and passwords."
        panel.prompt = "Import"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = true
        panel.showsHiddenFiles = true
        let usual = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/DBeaverData/workspace6/General/.dbeaver")
        if FileManager.default.fileExists(atPath: usual.path) { panel.directoryURL = usual }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        importDBeaverConnections(from: url)
    }

    func importDBeaverConnections(from url: URL) {
        let batch: ImportBatch
        do {
            batch = try DBeaverImport.read(url)
        } catch {
            notice = Notice(title: "Couldn't read the DBeaver connections", message: error.localizedDescription)
            return
        }
        applyImport(batch, source: "DBeaver")
    }

    /// Asks for DataGrip's `dataSources.xml` (with its two companions) and, only if the person says
    /// so, reads the saved passwords out of the Keychain. Consent is asked here, in words, before the
    /// first system prompt: macOS then asks again per item, and each of those is the person's to refuse.
    func presentDataGripImport() {
        let panel = NSOpenPanel()
        panel.title = "Import Connections from DataGrip"
        panel.message = "Choose dataSources.xml (required), and dataSources.local.xml and db-forest-config.xml if you have them, or the folder that holds them."
        panel.prompt = "Choose"
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = true
        panel.showsHiddenFiles = true
        guard panel.runModal() == .OK, !panel.urls.isEmpty else { return }

        let alert = NSAlert()
        alert.messageText = "Read DataGrip's passwords from Keychain?"
        alert.informativeText = "DataGrip keeps saved passwords in your macOS Keychain, not in its files. If you continue, macOS asks you to allow QueryHive to read each one. Cancel a prompt and QueryHive stops asking. The passwords are copied into QueryHive's own Keychain items; DataGrip's are not changed."
        alert.addButton(withTitle: "Read Passwords")
        alert.addButton(withTitle: "Import Without Passwords")
        alert.addButton(withTitle: "Cancel")
        switch alert.runModal() {
        case .alertFirstButtonReturn: importDataGripConnections(from: panel.urls, keychain: SystemForeignKeychain())
        case .alertSecondButtonReturn: importDataGripConnections(from: panel.urls, keychain: nil)
        default: return
        }
    }

    /// `keychain` is `nil` for "import without passwords".
    func importDataGripConnections(from urls: [URL], keychain: (any ForeignKeychain)?) {
        let batch: ImportBatch
        do {
            batch = try DataGripImport.read(DataGripImport.locate(urls), keychain: keychain)
        } catch {
            notice = Notice(title: "Couldn't read the DataGrip connections", message: error.localizedDescription)
            return
        }
        applyImport(batch, source: "DataGrip")
    }

    /// Writes the connection list, groups included, with no secret in it.
    func presentConnectionListExport() {
        guard !connections.isEmpty else {
            notice = Notice(title: "Nothing to export", message: "There are no saved connections yet.")
            return
        }
        let panel = NSSavePanel()
        panel.title = "Export Connection List"
        panel.message = "Connections and groups only. No password, token or key passphrase is written."
        panel.nameFieldStringValue = "QueryHive connections.json"
        panel.allowedContentTypes = [.json]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        exportConnectionList(to: url)
    }

    func exportConnectionList(to url: URL) {
        do {
            try ConnectionListTransfer.export(groups: groups, connections: connections)
                .write(to: url, options: .atomic)
        } catch {
            notice = Notice(title: "Couldn't export the connections", message: error.localizedDescription)
            return
        }
        notice = Notice(title: "Exported \(connections.count) \(connections.count == 1 ? "connection" : "connections")",
                        message: "No passwords, tokens or key passphrases are in the file; whoever imports it sets those on their own Mac. Key-file and CA paths are as they are here.")
    }

    func presentConnectionListImport() {
        let panel = NSOpenPanel()
        panel.title = "Import Connection List"
        panel.message = "Choose a connection list exported from QueryHive."
        panel.prompt = "Import"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.json]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        importConnectionList(from: url)
    }

    /// Adds a list exported by `exportConnectionList`. **Only adds**: a connection already here
    /// (same name, host and driver) is left exactly as it is, because a file that carried a Safe
    /// Mode or a TLS mode must not be able to rewrite one the person tightened since. Each added
    /// connection gets a new id, so it cannot meet a Keychain item that belongs to another one.
    func importConnectionList(from url: URL) {
        let document: ConnectionsDocument
        do {
            document = try ConnectionListTransfer.read(Data(contentsOf: url))
        } catch {
            notice = Notice(title: "Couldn't read the connection list", message: error.localizedDescription)
            return
        }

        var next = connections
        var nextGroups = groups
        var madeGroups: [UUID: UUID] = [:]
        var taken = Set(next.map(\.name))
        var added: [String] = []
        var renamed: [String] = []
        var present: [String] = []
        var localFiles: [String] = []

        func groupID(for old: UUID?) -> UUID? {
            guard let old, let source = document.groups.first(where: { $0.id == old }) else { return nil }
            if let made = madeGroups[old] { return made }
            if let found = nextGroups.first(where: { $0.name == source.name }) {
                madeGroups[old] = found.id
                return found.id
            }
            let made = ConnectionGroup(name: source.name)
            nextGroups.append(made)
            madeGroups[old] = made.id
            return made.id
        }

        for var connection in document.connections {
            if next.contains(where: { $0.name == connection.name && $0.host == connection.host && $0.kind == connection.kind }) {
                present.append(connection.name)
                continue
            }
            var name = connection.name
            if taken.contains(name) {
                var suffix = 2
                while taken.contains("\(name) \(suffix)") { suffix += 1 }
                renamed.append("\(name) → \(name) \(suffix)")
                name = "\(name) \(suffix)"
            }
            taken.insert(name)
            connection.id = UUID()
            connection.name = name
            connection.group = groupID(for: connection.group)
            if !connection.sshKeyPath.isEmpty || !connection.caFile.isEmpty { localFiles.append(name) }
            next.append(connection)
            added.append(name)
        }

        do {
            try ConnectionStore.save(ConnectionsDocument(groups: nextGroups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the imported connections", message: error.localizedDescription)
            return
        }
        connections = next
        groups = nextGroups
        rebuildTree()

        var lines: [String] = []
        lines.append(added.isEmpty ? "Nothing was added." : "Added: " + added.joined(separator: ", ") + ".")
        if !renamed.isEmpty { lines.append("Renamed, because the name was already in use: " + renamed.joined(separator: ", ") + ".") }
        if !present.isEmpty {
            lines.append("Already here, left as they are: " + present.joined(separator: ", ")
                         + ". An import never changes a connection you have.")
        }
        if !added.isEmpty {
            lines.append("The list carries no passwords, tokens or key passphrases. Set them in the connection editor.")
        }
        if !localFiles.isEmpty {
            lines.append("These point at a key file or CA bundle by path; check the files exist on this Mac: "
                         + localFiles.joined(separator: ", ") + ".")
        }
        notice = Notice(title: added.isEmpty ? "Nothing new from the list"
                                : "Imported \(added.count) \(added.count == 1 ? "connection" : "connections") from a QueryHive list",
                        message: lines.joined(separator: "\n\n"))
    }

    /// Copies an imported entry's tunnel onto a connection. The host is taken as a name, never as an
    /// alias: Navicat has no notion of `~/.ssh/config`.
    static func applyTunnel(_ imported: ImportedConnection, to connection: inout Connection) {
        connection.sshHost = imported.sshHost
        connection.sshUseConfig = false
        connection.sshPort = imported.sshPort
        connection.sshUser = imported.sshUser
        connection.sshAuth = imported.sshAuth
        connection.sshKeyPath = imported.sshKeyPath
    }

    /// What a slot is called in a sentence.
    static func secretName(_ slot: ConnectionKeychain.Slot) -> String {
        switch slot {
        case .database: "password"
        case .sshPassword: "SSH password"
        case .sshPassphrase: "SSH key passphrase"
        case .jwt: "JWT"
        }
    }

    /// Recolours a saved connection. The colour is the user's own tag — it is what the sidebar
    /// row and the tree tile are painted with.
    func setColor(_ color: ConnectionColor, for id: UUID) {
        guard let index = connections.firstIndex(where: { $0.id == id }) else { return }
        var next = connections
        next[index].color = color
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the colour", message: error.localizedDescription)
            return
        }
        connections = next
        rebuildTree()
    }

    /// Flips "show all schemas" for a connection and re-lists its children so the change is visible
    /// immediately rather than after the next expansion. Only Postgres has anything to reveal, but
    /// the toggle is stored per-connection so it survives a relaunch like every other field.
    func toggleShowAllSchemas(_ id: UUID) {
        toggleBrowseFlag(id) { $0.showAllSchemas.toggle() }
    }

    /// Flips "show all databases" for a Postgres connection and re-lists its children.
    ///
    /// Same storage story as the schemas flag and the same reason: it is a property of the
    /// connection, so it belongs in `connections.json` beside the rest of them and has to survive
    /// the relaunch that follows. What it changes is the *shape* of the tree under the connection —
    /// a database level, or none — which is why the rebuild is the point rather than a side effect.
    func toggleShowAllDatabases(_ id: UUID) {
        toggleBrowseFlag(id) { $0.showAllDatabases.toggle() }
    }

    /// Saves one per-connection browse flag and rebuilds the tree around it.
    ///
    /// Both toggles are this: a flag that only ever changes what a *fetch* asks for, so nothing is
    /// re-run here — `rebuildTree` drops the cached children and the connection re-lists with the
    /// new flag the next time it is expanded, instead of showing children fetched under the old one.
    private func toggleBrowseFlag(_ id: UUID, _ flip: (inout Connection) -> Void) {
        guard let index = connections.firstIndex(where: { $0.id == id }) else { return }
        var next = connections
        flip(&next[index])
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the setting", message: error.localizedDescription)
            return
        }
        connections = next
        rebuildTree()
    }

    func connectionName(id: UUID) -> String {
        connections.first { $0.id == id }?.name ?? "connection"
    }

    /// The connection variables plus the scheme, shared by the export flow, the object tree and
    /// the connection editor's Test button, so the engine contract lives in exactly one place.
    /// The password travels in its own variable and never inside `TRINO_URL`, so a URL echoed
    /// back in an error message can never carry the secret.
    ///
    /// `DB_PATH` comes from `localEnvironment`, and only when something redirected the store (the
    /// tests, a snapshot, `--bench`). A run on a query lane writes its Safe Mode decision to the
    /// database it names, and with no name that is the Application Support file the installed app
    /// shares: a redirected session must log into its own.
    ///
    /// The tunnel, the JWT and the CA file follow blueprint w11 section 9.1, and the MCP server maps
    /// a stored connection the same way: `ConnectionEnvironmentTests` and `qh-ffi`'s `tests/mcp.rs`
    /// both read `crates/qh-ffi/tests/fixtures/connection_env.json` and must produce the same
    /// values. A key with no value is **left out** rather than sent empty, on both sides.
    /// `SSH_HOST_KEY_ACCEPT` is never set here: only a person pressing the host-key sheet's one
    /// button sets it, for one run (`HostKeyCenter.trust`), and `SSH_HOST_KEY_DETAIL` is added by
    /// `HostKeyGate` to every run that names a bastion.
    static func connectionEnvironment(kind: ConnectionKind, host: String, port: Int, user: String,
                                      secrets: ConnectionSecrets, database: String, schema: String,
                                      scheme: String, sslmode: String, verify: Bool,
                                      safeMode: String, ssh: SSHSettings = .none,
                                      dbAuth: DatabaseAuth = .password, caFile: String = "") -> [String: String] {
        let transport = TrinoTransport(stored: scheme)
        // A bearer token is a Trino thing: the other two drivers always send the password.
        let jwt = kind == .trino && dbAuth == .jwt
        var vars: [String: String] = [
            "DB_KIND": kind.rawValue,
            "DB_HOST": host,
            "DB_PORT": String(port),
            "DB_USER": user,
            "DB_PASSWORD": jwt ? "" : (secrets.password ?? ""),
            "DB_DATABASE": database,
            "DB_SCHEMA": schema,
            // Trino's transport is a scheme; the other two express encryption through sslmode.
            // Sending the wrong one is harmless — the engine ignores what a driver has no use
            // for — but sending both would be confusing to read in a bug report.
            //
            // `prefer` is the one mode that needs both, and needs `sslmode` to be the one that
            // names it: a scheme can say "clear" or "encrypt", never "encrypt if you can". So
            // `DB_SCHEME=https` still goes out — it is where the mode starts, and the engine
            // reads it — and `DB_SSLMODE=prefer` decides, because a named sslmode outranks the
            // scheme in `qh_ffi::config`. The scheme word is sent unchanged rather than
            // normalised, so a value the engine would refuse is still refused out loud.
            "DB_SCHEME": kind == .trino ? (transport == .prefer ? "https" : (scheme.isEmpty ? "http" : scheme)) : "",
            // Only `prefer` is named for Trino. A stored Trino connection's `sslmode` is
            // whatever an earlier editor left there (it wrote Postgres's default into it), and
            // sending that for the other transports would outrank the scheme — turning a
            // connection the user set to https into a clear one, silently.
            "DB_SSLMODE": kind == .trino
                ? (transport == .prefer ? TrinoTransport.prefer.rawValue : "")
                : (sslmode.isEmpty ? kind.defaultSSLMode : sslmode),
            // `prefer` decides its own verification, and it never checks: `DB_INSECURE` would
            // otherwise demote it to a required, unverified connection, which is a different
            // mode and not the fallback the user asked for. Every other transport keeps the
            // stored flag, `http` included — the engine ignores it in clear.
            "DB_INSECURE": kind == .trino && transport == .prefer ? "" : (verify ? "" : "1"),
            // What the engine refuses, enforced there and not here: the CLI and the MCP
            // server run the same guard, and neither has this picker. Sent for every
            // driver, because the levels are about SQL and not about a server.
            "SAFE_MODE": safeMode,
        ]
        func put(_ key: String, _ value: String?) {
            if let value, !value.isEmpty { vars[key] = value }
        }
        if jwt { put("DB_JWT", secrets.jwt) }
        put("DB_CA_FILE", caFile.trimmingCharacters(in: .whitespaces))
        let bastion = ssh.host.trimmingCharacters(in: .whitespaces)
        if !bastion.isEmpty {
            put("SSH_HOST", bastion)
            if ssh.useConfig { put("SSH_USE_CONFIG", "1") }
            if ssh.port != 0 { put("SSH_PORT", String(ssh.port)) }
            put("SSH_USER", ssh.user.trimmingCharacters(in: .whitespaces))
            put("SSH_AUTH_METHOD", ssh.auth.rawValue)
            put("SSH_KEY_PATH", ssh.keyPath.trimmingCharacters(in: .whitespaces))
            switch ssh.auth {
            case .password: put("SSH_PASSWORD", secrets.sshPassword)
            case .key: put("SSH_KEY_PASSPHRASE", secrets.sshPassphrase)
            case .agent: break
            }
            // The app's own trust file: where `HostKeyCenter`'s pinned run records an accepted key.
            if let directory = try? ConnectionStore.directory() {
                put("SSH_APP_KNOWN_HOSTS", directory.appendingPathComponent("known_hosts").path)
            }
        }
        return localEnvironment(vars)
    }

    /// A saved connection with the secrets already in hand.
    static func connectionEnvironment(_ connection: Connection, secrets: ConnectionSecrets) -> [String: String] {
        connectionEnvironment(kind: connection.kind, host: connection.host, port: connection.port,
                              user: connection.user, secrets: secrets,
                              database: connection.database, schema: connection.schema,
                              scheme: connection.scheme, sslmode: connection.sslmode,
                              verify: connection.verify, safeMode: connection.safeMode.rawValue,
                              ssh: SSHSettings(connection), dbAuth: connection.dbAuth,
                              caFile: connection.caFile)
    }

    /// For a caller that holds only the database password (the tree's browse and the warm-up).
    ///
    /// The other slots the connection uses are read **without a prompt**: this runs on paths that
    /// must not put a dialog up, so an item that would need approval is left out, and the run says
    /// what is missing (the engine names the setting). Under `--bench` there is no Keychain to read.
    static func connectionEnvironment(_ connection: Connection, password: String?) -> [String: String] {
        var secrets = ConnectionSecrets(password: password)
        if benchPassword == nil {
            for slot in ConnectionKeychain.Slot.allCases
            where slot != .database && ConnectionSecretPlan.uses(slot, connection) {
                secrets[slot] = (try? ConnectionKeychain.getWithoutPrompt(slot: slot, for: connection.id)) ?? nil
            }
        }
        return connectionEnvironment(connection, secrets: secrets)
    }

    /// What a statement may run for on this connection: its own bound when it has one, the app-wide
    /// setting otherwise. `nil` on the connection inherits, so changing the setting still moves it.
    func effectiveStatementTimeoutMS(_ connection: Connection) -> Int {
        connection.statementTimeoutMS ?? statementTimeoutMS
    }

    /// The connection variables for a saved connection, Keychain read included. Shared by `run`
    /// and the target-option fetches so those cannot drift from what a run actually sends.
    ///
    /// The statement timeout is added here rather than at each run path: this is the one
    /// function every path that runs caller SQL goes through, and a bound that reached
    /// `preview` but not `count` would be a bound the user thinks they set.
    func connectionEnvironment(_ connection: Connection) throws -> [String: String] {
        var secrets = ConnectionSecrets()
        for slot in ConnectionKeychain.Slot.allCases where ConnectionSecretPlan.uses(slot, connection) {
            do {
                secrets[slot] = try Self.storedSecret(slot, for: connection.id)
            } catch {
                throw EngineLaunchError(message: "Couldn't read the \(Self.secretName(slot)) for \(connection.name) from Keychain: \(error.localizedDescription)")
            }
        }
        var vars = Self.connectionEnvironment(connection, secrets: secrets)
        vars["STATEMENT_TIMEOUT_MS"] = String(effectiveStatementTimeoutMS(connection))
        return vars
    }

    /// A host key was trusted in the sheet: every failed node of a tunnelled connection loads again,
    /// which is what the person was doing when the sheet came up. Operations other than the tree
    /// are not repeated for them; trusting a key and retrying a statement are separate acts.
    func reloadAfterHostKeyTrusted() {
        let tunnelled = Set(connections.filter(\.usesTunnel).map(\.id))
        guard !tunnelled.isEmpty else { return }
        for node in allNodes() where node.error != nil {
            guard let id = node.connectionID, tunnelled.contains(id) else { continue }
            refresh(node)
        }
    }
}
