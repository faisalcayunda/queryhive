import Foundation

/// What `~/.ssh/config` says about one `Host` alias: the same four things the engine's
/// `ssh_config_resolve` returns, and what a connect with `SSH_USE_CONFIG=1` would use for whatever the
/// form leaves blank.
struct SSHAliasInfo: Equatable {
    var hostName: String
    var user: String?
    var port: Int?
    var identityFiles: [String]

    /// "→ bastion.corp:22 as deploy, key ~/.ssh/id_ed25519": the preview under the alias picker.
    var summary: String {
        var text = "→ \(hostName):\(port ?? 22)"
        if let user, !user.isEmpty { text += " as \(user)" }
        if let key = identityFiles.first { text += ", key \(key)" }
        return text
    }
}

/// Why an alias cannot be used, in the engine's own words: it names the file, the line and the
/// directive, never a value.
struct SSHAliasError: Error, Equatable {
    var message: String
}

/// Where the form gets the aliases to offer and the preview of the one chosen.
///
/// A seam rather than a call, because the answer comes from the engine (`ssh_config_hosts()` and
/// `ssh_config_resolve(alias:)` over UniFFI) and `RustEngine` is the only type that imports the
/// generated module (blueprint section 1.6). Until the bindings that carry those two functions are
/// generated (`app/build-ffi.sh`), the default offers no aliases and fails every preview with a
/// sentence that says so; the wiring is one assignment at the composition root.
struct SSHConfigAliases {
    var hosts: () -> [String]
    var resolve: (String) -> Result<SSHAliasInfo, SSHAliasError>

    static let unavailable = SSHConfigAliases(
        hosts: { [] },
        resolve: { _ in .failure(SSHAliasError(message: "This build can't read ~/.ssh/config yet.")) })

    /// Set once at launch by whoever owns the engine; read by the form.
    nonisolated(unsafe) static var current = unavailable
}

/// The fields of the connection form that can be missing or wrong, in the order the sheet shows
/// them.
enum ConnectionField: String, CaseIterable {
    case name, host, port, user, database, sshHost, sshPort, sshUser, sshKeyFile, caFile, timeout

    /// The name a message uses. A label with a parenthesis is a value that is *present but wrong*;
    /// the others are values that are *missing*, and `ConnectionFormIssues.message` words them
    /// differently.
    var label: String {
        switch self {
        case .name: "Name"
        case .host: "Host"
        case .port: "Port (1-65535)"
        case .user: "User"
        case .database: "Database"
        case .sshHost: "SSH host"
        case .sshPort: "SSH port (1-65535)"
        case .sshUser: "SSH user"
        case .sshKeyFile: "SSH key file"
        case .caFile: "CA file (not found)"
        case .timeout: "Statement timeout (0-600 seconds)"
        }
    }
}

/// What the form holds, reduced to what validation reads. A plain value so the rules are tested
/// without a window.
struct ConnectionFormState {
    var kind: ConnectionKind
    var name = ""
    var host = ""
    var port = 0
    var user = ""
    var database = ""
    var sshEnabled = false
    var sshHost = ""
    var sshUseConfig = false
    /// `0` means not set (the default, or the alias's own).
    var sshPort = 0
    var sshUser = ""
    var sshAuth = SSHAuthMethod.agent
    var sshKeyPath = ""
    var caFile = ""
    /// The per-connection statement timeout as typed, in seconds. Blank inherits.
    var timeoutText = ""
    /// Whether a path names a file. Injected so a test does not need the disk.
    var fileExists: (String) -> Bool = { path in
        FileManager.default.fileExists(atPath: (path as NSString).expandingTildeInPath)
    }
}

/// The named validation of FR-CON-09: which fields are missing or wrong, and one sentence about
/// them. Pure, so Test Connection and Save can refuse in the same words before they touch anything.
enum ConnectionFormIssues {
    /// The fields to fix, in sheet order. `resolved` is what the chosen `~/.ssh/config` alias
    /// provides (nil when the host is not an alias or the alias did not resolve): an alias that
    /// names the user or the key file satisfies the field, which is the whole point of picking one.
    static func fields(for state: ConnectionFormState, resolved: SSHAliasInfo?) -> [ConnectionField] {
        func blank(_ text: String) -> Bool { text.trimmingCharacters(in: .whitespaces).isEmpty }
        let tunnel = state.sshEnabled
        let alias = tunnel && state.sshUseConfig ? resolved : nil
        var out: [ConnectionField] = []
        if blank(state.name) { out.append(.name) }
        if blank(state.host) { out.append(.host) }
        if !(1...65535).contains(state.port) { out.append(.port) }
        if blank(state.user) { out.append(.user) }
        if state.kind.requiresDatabase, blank(state.database) { out.append(.database) }
        if tunnel {
            if blank(state.sshHost) { out.append(.sshHost) }
            if state.sshPort != 0, !(1...65535).contains(state.sshPort) { out.append(.sshPort) }
            if blank(state.sshUser), blank(alias?.user ?? "") { out.append(.sshUser) }
            if state.sshAuth == .key, blank(state.sshKeyPath), alias?.identityFiles.isEmpty ?? true {
                out.append(.sshKeyFile)
            }
        }
        if !blank(state.caFile), !state.fileExists(state.caFile.trimmingCharacters(in: .whitespaces)) {
            out.append(.caFile)
        }
        let typed = state.timeoutText.trimmingCharacters(in: .whitespaces)
        if !typed.isEmpty, !(Int(typed).map { (0...600).contains($0) } ?? false) { out.append(.timeout) }
        return out
    }

    /// The same list as labels.
    static func missing(for state: ConnectionFormState, resolved: SSHAliasInfo?) -> [String] {
        fields(for: state, resolved: resolved).map(\.label)
    }

    /// "Host and User are required." / "Host, Database and SSH user are required." Values that are
    /// there but wrong follow, so the sentence never says a present value is "required".
    static func message(_ labels: [String]) -> String {
        let required = labels.filter { !$0.contains("(") }
        let wrong = labels.filter { $0.contains("(") }
        var sentences: [String] = []
        if !required.isEmpty {
            sentences.append("\(list(required)) \(required.count == 1 ? "is" : "are") required.")
        }
        if !wrong.isEmpty { sentences.append("Check \(list(wrong)).") }
        return sentences.joined(separator: " ")
    }

    private static func list(_ items: [String]) -> String {
        switch items.count {
        case 0: ""
        case 1: items[0]
        default: items.dropLast().joined(separator: ", ") + " and " + items[items.count - 1]
        }
    }
}

/// Which secrets a saved connection actually uses, and what saving the form does to the Keychain
/// (blueprint section 8 as amended by PF-5).
///
/// The rule that matters, and the reason this is a type of its own: **an empty field means keep**.
/// Saving a connection whose secret fields were left blank touches nothing, because the form
/// cannot show a saved secret, so "blank" can only mean "I did not retype it". A secret is deleted
/// in exactly two cases: the person pressed an explicit "Remove saved…", or the connection stopped
/// using it (the auth method changed, the tunnel was turned off, the driver changed).
enum ConnectionSecretPlan {
    enum Operation: Equatable {
        case set(ConnectionKeychain.Slot, String)
        case remove(ConnectionKeychain.Slot)
    }

    /// What the form holds for each slot: what was typed, and which slots were explicitly removed.
    struct Draft {
        var password = ""
        var sshPassword = ""
        var sshPassphrase = ""
        var jwt = ""
        var removals: Set<ConnectionKeychain.Slot> = []

        func typed(_ slot: ConnectionKeychain.Slot) -> String {
            switch slot {
            case .database: password
            case .sshPassword: sshPassword
            case .sshPassphrase: sshPassphrase
            case .jwt: jwt
            }
        }
    }

    /// Whether a connection reads a slot at all.
    static func uses(_ slot: ConnectionKeychain.Slot, _ connection: Connection) -> Bool {
        let jwt = connection.kind == .trino && connection.dbAuth == .jwt
        switch slot {
        case .database: return !jwt
        case .jwt: return jwt
        case .sshPassword: return connection.usesTunnel && connection.sshAuth == .password
        case .sshPassphrase: return connection.usesTunnel && connection.sshAuth == .key
        }
    }

    /// The writes and deletions Save performs. `old` is the connection as it was saved (nil for a
    /// new one, which has nothing to remove).
    static func operations(draft: Draft, old: Connection?, new: Connection) -> [Operation] {
        var out: [Operation] = []
        for slot in ConnectionKeychain.Slot.allCases {
            let typed = draft.typed(slot)
            if !typed.isEmpty, uses(slot, new) {
                out.append(.set(slot, typed))
            } else if draft.removals.contains(slot) {
                out.append(.remove(slot))
            } else if let old, uses(slot, old), !uses(slot, new) {
                out.append(.remove(slot))
            }
        }
        return out
    }
}
