import Foundation

/// What the connection sheet holds while a person edits: every field of a `Connection`, as typed,
/// plus the secrets typed beside it. One value instead of a `@State` per field, so the rules that
/// tie fields together live here and are tested without a window.
///
/// Nothing here reaches the disk or the Keychain. `connection(id:group:)` is what Save stores
/// (secrets aside), and `secrets` is what `ConnectionSecretPlan` turns into Keychain writes: the
/// form never holds a saved secret, so a blank one means "keep".
struct ConnectionDraft: Equatable {
    var name = ""
    var color = ConnectionColor.blue
    var kind = ConnectionKind.trino
    var host = ""
    var port = ConnectionKind.trino.defaultPort
    var scheme = "https"
    var sslmode = ConnectionKind.postgres.defaultSSLMode
    var user = ""
    var database = ""
    var schema = ""
    var verifyTLS = true
    /// Postgres only: list system schemas too. Meaningful nowhere else, so the field is shown
    /// only for that driver, mirroring the tree's own context-menu toggle.
    var showAllSchemas = false
    /// Postgres only, and the same deal one level up: draw a database level under the connection
    /// instead of starting at the schemas of the database named above.
    var showAllDatabases = false
    /// What the engine refuses on this connection. Chosen here, enforced there: the picker
    /// only records the level, and `AppModel.connectionEnvironment` sends it as `SAFE_MODE`.
    var safeMode = ConnectionSafeMode.full
    /// A label shown in the tab, breadcrumb and status bar. It changes nothing else.
    var environment: ConnectionEnvironment?
    // The W11 fields. Secrets are typed into `secrets` and written on Save; a saved one is never shown.
    var sshEnabled = false
    var sshHost = ""
    var sshUseConfig = false
    /// `0` is "not set": 22, or the alias's own port.
    var sshPort = 0
    var sshUser = ""
    var sshAuth = SSHAuthMethod.agent
    var sshKeyPath = ""
    var dbAuth = DatabaseAuth.password
    var caFile = ""
    /// Seconds, as typed; blank inherits the app-wide setting.
    var timeoutText = ""
    /// What was typed for each Keychain slot, and the slots the person pressed "Remove saved…" for.
    /// Staged: nothing leaves the Keychain until Save.
    var secrets = ConnectionSecretPlan.Draft()

    /// The blank form of a connection that does not exist yet.
    static func new() -> ConnectionDraft { ConnectionDraft() }

    /// The form of a saved connection, as it is saved. Nothing is normalised: a blank `sslmode` stays
    /// blank (the encryption control reads it as the driver's default), so opening a connection
    /// never makes it look changed (blueprint T-1).
    init(_ connection: Connection) {
        name = connection.name
        color = connection.color
        kind = connection.kind
        host = connection.host
        port = connection.port
        scheme = connection.scheme
        sslmode = connection.sslmode
        user = connection.user
        database = connection.database
        schema = connection.schema
        verifyTLS = connection.verify
        showAllSchemas = connection.showAllSchemas
        showAllDatabases = connection.showAllDatabases
        safeMode = connection.safeMode
        environment = connection.environment
        sshEnabled = connection.usesTunnel
        sshHost = connection.sshHost
        sshUseConfig = connection.sshUseConfig
        sshPort = connection.sshPort
        sshUser = connection.sshUser
        sshAuth = connection.sshAuth
        sshKeyPath = connection.sshKeyPath
        dbAuth = connection.dbAuth
        caFile = connection.caFile
        timeoutText = connection.statementTimeoutMS.map { String($0 / 1000) } ?? ""
    }

    init() {}

    // MARK: What the form describes

    /// The connection the form describes, secrets aside. Compared against the saved one for the
    /// "Unsaved changes" mark, and saved as it is.
    func connection(id: UUID, group: UUID?) -> Connection {
        Connection(id: id,
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
                   showAllSchemas: showAllSchemas,
                   showAllDatabases: showAllDatabases,
                   group: group,
                   safeMode: safeMode,
                   environment: environment,
                   // Turning the tunnel off clears its fields on Save, as the blueprint says; the
                   // values stay in the form until then so turning it back on finds them.
                   sshHost: sshEnabled ? sshHost.trimmingCharacters(in: .whitespaces) : "",
                   sshUseConfig: sshEnabled && sshUseConfig,
                   sshPort: sshEnabled ? sshPort : 0,
                   sshUser: sshEnabled ? sshUser.trimmingCharacters(in: .whitespaces) : "",
                   sshAuth: sshEnabled ? sshAuth : .agent,
                   sshKeyPath: sshEnabled && sshAuth == .key ? sshKeyPath.trimmingCharacters(in: .whitespaces) : "",
                   dbAuth: kind == .trino ? dbAuth : .password,
                   caFile: kind == .mysql ? "" : caFile.trimmingCharacters(in: .whitespaces),
                   statementTimeoutMS: timeoutMS)
    }

    /// The per-connection bound in milliseconds, `nil` when the field is blank (inherit) or not a
    /// number (which `ConnectionFormIssues` reports).
    var timeoutMS: Int? {
        guard let seconds = Int(timeoutText.trimmingCharacters(in: .whitespaces)), (0...600).contains(seconds)
        else { return nil }
        return seconds * 1000
    }

    /// What validation reads.
    var formState: ConnectionFormState {
        ConnectionFormState(kind: kind, name: name, host: host, port: port, user: user, database: database,
                            sshEnabled: sshEnabled, sshHost: sshHost, sshUseConfig: sshUseConfig,
                            sshPort: sshPort, sshUser: sshUser, sshAuth: sshAuth, sshKeyPath: sshKeyPath,
                            caFile: kind == .mysql ? "" : caFile, timeoutText: timeoutText)
    }

    /// Whether Save has anything to save. A new connection counts once any field holds something;
    /// a saved one when a secret was typed or staged for removal, or the connection it describes
    /// differs from the stored one. The stored one is read through the draft too, so what Save
    /// would tidy (a trailing space an import left, a timeout that is not whole seconds, a CA on a
    /// MySQL connection) is not a change the person made.
    func isDirty(against original: Connection?) -> Bool {
        guard let original else {
            return !(name.isEmpty && host.isEmpty && user.isEmpty && secrets.password.isEmpty
                     && database.isEmpty && schema.isEmpty && !sshEnabled && secrets.sshPassword.isEmpty
                     && secrets.sshPassphrase.isEmpty && secrets.jwt.isEmpty && caFile.isEmpty
                     && timeoutText.isEmpty)
        }
        return !secrets.password.isEmpty || !secrets.sshPassword.isEmpty || !secrets.sshPassphrase.isEmpty
            || !secrets.jwt.isEmpty || !secrets.removals.isEmpty
            || connection(id: original.id, group: original.group)
                != ConnectionDraft(original).connection(id: original.id, group: original.group)
    }

    // MARK: Rules that tie fields together

    /// Picking a tile is the only place the driver is chosen; the form then has no type control,
    /// because changing it would invalidate half of what is already filled in.
    mutating func choose(_ option: ConnectionKind) {
        // Picking the driver the draft already has is not a change: nothing is reset, filled in or
        // enforced, so Change Type and the same tile leave the form as it was.
        guard option != kind else { return }
        if port == kind.defaultPort { port = option.defaultPort }
        if option.hasSSLModes, sslmode.isEmpty { sslmode = option.defaultSSLMode }
        // What only applied to the other driver goes with it: a JWT is Trino's, and MySQL has no CA.
        if option != .trino { dbAuth = .password; secrets.jwt = "" }
        if option == .mysql { caFile = "" }
        // `verify` is not Trino's alone: for PostgreSQL and MySQL the engine turns `verify == false`
        // into "encrypt, never check" for any mode but `disable`, which for `prefer` is also the
        // difference between "may fall back to clear" and "must encrypt". So a flag that meant
        // "unverified HTTPS" on Trino must not ride into another driver and silently decide that,
        // and it is reset when the driver is left. Moving between PostgreSQL and MySQL leaves
        // `verify` alone: it is the connection's existing answer there, and changing it would
        // change what the connection does without anything on the form changing.
        if kind == .trino, option != .trino { verifyTLS = true }
        kind = option
        enforceCAConstraint()
    }

    /// What a pasted URL meant. It fills the fields the URL speaks about and leaves the rest as
    /// they were; the name is only filled while it is blank. The URL's port is kept as written (the
    /// sheet's old change handler on `scheme` used to rewrite 8080 and 8443 after this).
    ///
    /// A CA left in the draft wins over the URL's own mode: `sslmode=disable` or `http://` ends as a
    /// verifying mode, because a CA means nothing to any other (`enforceCAConstraint`). That is the
    /// safe direction, the CA is a choice the person made earlier, and the form is shown before
    /// anything is saved.
    mutating func apply(_ parsed: ConnectionURL.Parsed) {
        kind = parsed.kind
        host = parsed.host
        port = parsed.port
        scheme = parsed.scheme.isEmpty ? "https" : parsed.scheme
        sslmode = parsed.sslmode ?? parsed.kind.defaultSSLMode
        verifyTLS = parsed.verify ?? true
        user = parsed.user
        secrets.password = parsed.password ?? ""
        database = parsed.database
        schema = parsed.schema
        if name.isEmpty {
            name = parsed.host + (parsed.database.isEmpty ? "" : "/\(parsed.database)")
        }
        enforceCAConstraint()
    }

    /// Writes the scheme and lets the port follow it.
    mutating func setScheme(_ new: String) {
        let old = TrinoTransport(stored: scheme)
        scheme = new
        // Follow the standard port for the scheme, but only while the user is still on the other
        // scheme's standard one. Read as the engine reads a scheme, so a blank one is HTTP.
        let now = TrinoTransport(stored: new)
        if old == .http, now == .https, port == 8080 { port = 8443 }
        if old == .https, now == .http, port == 8443 { port = 8080 }
        // `prefer` moves nothing. It tries HTTPS and falls back to clear on whatever
        // port it was given, which is the only port that can answer the question the
        // transport exists to leave open; rewriting the port would answer it for the
        // coordinator instead of asking it.
    }

    /// Writes the CA file path and applies what a CA means.
    mutating func setCAFile(_ path: String) {
        caFile = path
        enforceCAConstraint()
    }

    /// Whether there is a CA path worth acting on: one that is not blank once trimmed, as Save
    /// trims it.
    var hasCA: Bool { !caFile.trimmingCharacters(in: .whitespaces).isEmpty }

    /// A CA only means something to a mode that checks the certificate (blueprint 9.1), so a CA in
    /// the draft moves the mode to one that does, and turns verification on. It runs wherever the
    /// draft can change under a CA: choosing the CA, choosing a driver, applying a URL. It does
    /// not run on load, so a saved connection that already breaks the rule is shown as saved.
    /// A blank path asks for nothing, and MySQL has no CA setting.
    private mutating func enforceCAConstraint() {
        guard hasCA else { return }
        switch kind {
        case .postgres:
            if sslmode != "verify-ca", sslmode != "verify-full" { sslmode = "verify-full" }
            verifyTLS = true
        case .trino:
            setScheme(TrinoTransport.https.rawValue)
            verifyTLS = true
        case .mysql:
            break
        }
    }
}
