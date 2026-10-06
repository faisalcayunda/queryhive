import Foundation
import LocalAuthentication
import Security
import SwiftUI

/// The six dot colors a saved connection can pick, per the design contract.
enum ConnectionColor: String, CaseIterable, Identifiable, Codable {
    case gray, green, amber, red, blue, violet

    var id: Self { self }

    var color: Color {
        switch self {
        case .gray: Tone.gray
        case .green: Tone.mint
        case .amber: Tone.amber
        case .red: Tone.coral
        case .blue: Tone.blue
        case .violet: Tone.violet
        }
    }
}

/// The database a connection speaks to.
///
/// The driver is not a detail that can be hidden behind one set of fields: the three differ in
/// their default port, in how they quote an identifier, and in which object-tree levels they
/// even have. `levels` is the contract the tree builds itself from, and the engine has a matching
/// `db_drivers` command that reports the same shape.
enum ConnectionKind: String, CaseIterable, Identifiable, Codable {
    case trino, postgres, mysql

    var id: Self { self }

    var label: String {
        switch self {
        case .trino: "Trino"
        case .postgres: "PostgreSQL"
        case .mysql: "MySQL"
        }
    }

    var symbol: String {
        switch self {
        case .trino: "bolt.horizontal.circle"
        case .postgres: "cylinder.split.1x2"
        case .mysql: "cylinder"
        }
    }

    var defaultPort: Int {
        switch self {
        case .trino: 8080
        case .postgres: 5432
        case .mysql: 3306
        }
    }

    /// The object-tree levels this driver actually has, outermost first.
    ///
    /// Postgres cannot query across databases, so its database is fixed by the connection and
    /// never appears as a node. MySQL has no separate schema level — a schema *is* a database
    /// there — but it can query across them on one connection, so its databases do appear.
    var levels: [TreeNode.Kind] {
        switch self {
        case .trino: [.catalog, .schema, .table]
        case .postgres: [.schema, .table]
        case .mysql: [.database, .table]
        }
    }

    /// The levels the toolbar's context breadcrumb offers, outermost first — which is *not*
    /// `levels`, the object tree's contract.
    ///
    /// They differ in exactly one place, and deliberately. The tree for Postgres is schema-first
    /// because a connection cannot query across databases, so its database is not a level you
    /// browse. But it is a choice you make: every database on the server is reachable by pointing
    /// the query at it, so the breadcrumb offers one.
    var contextLevels: [TreeNode.Kind] {
        switch self {
        case .trino: [.catalog, .schema]
        case .postgres: [.database, .schema]
        case .mysql: [.database]
        }
    }

    /// What the connection's `database` field is called for this driver.
    var databaseLabel: String { self == .trino ? "Catalog" : "Database" }

    var hasSchemaLevel: Bool { self != .mysql }

    /// Postgres must have a database to connect to at all. MySQL and Trino can manage without.
    var requiresDatabase: Bool { self == .postgres }

    /// Drivers whose wire encryption is a mode rather than a scheme.
    var hasSSLModes: Bool { self != .trino }

    var sslModes: [String] {
        switch self {
        // libpq's vocabulary, and the order the picker shows: the default first.
        case .postgres: ["prefer", "disable", "require", "verify-ca", "verify-full"]
        // `prefer` was missing here, which made the MySQL driver's own default
        // unreachable from the UI: the engine encrypts when the server offers it,
        // but a connection made in the app could only say "never" or "always".
        //
        // The default stays `disable` rather than following the driver, and that is
        // parity rather than an oversight: pymysql connected without TLS unless it
        // was asked otherwise, so a connection made before this change behaves the
        // way it always did. `verify-ca` and `verify-full` are deliberately absent
        // too -- the MySQL client cannot reach the platform trust store, so on an
        // internal server with a private CA they could only ever refuse, and an
        // option whose only outcome is a failure is worse than no option.
        case .mysql: ["disable", "prefer", "require"]
        // Empty on purpose, and Trino is the one driver whose encryption is not a list of
        // mode words: its UI is a transport picker, which spells the same four outcomes as
        // `https`/`http` beside a verify flag, plus `prefer` as a third transport word.
        // Listing modes here as well would be a second control for one decision.
        case .trino: []
        }
    }

    var defaultSSLMode: String { self == .postgres ? "prefer" : "disable" }
}

/// What a connection refuses, as the engine's `SAFE_MODE` names it.
///
/// Four levels rather than a boolean, because both middle grounds are real: someone poking
/// at a production replica wants to run an `UPDATE` on one row without being able to `DROP`
/// the table, and someone who wants the engine to stop and ask before touching data does
/// not want it to stop and ask before a `SELECT`.
///
/// `confirm` is the engine's whole idea of a confirmation: a write runs only when the run
/// carries `SAFE_MODE_CONFIRMED=1`, a boolean only a caller that asked its user could have
/// set. Touch ID, a dialog and the password fallback are the app's business — the CLI and
/// the MCP server have no window to raise, so the engine cannot express them and does not
/// pretend to. The app asks through `RunConfirmation` and adds the flag to the one approved
/// run (`AppModel`), for the run paths the engine guards with a confirmation; the bulk paths
/// (`import_data`, `apply_changes`) are refused by the engine at this level on purpose, because
/// one approval cannot cover a whole plan (ADR-0026).
///
/// The engine is what enforces this — the CLI and the MCP server run the same guard — so
/// this enum is only how the level is chosen and stored. It reaches the engine as
/// `SAFE_MODE`, and the stored value is one of the engine's own words, so a connection
/// moved between the app and the command line means the same thing in both.
enum ConnectionSafeMode: String, CaseIterable, Identifiable, Codable {
    /// Run anything. The default, and what every connection made before this setting did.
    case full
    case noDDL = "no_ddl"
    /// Run a read; ask before a write; refuse DDL.
    case confirm
    case readOnly = "read_only"

    var id: Self { self }

    var title: String {
        switch self {
        case .full: "Full"
        case .noDDL: "No DDL"
        case .confirm: "Confirm"
        case .readOnly: "Read only"
        }
    }

    /// The one-line consequence, shown beside the picker so the level is a promise and
    /// not a name.
    var detail: String {
        switch self {
        case .full: "Anything runs, including DDL."
        case .noDDL: "Writes are allowed; CREATE, ALTER, DROP and the rest of DDL are refused."
        case .confirm:
            "Reads run; every write waits for an explicit confirmation, and DDL is refused."
        case .readOnly: "Only reads run. Every write and every DDL is refused."
        }
    }
}

/// What a connection is for, as a label: nothing in the app or the engine changes with it.
///
/// It exists so the person writing a query can see, in the tab, the breadcrumb and the status bar,
/// whether this is the production server. It never reaches the engine and never changes Safe
/// Mode; that is `ConnectionSafeMode`'s job.
enum ConnectionEnvironment: String, CaseIterable, Codable, Identifiable {
    case dev, staging, prod

    var id: Self { self }

    var title: String {
        switch self {
        case .dev: "Dev"
        case .staging: "Staging"
        case .prod: "Prod"
        }
    }

    /// The word VoiceOver reads after "Environment:".
    var spoken: String {
        switch self {
        case .dev: "development"
        case .staging: "staging"
        case .prod: "production"
        }
    }
}

/// Trino's transport, as its picker spells it: the two scheme words, plus the one outcome
/// neither of them can express.
///
/// `prefer` is not a new word. It is the shared vocabulary's own — the Postgres and MySQL
/// pickers already offer it for exactly this outcome, "encrypt when the server offers it" —
/// and the engine reads it for Trino too, as `DB_SSLMODE`. What it is not is a *scheme*:
/// `prefer` starts on `https` and keeps plain `http` in reserve for the one coordinator that
/// answers the TLS handshake with something that is not TLS at all. So the app stores the
/// word in the connection's scheme slot (the JSON key stays `scheme`, and every value written
/// before this existed is one of the two schemes and still means what it did) and translates
/// it to `DB_SCHEME` + `DB_SSLMODE` + `DB_INSECURE` on the way out, in
/// `AppModel.connectionEnvironment`.
///
/// The four outcomes, and where each one is reachable from — there is no fifth:
///
/// | Transport | `DB_SCHEME` | `DB_SSLMODE` | `DB_INSECURE` | Engine mode |
/// |---|---|---|---|---|
/// | `https` | `https` | — | — | `Require` |
/// | `https` + verify off | `https` | — | `1` | `RequireNoVerify` |
/// | `http` | `http` | — | — | `Disable` |
/// | `prefer` | `https` | `prefer` | — | `Prefer` |
enum TrinoTransport: String, CaseIterable, Identifiable {
    case https, http, prefer

    var id: Self { self }

    /// What the segmented control shows, in the same upper case as the other two options.
    var label: String { rawValue.uppercased() }

    /// Reads the stored word. Anything unrecognised — a blank included, which the engine has
    /// always read as plain `http` — lands on `http`: guessing `https` for a value nobody can
    /// explain would silently encrypt a connection that used to be in clear.
    init(stored word: String) {
        switch word.lowercased() {
        case "https": self = .https
        case "prefer": self = .prefer
        default: self = .http
        }
    }
}

/// Whether the app is talking to a server, as the status bar reports it.
///
/// Three states rather than a boolean, because "still trying" is the one a user actually needs:
/// a connection with no children yet is not *broken*, and showing it as disconnected would report
/// a failure that has not happened.
enum ConnectionState {
    case connected
    case connecting
    /// Never expanded, so nothing has been asked of it yet. Distinct from `disconnected`, which
    /// means a browse was attempted and failed — a connection the user has simply not opened is not
    /// broken, and calling it disconnected announces a failure that has not happened.
    case idle
    case disconnected

    var label: String {
        switch self {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .idle: "Not browsed yet"
        case .disconnected: "Disconnected"
        }
    }
}

/// How the SSH tunnel proves who the user is to the bastion. The words are the engine's own
/// `SSH_AUTH_METHOD` values, so a stored connection reads the same on both sides of the bridge.
enum SSHAuthMethod: String, CaseIterable, Identifiable, Codable {
    /// Whatever `SSH_AUTH_SOCK` offers: the default, and the only method with no secret.
    case agent
    /// A private key file, with a passphrase when it has one.
    case key
    case password

    var id: Self { self }

    var title: String {
        switch self {
        case .agent: "Agent"
        case .key: "Key file"
        case .password: "Password"
        }
    }
}

/// How a Trino connection logs in. `jwt` sends a bearer token and never a password; the other two
/// drivers have no such thing, so the setting is read for Trino only (the engine refuses a token
/// anywhere else, and `AppModel.connectionEnvironment` does not send one).
enum DatabaseAuth: String, CaseIterable, Identifiable, Codable {
    case password
    case jwt

    var id: Self { self }

    var title: String { self == .jwt ? "JWT" : "Password" }
}

/// A saved database, Navicat style. The password never lives here: it is stored separately in the
/// Keychain, keyed by `id`, and a connection with no stored password simply connects without one.
struct Connection: Identifiable, Codable, Equatable {
    var id: UUID
    var name: String
    var color: ConnectionColor
    var kind: ConnectionKind
    var host: String
    var port: Int
    /// Trino only: the transport word the editor's picker offers — `https`, `http`, or `prefer`
    /// (see `TrinoTransport`, which is how the third one reaches the engine). The key stays
    /// `scheme` on disk, and the two scheme values mean exactly what they always have.
    /// A port of 443/8443 or a stored password upgrades a clear connection to https in the
    /// engine, matching `TrinoConfig.__post_init__`.
    var scheme: String
    /// Postgres and MySQL only: the wire encryption mode.
    var sslmode: String
    var user: String
    /// Trino calls this a catalog and the other two a database. It is one slot on the wire
    /// (`DB_DATABASE`), because it means the same thing: which database to open.
    var database: String
    var schema: String
    /// Trino only. Postgres expresses certificate checking through `sslmode`.
    var verify: Bool
    /// Postgres only: list system schemas (`pg_catalog`, `information_schema`, anything `pg_*`)
    /// in the object tree too, instead of hiding them. Meaningful only where a schema level exists
    /// and the engine filters it.
    var showAllSchemas: Bool
    /// Postgres only: draw a database level under the connection and list every database this user
    /// may connect to on the server (`pg_database`, templates and `datallowconn = false` aside),
    /// instead of starting at the schemas of the one database the connection names.
    ///
    /// Off is the old tree and the common case. On is for a server where "which database is this
    /// table in" is the question, and expanding one lists *its* schemas — a fresh connection per
    /// listing, because Postgres cannot read another database's catalogs from this one.
    var showAllDatabases: Bool
    /// The group this connection is filed under in the sidebar, or nil for the top level.
    ///
    /// A group **id**, not its name: renaming a group must not orphan the connections in it, and a
    /// name is not an identity — two groups could be called "Production" and the file would have no
    /// way to say which one a connection meant. Decoded with `decodeIfPresent`, so a
    /// connections.json written before groups existed loads with everything at the top level.
    var group: UUID?
    /// What this connection refuses, enforced by the engine. Stored as the engine's own
    /// `SAFE_MODE` word, so a connection moved between the app and the command line means
    /// the same thing in both. Decoded with `decodeIfPresent`, so a file written before
    /// this existed loads as `full` — the behaviour it had.
    var safeMode: ConnectionSafeMode
    /// A label only: dev, staging or prod, or none. Decoded softly (a missing key, or a word a
    /// later build wrote, is `nil` and does not make the file unreadable), and never sent to the
    /// engine.
    var environment: ConnectionEnvironment?

    // The W11 fields (blueprint section 9.1). All flat, all `decodeIfPresent`, and every default is
    // what a connection did before they existed, so a connections.json from an earlier build loads
    // as it was and is never moved aside as corrupt. No secret is ever stored here: the bastion's
    // password, a key's passphrase and a JWT live in the Keychain (`ConnectionKeychain.Slot`).

    /// The bastion, by name or by `~/.ssh/config` alias (`sshUseConfig`). Empty means no tunnel.
    var sshHost: String
    /// `sshHost` is a `Host` alias in `~/.ssh/config`, and the engine reads `HostName`, `User`,
    /// `Port` and `IdentityFile` from there for whatever the fields below leave blank.
    var sshUseConfig: Bool
    /// `0` means "not set": 22, or the alias's own port.
    var sshPort: Int
    var sshUser: String
    var sshAuth: SSHAuthMethod
    var sshKeyPath: String
    /// Trino only.
    var dbAuth: DatabaseAuth
    /// A CA bundle this connection trusts instead of the platform store (PostgreSQL and Trino).
    var caFile: String
    /// Milliseconds a statement may run on this connection. `nil` inherits the app-wide setting,
    /// which is not the same as `0` ("no bound"): it keeps following that setting when it changes.
    var statementTimeoutMS: Int?

    /// Whether a run on this connection goes through an SSH bastion.
    var usesTunnel: Bool { !sshHost.trimmingCharacters(in: .whitespaces).isEmpty }

    init(id: UUID, name: String, color: ConnectionColor, kind: ConnectionKind = .trino,
         host: String, port: Int, scheme: String = "https", sslmode: String = "",
         user: String, database: String, schema: String, verify: Bool,
         showAllSchemas: Bool = false, showAllDatabases: Bool = false, group: UUID? = nil,
         safeMode: ConnectionSafeMode = .full, environment: ConnectionEnvironment? = nil,
         sshHost: String = "", sshUseConfig: Bool = false, sshPort: Int = 0, sshUser: String = "",
         sshAuth: SSHAuthMethod = .agent, sshKeyPath: String = "", dbAuth: DatabaseAuth = .password,
         caFile: String = "", statementTimeoutMS: Int? = nil) {
        self.id = id
        self.name = name
        self.color = color
        self.kind = kind
        self.host = host
        self.port = port
        self.scheme = scheme
        self.sslmode = sslmode
        self.user = user
        self.database = database
        self.schema = schema
        self.verify = verify
        self.showAllSchemas = showAllSchemas
        self.showAllDatabases = showAllDatabases
        self.group = group
        self.safeMode = safeMode
        self.environment = environment
        self.sshHost = sshHost
        self.sshUseConfig = sshUseConfig
        self.sshPort = sshPort
        self.sshUser = sshUser
        self.sshAuth = sshAuth
        self.sshKeyPath = sshKeyPath
        self.dbAuth = dbAuth
        self.caFile = caFile
        self.statementTimeoutMS = statementTimeoutMS
    }

    private enum CodingKeys: String, CodingKey {
        case id, name, color, kind, host, port, scheme, sslmode, user, database, schema, verify
        case showAllSchemas, showAllDatabases, group, safeMode, environment
        case sshHost, sshUseConfig, sshPort, sshUser, sshAuth, sshKeyPath, dbAuth, caFile
        case statementTimeoutMS
    }

    /// The names this file used before QueryHive spoke to more than Trino. Read and never
    /// written, so a connections.json saved by an earlier build loads instead of being moved
    /// aside as corrupt.
    private enum LegacyKeys: String, CodingKey {
        case httpScheme, catalog
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let legacy = try decoder.container(keyedBy: LegacyKeys.self)
        id = try container.decode(UUID.self, forKey: .id)
        name = try container.decode(String.self, forKey: .name)
        color = try container.decodeIfPresent(ConnectionColor.self, forKey: .color) ?? .blue
        kind = try container.decodeIfPresent(ConnectionKind.self, forKey: .kind) ?? .trino
        host = try container.decodeIfPresent(String.self, forKey: .host) ?? ""
        port = try container.decodeIfPresent(Int.self, forKey: .port) ?? kind.defaultPort
        scheme = try container.decodeIfPresent(String.self, forKey: .scheme)
            ?? legacy.decodeIfPresent(String.self, forKey: .httpScheme)
            ?? "https"
        sslmode = try container.decodeIfPresent(String.self, forKey: .sslmode) ?? ""
        user = try container.decodeIfPresent(String.self, forKey: .user) ?? ""
        database = try container.decodeIfPresent(String.self, forKey: .database)
            ?? legacy.decodeIfPresent(String.self, forKey: .catalog)
            ?? ""
        schema = try container.decodeIfPresent(String.self, forKey: .schema) ?? ""
        verify = try container.decodeIfPresent(Bool.self, forKey: .verify) ?? true
        // Absent on a connections.json written before "Show all schemas" existed: stay hidden.
        showAllSchemas = try container.decodeIfPresent(Bool.self, forKey: .showAllSchemas) ?? false
        showAllDatabases = try container.decodeIfPresent(Bool.self, forKey: .showAllDatabases) ?? false
        // Absent on a file written before groups existed: everything sits at the top level.
        group = try container.decodeIfPresent(UUID.self, forKey: .group)
        // Absent on a file written before Safe Mode existed: `full`, which is what that
        // connection was already doing.
        safeMode = try container.decodeIfPresent(ConnectionSafeMode.self, forKey: .safeMode) ?? .full
        // `try?`: a value this build does not know must not move the whole file aside as corrupt.
        let word = try? container.decodeIfPresent(String.self, forKey: .environment)
        environment = word.flatMap { ConnectionEnvironment(rawValue: $0) }
        sshHost = try container.decodeIfPresent(String.self, forKey: .sshHost) ?? ""
        sshUseConfig = try container.decodeIfPresent(Bool.self, forKey: .sshUseConfig) ?? false
        sshPort = try container.decodeIfPresent(Int.self, forKey: .sshPort) ?? 0
        sshUser = try container.decodeIfPresent(String.self, forKey: .sshUser) ?? ""
        // `try?` for the two words, like the tag: one a later build wrote is the default here, not a
        // reason to move the whole file aside.
        sshAuth = (try? container.decodeIfPresent(SSHAuthMethod.self, forKey: .sshAuth)) ?? .agent
        sshKeyPath = try container.decodeIfPresent(String.self, forKey: .sshKeyPath) ?? ""
        dbAuth = (try? container.decodeIfPresent(DatabaseAuth.self, forKey: .dbAuth)) ?? .password
        caFile = try container.decodeIfPresent(String.self, forKey: .caFile) ?? ""
        statementTimeoutMS = try? container.decodeIfPresent(Int.self, forKey: .statementTimeoutMS)
    }

    /// One-line identity for the sidebar and the picker: `host:port/database.schema`.
    var displayTarget: String {
        var text = "\(host):\(port)"
        if !database.isEmpty {
            text += "/\(database)"
            if hasSchema, !schema.isEmpty { text += ".\(schema)" }
        }
        return text
    }

    /// Whether the schema is part of this connection's identity. Postgres's `schema` is really
    /// `search_path`, and MySQL has none, so neither belongs in the connection's one-line name.
    private var hasSchema: Bool { kind == .trino }

    /// The driver-specific encryption setting, for the connection list and the editor.
    var securityLabel: String {
        switch kind {
        case .trino:
            switch TrinoTransport(stored: scheme) {
            case .https: "HTTPS" + (verify ? "" : " · unverified")
            // Nothing is encrypted, so there is no verification answer to add — and `verify`
            // is not one either, whichever way it is stored.
            case .http: "HTTP"
            // `prefer` never checks the certificate, so the stored flag is not reported here:
            // it cannot apply to this transport.
            case .prefer: "prefer · unverified"
            }
        case .postgres, .mysql: (sslmode.isEmpty ? kind.defaultSSLMode : sslmode)
        }
    }
}

/// Parses a connection URL into the fields of a Connection. Used by "Add from URL…", the
/// one-step way to get a connection without typing every field.
///
///     trino://user:secret@host:8443/hive/analytics
///     postgresql://user:secret@host:5432/mydb?sslmode=verify-full
///     mysql://user:secret@host:3306/mydb?ssl-mode=REQUIRED
///
/// The scheme picks the driver — the one thing a URL says that the individual fields cannot.
/// `http`/`https` are read as Trino, because that is what they meant before the other two existed.
///
/// **TLS is never lowered by an import** (DBX-29). A URL that names an encryption mode is read
/// for it: PostgreSQL's libpq words are kept verbatim (`verify-full` stays `verify-full`, it is
/// never folded into `require`, which does not check the certificate), MySQL's `ssl-mode` and
/// `useSSL` map to `disable`/`prefer`/`require`, and a word that means something this app cannot
/// do — MySQL's `VERIFY_CA`/`VERIFY_IDENTITY`, libpq's `allow`, anything unknown — is refused *by
/// name* so the person knows what to fix. The old behaviour dropped the parameter and left the
/// form on its default, so a URL that said "verify the certificate" became a connection that did
/// not, with no notice. `host`, `hostaddr` and `port` parameters are ignored: they would override
/// the authority the person can see, and the form is where the host is chosen.
enum ConnectionURL {
    struct Parsed {
        var kind: ConnectionKind
        var host: String
        var port: Int
        var scheme: String
        var user: String
        var password: String?
        var database: String
        var schema: String
        /// Postgres and MySQL: the mode the URL named, in this app's words. `nil` when it named none.
        var sslmode: String?
        /// Trino: whether the URL asked for the certificate to be checked. `nil` when it did not say.
        var verify: Bool?
        /// Parameters that were read and deliberately not used, by name, for the form to report.
        var ignored: [String] = []
    }

    /// Why a URL cannot be imported as written. The message is for the person: it names the
    /// parameter and the value, and what would work.
    struct Failure: LocalizedError, Equatable {
        let message: String
        var errorDescription: String? { message }
    }

    /// `nil` for a URL that is not a connection URL at all; a URL that is one but says something
    /// this app cannot honour throws a `Failure` naming it.
    static func parse(_ raw: String) throws -> Parsed? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        // A bare `host:5432/db` would be read as the scheme `host`, so only an explicit `://`
        // counts as one; anything else is assumed to be a Trino coordinator over http.
        let text = trimmed.contains("://") ? trimmed : "http://" + trimmed
        guard let parts = URLComponents(string: text), let host = parts.host, !host.isEmpty else { return nil }

        let rawScheme = (parts.scheme ?? "trino").lowercased()
        let kind: ConnectionKind
        switch rawScheme {
        case "postgres", "postgresql": kind = .postgres
        case "mysql": kind = .mysql
        default: kind = .trino
        }

        let path = parts.path.split(separator: "/").map(String.init)
        var parsed = Parsed(
            kind: kind,
            host: host,
            port: parts.port ?? kind.defaultPort,
            scheme: kind == .trino ? (rawScheme == "https" ? "https" : "http") : "",
            user: parts.user?.removingPercentEncoding ?? "",
            password: parts.password?.removingPercentEncoding,
            database: path.first ?? "",
            // Only Trino has a second path segment that means anything; for the others it is a
            // database with a slash in its name, which is not a thing.
            schema: kind == .trino && path.count > 1 ? path[1] : ""
        )
        try applyQuery(parts.queryItems ?? [], to: &parsed)
        return parsed
    }

    private static let libpqModes = ["disable", "prefer", "require", "verify-ca", "verify-full"]

    private static func applyQuery(_ items: [URLQueryItem], to parsed: inout Parsed) throws {
        for item in items {
            let name = item.name.lowercased()
            let value = (item.value ?? "").trimmingCharacters(in: .whitespaces)
            switch name {
            case "host", "hostaddr", "port":
                parsed.ignored.append(item.name)
            case "sslmode", "ssl-mode", "ssl_mode":
                try applyMode(named: item.name, value: value, to: &parsed)
            case "usessl":
                guard parsed.kind == .mysql else { parsed.ignored.append(item.name); continue }
                switch value.lowercased() {
                case "true": parsed.sslmode = "require"
                case "false": parsed.sslmode = "disable"
                default:
                    throw Failure(message: "\(item.name)=\(value) isn't true or false, so the TLS setting can't be imported.")
                }
            default:
                parsed.ignored.append(item.name)
            }
        }
    }

    private static func applyMode(named name: String, value: String, to parsed: inout Parsed) throws {
        let word = value.lowercased()
        switch parsed.kind {
        case .postgres:
            guard libpqModes.contains(word) else {
                throw Failure(message: "\(name)=\(value) isn't a PostgreSQL SSL mode this app supports. "
                              + "Use \(libpqModes.joined(separator: ", ")).")
            }
            parsed.sslmode = word
        case .mysql:
            switch word.uppercased() {
            case "DISABLED", "DISABLE": parsed.sslmode = "disable"
            case "PREFERRED", "PREFER": parsed.sslmode = "prefer"
            case "REQUIRED", "REQUIRE": parsed.sslmode = "require"
            case "VERIFY_CA", "VERIFY_IDENTITY", "VERIFY-CA", "VERIFY-FULL", "VERIFY_FULL":
                throw Failure(message: "\(name)=\(value) asks for the certificate to be verified, which the MySQL "
                              + "driver can't do here. Use DISABLED, PREFERRED or REQUIRED, or set the mode by hand.")
            default:
                throw Failure(message: "\(name)=\(value) isn't a MySQL SSL mode this app knows. "
                              + "Use DISABLED, PREFERRED or REQUIRED.")
            }
        case .trino:
            // The engine reads `sslmode` for Trino too; the app's own picker spells it as a
            // transport and a verify flag, so the word is translated rather than stored.
            switch word {
            case "disable": parsed.scheme = "http"
            case "prefer": parsed.scheme = "prefer"
            case "require": parsed.scheme = "https"; parsed.verify = false
            case "verify-ca", "verify-full": parsed.scheme = "https"; parsed.verify = true
            default:
                throw Failure(message: "\(name)=\(value) isn't an SSL mode this app knows. "
                              + "Use \(libpqModes.joined(separator: ", ")).")
            }
        }
    }
}

/// A folder in the sidebar that connections can be gathered into.
///
/// A group is a name and an identity, nothing else: it holds connections and does not nest. One
/// level is what "keep these together" needs, and nesting would bring a tree of folders to manage
/// plus a question with no good answer — what does a folder inside a folder mean to the catalogs
/// and schemas underneath?
struct ConnectionGroup: Identifiable, Codable, Equatable {
    var id: UUID
    var name: String

    init(id: UUID = UUID(), name: String) {
        self.id = id
        self.name = name
    }
}

/// What `connections.json` holds: the connections, and the groups they can be filed under.
///
/// An envelope rather than a bare array, because a group has to outlive the connections in it.
/// "New Group" on a side bar with nothing in it must still be there after a relaunch, and it cannot
/// be if groups are inferred from the connections that point at them — an empty group would leave
/// no trace to infer from.
///
/// The decoder still accepts the bare array this file used to be, so an existing connections.json
/// loads rather than being moved aside as corrupt. That is the same promise `Connection`'s legacy
/// keys keep.
struct ConnectionsDocument: Codable, Equatable {
    var groups: [ConnectionGroup]
    var connections: [Connection]

    init(groups: [ConnectionGroup] = [], connections: [Connection] = []) {
        self.groups = groups
        self.connections = connections
    }

    private enum CodingKeys: String, CodingKey {
        case groups, connections
    }

    init(from decoder: Decoder) throws {
        // The envelope, if this is one. `container(keyedBy:)` throws on an array, which is exactly
        // the signal to fall through to the form this file had before groups existed.
        if let keyed = try? decoder.container(keyedBy: CodingKeys.self), keyed.contains(.connections) {
            groups = try keyed.decodeIfPresent([ConnectionGroup].self, forKey: .groups) ?? []
            connections = try keyed.decode([Connection].self, forKey: .connections)
            return
        }
        groups = []
        connections = try decoder.singleValueContainer().decode([Connection].self)
    }
}

/// QueryHive's own connection list as a file you can hand to another Mac (DBX-74).
///
/// **No secret is in it, by construction**: `Connection` has no slot for one (the four Keychain
/// items are keyed by `id` and never leave the Keychain), so the export is the same record
/// `connections.json` already holds, wrapped in a header that says what it is. Ids are not
/// carried over on import: a connection gets a fresh one, so it can never alias a Keychain item
/// of the machine it came from. The Swift side owns this: it needs no engine round trip, and
/// `qh-storage`'s `import_connections` stays the engine's own (W11-T3r).
enum ConnectionListTransfer {
    static let format = "queryhive-connections"
    static let version = 1

    struct Failure: LocalizedError, Equatable {
        let message: String
        var errorDescription: String? { message }
    }

    private struct Header: Decodable {
        var format: String?
        var version: Int?
    }

    private struct Envelope: Encodable {
        var format: String
        var version: Int
        var groups: [ConnectionGroup]
        var connections: [Connection]
    }

    static func export(groups: [ConnectionGroup], connections: [Connection]) throws -> Data {
        // Only the groups a connection is filed under travel, so an unused folder is not exported.
        let used = Set(connections.compactMap(\.group))
        let envelope = Envelope(format: format, version: version,
                                groups: groups.filter { used.contains($0.id) }, connections: connections)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return try encoder.encode(envelope)
    }

    static func read(_ data: Data) throws -> ConnectionsDocument {
        guard let header = try? JSONDecoder().decode(Header.self, from: data), header.format == format else {
            throw Failure(message: "That isn't a QueryHive connection list.")
        }
        guard let found = header.version, found >= 1, found <= version else {
            throw Failure(message: "This list was written by a newer QueryHive (version \(header.version.map(String.init) ?? "unknown")); update the app to read it.")
        }
        do {
            return try JSONDecoder().decode(ConnectionsDocument.self, from: data)
        } catch {
            throw Failure(message: "The connection list is damaged: \(error.localizedDescription)")
        }
    }
}

/// JSON array of connections in Application Support. No secrets in this file.
enum ConnectionStore {
    struct StoreError: Error, LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Set when a corrupt connections.json couldn't even be moved aside during `load()`: `save`
    /// refuses from then on, so a retry can never silently overwrite the file we couldn't
    /// preserve for inspection.
    private static var blockedURL: URL?

    /// Where connections.json lives, when something other than the app is asking — the test suite.
    static var root: URL?

    /// True while this process is a test run.
    ///
    /// The suite constructs `AppModel`, which **reads** this store, and then calls actions that
    /// **write** it. Pointed at the real file that is a data loss waiting to happen, and it has
    /// already happened once: a test that filed a fixture connection into a fixture group wrote the
    /// fixture over a real `connections.json`, and the app keeps no backup of that file.
    ///
    /// Checked here rather than left to each test to remember, because "remember to redirect the
    /// store" is exactly the kind of instruction a new test fails to follow. A test that wants its
    /// own directory sets `root`; one that says nothing gets a private temporary one.
    private static var isTesting: Bool {
        NSClassFromString("XCTestCase") != nil
    }

    static func directory() throws -> URL {
        if let root {
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            return root
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        // Per-process, so two test runs cannot see each other's connections — a suite that read the
        // previous run's fixtures would pass for the wrong reason.
        let dir = isTesting
            ? FileManager.default.temporaryDirectory
                .appendingPathComponent("QueryHive-tests-\(ProcessInfo.processInfo.processIdentifier)")
            : base.appendingPathComponent("QueryHive")
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        } catch {
            throw StoreError(message: "Couldn't create \(dir.path): \(error.localizedDescription)")
        }
        return dir
    }

    /// Missing file reads back as no connections. A file that fails to decode is renamed aside
    /// (so a later save can never overwrite it) and reported back as a notice; the caller
    /// starts from an empty list rather than guessing at recovery.
    static func load() -> (document: ConnectionsDocument, notice: Notice?) {
        guard let dir = try? directory() else {
            return (ConnectionsDocument(), Notice(title: "Couldn't read connections",
                                message: "Couldn't create the QueryHive folder in Application Support."))
        }
        let url = dir.appendingPathComponent("connections.json")
        guard let data = try? Data(contentsOf: url) else { return (ConnectionsDocument(), nil) }
        do {
            return (try JSONDecoder().decode(ConnectionsDocument.self, from: data), nil)
        } catch {
            let broken = dir.appendingPathComponent("connections.json.broken-\(Int(Date().timeIntervalSince1970))")
            do {
                try FileManager.default.moveItem(at: url, to: broken)
                return (ConnectionsDocument(),
                        Notice(title: "Couldn't read your saved connections",
                               message: "connections.json didn't parse and was moved to \(broken.lastPathComponent). Starting with no connections; nothing was overwritten."))
            } catch let moveError {
                blockedURL = url
                return (ConnectionsDocument(),
                        Notice(title: "Couldn't read your saved connections",
                               message: "connections.json didn't parse and couldn't be moved aside (\(moveError.localizedDescription)). Saving is disabled until \(url.lastPathComponent) is resolved by hand."))
            }
        }
    }

    static func save(_ document: ConnectionsDocument) throws {
        if let blockedURL {
            throw StoreError(message: "connections.json is corrupt and couldn't be moved aside earlier; resolve \(blockedURL.path) before saving again.")
        }
        let url = try directory().appendingPathComponent("connections.json")
        // One save behind, kept beside the file. This is here because the file was destroyed once
        // and there was nothing to restore it from: no Time Machine snapshot, nothing in the trash,
        // and the app itself keeps no history. A single previous copy turns "the connections file
        // was overwritten" from a loss into an inconvenience.
        //
        // Best effort on purpose: a failure to copy must not block the save the user asked for.
        let previous = url.appendingPathExtension("bak")
        if FileManager.default.fileExists(atPath: url.path) {
            try? FileManager.default.removeItem(at: previous)
            try? FileManager.default.copyItem(at: url, to: previous)
        }
        let data = try JSONEncoder().encode(document)
        try data.write(to: url, options: .atomic)
    }

    /// Directory the bundled engine runs in: the app's own Application Support folder, so an
    /// export started from the app never drops files into whatever the user launched it from.
    static var runDirectory: URL {
        (try? directory()) ?? FileManager.default.temporaryDirectory
    }

    /// Default place an export lands, matching the web UI's `/api/defaults`.
    static var defaultOutputDirectory: URL {
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        if let downloads, FileManager.default.fileExists(atPath: downloads.path) { return downloads }
        return FileManager.default.homeDirectoryForCurrentUser
    }
}

/// The Keychain items a connection owns, on the legacy login keychain: the database password
/// (the one that has always existed, under the bare UUID) and three more, each under a prefixed
/// account (blueprint W11 section 8). The prefixes are a contract with the engine
/// (`qh_credentials::account_for`): the two sides must produce the same string byte for byte, and
/// `ConnectionKeychainTests` and the Rust test pin the same literals.
///
/// The app is ad-hoc signed with no entitlements, so this deliberately avoids
/// kSecUseDataProtectionKeychain and access groups, which both need a real signing team.
enum ConnectionKeychain {
    private static let service = "id.data-ecosystem.queryhive"

    /// What a connection can keep in the Keychain. A secret is only ever here, never in
    /// `connections.json`, never in an environment the app logs.
    enum Slot: CaseIterable {
        case database, sshPassword, sshPassphrase, jwt

        fileprivate var prefix: String {
            switch self {
            case .database: ""
            case .sshPassword: "ssh-password:"
            case .sshPassphrase: "ssh-passphrase:"
            case .jwt: "jwt:"
            }
        }
    }

    struct KeychainError: Error, LocalizedError {
        let status: OSStatus
        var errorDescription: String? {
            (SecCopyErrorMessageString(status, nil) as String?) ?? "Keychain error \(status)"
        }
    }

    /// The account string of one slot of one connection. `uuidString` is upper case, which is
    /// the spelling the engine folds a lower-case UUID to.
    static func account(_ slot: Slot, for id: UUID) -> String { slot.prefix + id.uuidString }

    private static func query(_ slot: Slot, for id: UUID) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account(slot, for: id)]
    }

    static func set(_ credential: String, slot: Slot, for id: UUID) throws {
        let search = query(slot, for: id)
        let data = Data(credential.utf8)
        var attributes = search
        attributes[kSecValueData as String] = data
        let addStatus = SecItemAdd(attributes as CFDictionary, nil)
        if addStatus == errSecSuccess { return }
        guard addStatus == errSecDuplicateItem else { throw KeychainError(status: addStatus) }
        let updateStatus = SecItemUpdate(search as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        guard updateStatus == errSecSuccess else { throw KeychainError(status: updateStatus) }
    }

    static func get(slot: Slot, for id: UUID) throws -> String? {
        var search = query(slot, for: id)
        search[kSecReturnData as String] = true
        search[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw KeychainError(status: status) }
        return String(decoding: data, as: UTF8.self)
    }

    /// `get`, but never shows a prompt: an item that would need the user's approval is reported as
    /// not available. For background work (warming a session) that must not put a dialog up.
    static func getWithoutPrompt(slot: Slot, for id: UUID) throws -> String? {
        var search = query(slot, for: id)
        search[kSecReturnData as String] = true
        search[kSecMatchLimit as String] = kSecMatchLimitOne
        // `kSecUseAuthenticationUIFail`'s replacement (macOS 11): a context that may not show UI.
        let context = LAContext()
        context.interactionNotAllowed = true
        search[kSecUseAuthenticationContext as String] = context
        var result: AnyObject?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw KeychainError(status: status) }
        return String(decoding: data, as: UTF8.self)
    }

    /// Whether an item is saved, from its attributes only: the secret is never returned, so this
    /// can be asked on every render of the form without a prompt.
    static func contains(slot: Slot, for id: UUID) -> Bool {
        var search = query(slot, for: id)
        search[kSecMatchLimit as String] = kSecMatchLimitOne
        let context = LAContext()
        context.interactionNotAllowed = true
        search[kSecUseAuthenticationContext as String] = context
        let status = SecItemCopyMatching(search as CFDictionary, nil)
        // An item that exists but would need approval to read still exists.
        return status == errSecSuccess || status == errSecInteractionNotAllowed
    }

    static func delete(slot: Slot, for id: UUID) throws {
        let status = SecItemDelete(query(slot, for: id) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }

    /// Removes every slot of a connection. Each is tried, and the first failure is thrown after the
    /// rest have been: one stuck item must not leave the other three behind.
    static func deleteAll(for id: UUID) throws {
        var first: Error?
        for slot in Slot.allCases {
            do { try delete(slot: slot, for: id) } catch { first = first ?? error }
        }
        if let first { throw first }
    }

    /// The delete in `renew` went through and neither write-back did: the secret is gone from the
    /// Keychain and the person has to enter it again. Nothing else `renew` throws means that.
    struct SecretLost: Error, LocalizedError {
        let slot: Slot
        let underlying: Error
        var errorDescription: String? { "The saved secret could not be written back: \(underlying.localizedDescription)" }
    }

    /// The notice text for secrets `SecretLost` reported, one sentence a person can act on.
    static func lostMessage(connection: String, secrets: [String]) -> String {
        "Couldn't write the saved \(secrets.joined(separator: ", ")) for \(connection) back to Keychain after renewing it. Enter it again in the connection form."
    }

    /// Re-writes an item that an older build left with an access list that still prompts (C-6).
    ///
    /// Only an item `getWithoutPrompt` says would need approval is touched, by `renew`. Returns
    /// whether an item was rewritten.
    @discardableResult
    static func renewIfPrompting(slot: Slot, for id: UUID) throws -> Bool {
        do {
            _ = try getWithoutPrompt(slot: slot, for: id)
            return false
        } catch let error as KeychainError where error.status == errSecInteractionNotAllowed {
            return try renew(slot: slot, for: id, in: SystemKeychain())
        }
    }

    /// Reads the item (the one prompt, from a user who is saving the connection and expects
    /// dialogs), deletes it and writes it back, so this build owns it. If the write-back fails the
    /// value is written once more, and then `SecretLost` is thrown. A failure before the delete
    /// throws as it is: the item is still where it was.
    static func renew(slot: Slot, for id: UUID, in store: SecretStoring) throws -> Bool {
        guard let value = try store.get(slot: slot, for: id) else { return false }
        try store.delete(slot: slot, for: id)
        do {
            try store.set(value, slot: slot, for: id)
        } catch {
            do {
                try store.set(value, slot: slot, for: id)
            } catch {
                throw SecretLost(slot: slot, underlying: error)
            }
        }
        return true
    }

    // The database slot under the names every earlier caller uses.
    static func set(_ credential: String, for id: UUID) throws { try set(credential, slot: .database, for: id) }
    static func get(for id: UUID) throws -> String? { try get(slot: .database, for: id) }
    static func getWithoutPrompt(for id: UUID) throws -> String? { try getWithoutPrompt(slot: .database, for: id) }
    static func delete(for id: UUID) throws { try delete(slot: .database, for: id) }
}

/// Where a connection's secrets live, as the app uses them. The real answer is the Keychain
/// (`SystemKeychain`); a test hands in a dictionary, because writing to a login keychain during an
/// ordinary `swift test` is not a thing a test suite does unasked.
protocol SecretStoring {
    func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool
    func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String?
    func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws
    func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws
}

struct SystemKeychain: SecretStoring {
    func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool {
        ConnectionKeychain.contains(slot: slot, for: id)
    }
    func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
        try ConnectionKeychain.get(slot: slot, for: id)
    }
    func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {
        try ConnectionKeychain.set(secret, slot: slot, for: id)
    }
    func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws {
        try ConnectionKeychain.delete(slot: slot, for: id)
    }
}
