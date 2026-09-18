import Foundation
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

    /// What the connection's `database` field is called for this driver.
    var databaseLabel: String { self == .trino ? "Catalog" : "Database" }

    var hasSchemaLevel: Bool { self != .mysql }

    /// Postgres must have a database to connect to at all. MySQL and Trino can manage without.
    var requiresDatabase: Bool { self == .postgres }

    /// Drivers whose wire encryption is a mode rather than a scheme.
    var hasSSLModes: Bool { self != .trino }

    var sslModes: [String] {
        switch self {
        case .postgres: ["prefer", "disable", "require", "verify-ca", "verify-full"]
        case .mysql: ["disable", "require"]
        case .trino: []
        }
    }

    var defaultSSLMode: String { self == .postgres ? "prefer" : "disable" }
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
    /// Trino only: `http` or `https`. A port of 443/8443 or a stored password upgrades this to
    /// https in the engine, matching `TrinoConfig.__post_init__`.
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

    init(id: UUID, name: String, color: ConnectionColor, kind: ConnectionKind = .trino,
         host: String, port: Int, scheme: String = "https", sslmode: String = "",
         user: String, database: String, schema: String, verify: Bool,
         showAllSchemas: Bool = false) {
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
    }

    private enum CodingKeys: String, CodingKey {
        case id, name, color, kind, host, port, scheme, sslmode, user, database, schema, verify
        case showAllSchemas
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
        case .trino: scheme.uppercased() + (verify ? "" : " · unverified")
        case .postgres, .mysql: (sslmode.isEmpty ? kind.defaultSSLMode : sslmode)
        }
    }
}

/// Parses a connection URL into the fields of a Connection. Used by "Add from URL…", the
/// one-step way to get a connection without typing every field.
///
///     trino://user:secret@host:8443/hive/analytics
///     postgresql://user:secret@host:5432/mydb
///     mysql://user:secret@host:3306/mydb
///
/// The scheme picks the driver — the one thing a URL says that the individual fields cannot.
/// `http`/`https` are read as Trino, because that is what they meant before the other two existed.
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
    }

    static func parse(_ raw: String) -> Parsed? {
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
        return Parsed(
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

    static func directory() throws -> URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("QueryHive")
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
    static func load() -> (connections: [Connection], notice: Notice?) {
        guard let dir = try? directory() else {
            return ([], Notice(title: "Couldn't read connections",
                                message: "Couldn't create the QueryHive folder in Application Support."))
        }
        let url = dir.appendingPathComponent("connections.json")
        guard let data = try? Data(contentsOf: url) else { return ([], nil) }
        do {
            return (try JSONDecoder().decode([Connection].self, from: data), nil)
        } catch {
            let broken = dir.appendingPathComponent("connections.json.broken-\(Int(Date().timeIntervalSince1970))")
            do {
                try FileManager.default.moveItem(at: url, to: broken)
                return ([], Notice(title: "Couldn't read your saved connections",
                                    message: "connections.json didn't parse and was moved to \(broken.lastPathComponent). Starting with no connections; nothing was overwritten."))
            } catch let moveError {
                blockedURL = url
                return ([], Notice(title: "Couldn't read your saved connections",
                                    message: "connections.json didn't parse and couldn't be moved aside (\(moveError.localizedDescription)). Saving is disabled until \(url.lastPathComponent) is resolved by hand."))
            }
        }
    }

    static func save(_ connections: [Connection]) throws {
        if let blockedURL {
            throw StoreError(message: "connections.json is corrupt and couldn't be moved aside earlier; resolve \(blockedURL.path) before saving again.")
        }
        let url = try directory().appendingPathComponent("connections.json")
        let data = try JSONEncoder().encode(connections)
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

/// One generic-password Keychain item per connection, on the legacy login keychain.
/// The app is ad-hoc signed with no entitlements, so this deliberately avoids
/// kSecUseDataProtectionKeychain and access groups, which both need a real signing team.
enum ConnectionKeychain {
    private static let service = "id.data-ecosystem.queryhive"

    struct KeychainError: Error, LocalizedError {
        let status: OSStatus
        var errorDescription: String? {
            (SecCopyErrorMessageString(status, nil) as String?) ?? "Keychain error \(status)"
        }
    }

    private static func query(for id: UUID) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: id.uuidString]
    }

    static func set(_ credential: String, for id: UUID) throws {
        let search = query(for: id)
        let data = Data(credential.utf8)
        var attributes = search
        attributes[kSecValueData as String] = data
        let addStatus = SecItemAdd(attributes as CFDictionary, nil)
        if addStatus == errSecSuccess { return }
        guard addStatus == errSecDuplicateItem else { throw KeychainError(status: addStatus) }
        let updateStatus = SecItemUpdate(search as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        guard updateStatus == errSecSuccess else { throw KeychainError(status: updateStatus) }
    }

    static func get(for id: UUID) throws -> String? {
        var search = query(for: id)
        search[kSecReturnData as String] = true
        search[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw KeychainError(status: status) }
        return String(decoding: data, as: UTF8.self)
    }

    static func delete(for id: UUID) throws {
        let status = SecItemDelete(query(for: id) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}
