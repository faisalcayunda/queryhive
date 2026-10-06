import AppKit
import SwiftUI
import UniformTypeIdentifiers

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

    /// The one place a saved connection's password is read, so `--bench` can stand in for it.
    static func storedPassword(for id: UUID) throws -> String? {
        try benchPassword ?? ConnectionKeychain.get(for: id)
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
        do {
            try ConnectionKeychain.delete(for: id)
        } catch {
            notice = Notice(title: "Couldn't delete the Keychain password", message: error.localizedDescription)
        }
        for tab in tabs where tab.connectionID == id {
            tab.connectionID = connections.first?.id
        }
        rebuildTree()
    }

    /// Copies a connection under a new name. The password comes along: a duplicate that silently
    /// connects as nobody would be a worse outcome than not duplicating at all.
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
        if let password = try? Self.storedPassword(for: id), !password.isEmpty {
            do {
                try ConnectionKeychain.set(password, for: copy.id)
            } catch {
                notice = Notice(title: "Duplicated, but without the password",
                                 message: "Couldn't write the password for \(copy.name) to Keychain: \(error.localizedDescription). Set it in the connection editor.")
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

        var next = connections
        var taken = Set(next.map(\.name))
        var added: [String] = []
        var refreshed: [String] = []
        var renamed: [String] = []
        var noPassword: [String] = []
        var noDatabase: [String] = []
        var tunnelled: [String] = []
        var keychainFailures: [String] = []

        for imported in export.connections {
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
                $0.name == imported.name && $0.host == imported.host
            }) {
                let id = next[index].id
                next[index].kind = imported.kind
                next[index].port = imported.port
                next[index].user = imported.user
                next[index].database = imported.database
                // `schema` is deliberately not refreshed. The export has no schema to refresh it
                // with — see `NavicatImport` — so the only thing this could do is overwrite a value
                // the user set by hand with nothing.
                refreshed.append(imported.name)
                if imported.database.isEmpty { noDatabase.append(imported.name) }
                if !imported.sshHost.isEmpty { tunnelled.append(imported.name) }
                if let password = imported.password, !password.isEmpty {
                    do {
                        try ConnectionKeychain.set(password, for: id)
                    } catch {
                        keychainFailures.append(imported.name)
                    }
                } else {
                    noPassword.append(imported.name)
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
            let connection = Connection(
                id: id,
                name: name,
                // The colour is this app's own tag, not something Navicat has. Cycling the palette
                // keeps a freshly imported list visually separable instead of uniformly grey.
                color: ConnectionColor.allCases[next.count % ConnectionColor.allCases.count],
                kind: imported.kind,
                host: imported.host,
                port: imported.port,
                scheme: imported.kind == .trino ? "https" : "https",
                // The export carries encryption settings this app spells differently, and guessing
                // a mapping would be worse than leaving the default: an sslmode that is wrong is a
                // connection failure, not a silent one.
                sslmode: "",
                user: imported.user,
                database: imported.database,
                schema: imported.schema,
                verify: false,
                showAllSchemas: false
            )

            if let password = imported.password, !password.isEmpty {
                do {
                    try ConnectionKeychain.set(password, for: id)
                } catch {
                    keychainFailures.append(name)
                }
            } else {
                noPassword.append(name)
            }
            if imported.database.isEmpty { noDatabase.append(name) }
            if !imported.sshHost.isEmpty { tunnelled.append(name) }
            added.append(name)
            next.append(connection)
        }

        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the imported connections",
                            message: error.localizedDescription)
            return
        }
        connections = next
        rebuildTree()
        notice = Notice(title: noticeTitle(added: added.count, refreshed: refreshed.count,
                                            skipped: export.skipped.count),
                        message: importSummary(added: added, refreshed: refreshed, renamed: renamed,
                                               noPassword: noPassword,
                                               noDatabase: noDatabase, tunnelled: tunnelled,
                                               keychainFailures: keychainFailures,
                                               skipped: export.skipped))
    }

    private func noticeTitle(added: Int, refreshed: Int, skipped: Int) -> String {
        var parts: [String] = []
        if added > 0 { parts.append("Imported \(added) \(added == 1 ? "connection" : "connections")") }
        if refreshed > 0 { parts.append("refreshed \(refreshed)") }
        if parts.isEmpty { parts.append("Nothing new") }
        var title = parts.joined(separator: ", ")
        if skipped > 0 { title += ", skipped \(skipped)" }
        return title + " from Navicat"
    }

    /// Every line here exists because silence about it would be a lie of omission — a connection
    /// that arrived without its password, or with a tunnel this app cannot open, will not connect,
    /// and the user would otherwise be left to find that out one failed test at a time.
    private func importSummary(added: [String], refreshed: [String], renamed: [String],
                               noPassword: [String], noDatabase: [String], tunnelled: [String],
                               keychainFailures: [String],
                               skipped: [NavicatImport.Skipped]) -> String {
        var lines: [String] = []
        if !added.isEmpty { lines.append("Added: " + added.joined(separator: ", ") + ".") }
        if !refreshed.isEmpty {
            lines.append("Updated in place, because the name and host already matched: "
                         + refreshed.joined(separator: ", ") + ".")
        }
        if lines.isEmpty { lines.append("Nothing was added or updated.") }
        if !renamed.isEmpty {
            lines.append("Renamed, because the name was already in use: " + renamed.joined(separator: ", ") + ".")
        }
        if !noPassword.isEmpty {
            lines.append("No password in the export for: " + noPassword.joined(separator: ", ") + ". Set one in the connection editor.")
        }
        if !keychainFailures.isEmpty {
            lines.append("Password couldn't be saved to Keychain for: " + keychainFailures.joined(separator: ", ") + ".")
        }
        if !noDatabase.isEmpty {
            lines.append("No database in the export for: " + noDatabase.joined(separator: ", ") + ". Pick one in the connection editor before browsing.")
        }
        if !tunnelled.isEmpty {
            lines.append("These use an SSH tunnel, which this app doesn't open: " + tunnelled.joined(separator: ", ") + ".")
        }
        if !skipped.isEmpty {
            lines.append("Skipped: " + skipped.map { "\($0.name) (\($0.reason))" }.joined(separator: "; ") + ".")
        }
        return lines.joined(separator: "\n\n")
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
    static func connectionEnvironment(kind: ConnectionKind, host: String, port: Int, user: String,
                                      password: String?, database: String, schema: String,
                                      scheme: String, sslmode: String, verify: Bool,
                                      safeMode: String) -> [String: String] {
        let transport = TrinoTransport(stored: scheme)
        return [
            "DB_KIND": kind.rawValue,
            "DB_HOST": host,
            "DB_PORT": String(port),
            "DB_USER": user,
            "DB_PASSWORD": password ?? "",
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
    }

    static func connectionEnvironment(_ connection: Connection, password: String?) -> [String: String] {
        connectionEnvironment(kind: connection.kind, host: connection.host, port: connection.port,
                              user: connection.user, password: password,
                              database: connection.database, schema: connection.schema,
                              scheme: connection.scheme, sslmode: connection.sslmode,
                              verify: connection.verify, safeMode: connection.safeMode.rawValue)
    }

    /// The connection variables for a saved connection, Keychain read included. Shared by `run`
    /// and the target-option fetches so those cannot drift from what a run actually sends.
    ///
    /// The statement timeout is added here rather than at each run path: this is the one
    /// function every path that runs caller SQL goes through, and a bound that reached
    /// `preview` but not `count` would be a bound the user thinks they set.
    func connectionEnvironment(_ connection: Connection) throws -> [String: String] {
        let password: String?
        do {
            password = try Self.storedPassword(for: connection.id)
        } catch {
            throw EngineLaunchError(message: "Couldn't read the password for \(connection.name) from Keychain: \(error.localizedDescription)")
        }
        var env = Self.connectionEnvironment(connection, password: password)
        env["STATEMENT_TIMEOUT_MS"] = String(statementTimeoutMS)
        return env
    }
}
