import CommonCrypto
import Foundation

/// What one importer hands the model: the connections it could read, the entries it had to leave
/// out (named, with the reason), and file-level lines for the summary (a credentials file that was
/// missing, a Keychain read that was cancelled). Shared by the DBeaver and DataGrip importers.
struct ImportBatch {
    var connections: [ImportedConnection] = []
    var skipped: [NavicatImport.Skipped] = []
    var notes: [String] = []
}

/// Reads DBeaver's `data-sources.json`, and the `credentials-config.json` next to it.
///
/// DBeaver keeps the connection list in plain JSON and the saved user names and passwords in a
/// second file, AES-128-CBC encrypted with a key that ships inside every DBeaver build (it is
/// obfuscation, not secrecy: the first 16 bytes are the IV). This importer reads only those two
/// files, the ones the person picked; it never goes looking in `~/Library/DBeaverData` on its own.
///
/// **TLS is mapped without a downgrade** (DBX-29, PF-6). dbx hard-codes `ssl: false` for every
/// imported connection, which turns a `verify-full` source into a plaintext one with no notice.
/// Here a connection that names no TLS setting leaves the setting alone, and one that names a mode
/// this app cannot honour (MySQL `VERIFY_CA`) is skipped by name instead of being weakened.
enum DBeaverImport {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// DBeaver's published credentials key (`DBeaverCore` `DEFAULT_KEY`), the same 16 bytes dbx uses.
    private static let key = Data([186, 187, 74, 159, 119, 74, 184, 83, 201, 108, 45, 101, 61, 254, 84, 74])

    /// `url` is `data-sources.json`, or a folder that holds it (the `.dbeaver` folder, or the
    /// workspace around it). The credentials file is the one beside `data-sources.json`.
    static func read(_ url: URL) throws -> ImportBatch {
        let file = try locate(url)
        guard let sources = try? Data(contentsOf: file) else {
            throw Failure(message: "Couldn't read \(file.lastPathComponent).")
        }
        let credentials = try? Data(contentsOf: file.deletingLastPathComponent()
            .appendingPathComponent("credentials-config.json"))
        return try read(dataSources: sources, credentials: credentials)
    }

    static func locate(_ url: URL) throws -> URL {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
            throw Failure(message: "\(url.lastPathComponent) doesn't exist.")
        }
        guard isDirectory.boolValue else { return url }
        for relative in ["data-sources.json", ".dbeaver/data-sources.json",
                         "General/.dbeaver/data-sources.json", "workspace6/General/.dbeaver/data-sources.json"] {
            let candidate = url.appendingPathComponent(relative)
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
        }
        throw Failure(message: "No data-sources.json in \(url.lastPathComponent). Pick the file itself, in DBeaver's .dbeaver folder.")
    }

    static func read(dataSources: Data, credentials: Data?) throws -> ImportBatch {
        guard let root = (try? JSONSerialization.jsonObject(with: dataSources)) as? [String: Any] else {
            throw Failure(message: "That isn't the JSON file DBeaver writes as data-sources.json.")
        }
        guard let entries = root["connections"] as? [String: Any], !entries.isEmpty else {
            throw Failure(message: "data-sources.json has no connections in it.")
        }

        var batch = ImportBatch()
        var secrets: [String: [String: [String: String]]] = [:]
        if let credentials {
            if let decoded = decryptCredentials(credentials) {
                secrets = decoded
            } else {
                batch.notes.append("credentials-config.json couldn't be decrypted, so no saved user names or passwords were read.")
            }
        } else {
            batch.notes.append("No credentials-config.json beside data-sources.json, so no saved passwords were read.")
        }

        var seen = Set<String>()
        // Sorted, because a JSON object has no order and a re-import must come out the same way.
        for (id, raw) in entries.sorted(by: { $0.key < $1.key }) {
            guard let entry = raw as? [String: Any] else { continue }
            let config = entry["configuration"] as? [String: Any] ?? [:]
            let name = text(entry["name"])
            let label = name.isEmpty ? id : name
            do {
                guard let imported = try connection(id: id, entry: entry, config: config,
                                                    secrets: secrets[id] ?? [:], label: label) else {
                    batch.skipped.append(.init(name: label, reason: "DBeaver driver \(driverLabel(entry)) has no driver here"))
                    continue
                }
                if imported.host.isEmpty {
                    batch.skipped.append(.init(name: imported.name, reason: "the entry has no host"))
                    continue
                }
                let identity = [imported.name, imported.kind.rawValue, imported.host, String(imported.port), imported.database]
                    .joined(separator: "\u{0}")
                guard seen.insert(identity).inserted else {
                    batch.skipped.append(.init(name: imported.name, reason: "the same connection is listed twice"))
                    continue
                }
                batch.connections.append(imported)
            } catch let failure as Failure {
                batch.skipped.append(.init(name: label, reason: failure.message))
            }
        }
        if batch.connections.isEmpty && batch.skipped.isEmpty {
            throw Failure(message: "data-sources.json has no connections in it.")
        }
        return batch
    }

    // MARK: One entry

    private static func connection(id: String, entry: [String: Any], config: [String: Any],
                                   secrets: [String: [String: String]], label: String) throws -> ImportedConnection? {
        let url = JDBCImport.parse(text(config["url"]))
        guard let kind = kind(of: entry, url: url) else { return nil }

        let host = firstNonEmpty(text(config["host"]), url?.host ?? "")
        let port = Int(text(config["port"])) ?? url?.port ?? kind.defaultPort
        let database = firstNonEmpty(text(config["database"]), url?.path.first ?? "")
        let schema = kind == .trino ? (url?.path.dropFirst().first ?? "") : ""

        // TLS. Driver properties first, then the URL over them, then the SSL handler's own switch.
        var query: [String: String] = [:]
        for (name, value) in (config["properties"] as? [String: Any]) ?? [:] { query[JDBCImport.normalize(name)] = text(value) }
        for (name, value) in url?.query ?? [:] { query[name] = value }
        let handlers = config["handlers"] as? [String: Any] ?? [:]
        var sslHandlerOn = false
        for (handlerID, raw) in handlers.sorted(by: { $0.key < $1.key }) {
            guard let handler = raw as? [String: Any], handlerID.lowercased().contains("ssl"),
                  !handlerID.lowercased().contains("ssh"), flag(handler["enabled"]) else { continue }
            sslHandlerOn = true
            for (name, value) in (handler["properties"] as? [String: Any]) ?? [:] {
                let key = JDBCImport.normalize(name)
                if query[key] == nil { query[key] = text(value) }
            }
        }
        if sslHandlerOn {
            // The handler stores its switches under DBeaver's own names; say them in the driver's words
            // so a certificate check is never read as plain `require`.
            if kind == .mysql {
                if query["verifyservercertificate"] == nil, let verify = query["sslverifyserver"] { query["verifyservercertificate"] = verify }
                if query["requiressl"] == nil, let require = query["sslrequire"] { query["requiressl"] = require }
            }
            // DBeaver sends `ssl=true` for an SSL handler with no mode, and pgjdbc reads that as verify-full.
            if kind == .postgres, (query["sslmode"] ?? "").isEmpty, query["ssl"] == nil { query["ssl"] = "true" }
        }
        let tls = try JDBCImport.tls(kind: kind, query: query, port: port, floor: sslHandlerOn)

        var imported = ImportedConnection(
            name: label,
            kind: kind,
            host: host,
            port: port,
            user: firstNonEmpty(text(secrets["#connection"]?["user"]), text(secrets["#connection"]?["username"]),
                                text(config["user"]), query["user"] ?? ""),
            database: database,
            schema: schema,
            password: nonEmpty(firstNonEmpty(text(secrets["#connection"]?["password"]), text(config["password"]),
                                             query["password"] ?? "")),
            remarks: text(entry["description"]),
            sourceType: driverLabel(entry),
            sshHost: ""
        )
        imported.sslmode = tls.sslmode
        imported.trinoScheme = tls.scheme
        imported.trinoVerify = tls.verify
        imported.group = folderLabel(text(entry["folder"]))
        // DBeaver's "connection type" (dev, test, prod) is the same idea as this app's label.
        switch text(config["type"]).lowercased() {
        case "dev": imported.environment = .dev
        case "test": imported.environment = .staging
        case "prod": imported.environment = .prod
        default: break
        }
        applyHandlers(handlers, secrets: secrets, to: &imported)
        return imported
    }

    /// The SSH tunnel and what else the entry routes through, as far as this app has a slot for it.
    private static func applyHandlers(_ handlers: [String: Any], secrets: [String: [String: String]],
                                      to imported: inout ImportedConnection) {
        for (handlerID, raw) in handlers.sorted(by: { $0.key < $1.key }) {
            guard let handler = raw as? [String: Any], flag(handler["enabled"]) else { continue }
            let lower = handlerID.lowercased()
            if lower == "ssh_tunnel" {
                applyTunnel(handler, secrets: secrets["network/ssh_tunnel"] ?? [:], to: &imported)
            } else if lower.contains("jump") || lower.contains("proxy") {
                // A hop or a proxy this app cannot express: said out loud, never dropped quietly.
                imported.notImported.append("\(handlerID) handler")
            }
        }
    }

    private static func applyTunnel(_ handler: [String: Any], secrets: [String: String],
                                    to imported: inout ImportedConnection) {
        let props = handler["properties"] as? [String: Any] ?? [:]
        let host = text(props["host"])
        guard !host.isEmpty else {
            imported.notImported.append("SSH tunnel without a host")
            return
        }
        imported.sshHost = host
        imported.sshPort = Int(text(props["port"])) ?? 0
        imported.sshUser = firstNonEmpty(text(secrets["user"]), text(handler["user"]))
        let saved = handler["save-password"].map(flag) ?? true
        let secret = saved ? nonEmpty(firstNonEmpty(text(secrets["password"]), text(handler["password"]))) : nil
        switch text(props["authType"]).uppercased() {
        case "PASSWORD":
            imported.sshAuth = .password
            imported.sshPassword = secret
        case "PUBLIC_KEY":
            let keyPath = text(props["keyPath"])
            if keyPath.isEmpty {
                imported.notImported.append("SSH key file")
            } else {
                imported.sshAuth = .key
                imported.sshKeyPath = keyPath
                // For a key tunnel DBeaver keeps the key's passphrase in the password slot.
                imported.sshPassphrase = secret
            }
        case "AGENT", "":
            imported.sshAuth = .agent
        default:
            // A method this app does not have: the agent is the choice that sends no secret.
            imported.sshAuth = .agent
            imported.notImported.append("SSH method \(text(props["authType"]))")
        }
    }

    // MARK: Which database

    private static func kind(of entry: [String: Any], url: JDBCImport.Target?) -> ConnectionKind? {
        let driver = JDBCImport.normalize(text(entry["driver"]))
        let provider = JDBCImport.normalize(text(entry["provider"]))
        // The driver id is the specific one ("redshift" under the "postgresql" provider), so when
        // there is one it decides, and an unknown one is left out rather than guessed from the provider.
        if !driver.isEmpty { return JDBCImport.kind(ofDriverWord: driver) }
        if !provider.isEmpty, let kind = JDBCImport.kind(ofDriverWord: provider) { return kind }
        return url?.kind
    }

    private static func driverLabel(_ entry: [String: Any]) -> String {
        firstNonEmpty(text(entry["driver"]), text(entry["provider"]), "unknown")
    }

    private static func folderLabel(_ path: String) -> String {
        path.split(separator: "/").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
            .joined(separator: " / ")
    }

    // MARK: Credentials

    /// `{ "<connection id>": { "#connection": { "user": …, "password": … }, "network/ssh_tunnel": {…} } }`.
    /// `nil` when the file is not DBeaver's, so a damaged one is reported and not mistaken for "no passwords".
    static func decryptCredentials(_ data: Data) -> [String: [String: [String: String]]]? {
        guard data.count > kCCBlockSizeAES128 else { return nil }
        let iv = data.prefix(kCCBlockSizeAES128)
        let body = data.dropFirst(kCCBlockSizeAES128)
        var out = Data(count: body.count + kCCBlockSizeAES128)
        var moved = 0
        let status = out.withUnsafeMutableBytes { outBytes in
            body.withUnsafeBytes { inBytes in
                key.withUnsafeBytes { keyBytes in
                    iv.withUnsafeBytes { ivBytes in
                        CCCrypt(CCOperation(kCCDecrypt), CCAlgorithm(kCCAlgorithmAES), CCOptions(kCCOptionPKCS7Padding),
                                keyBytes.baseAddress, key.count, ivBytes.baseAddress,
                                inBytes.baseAddress, body.count, outBytes.baseAddress, outBytes.count, &moved)
                    }
                }
            }
        }
        guard status == kCCSuccess,
              let json = (try? JSONSerialization.jsonObject(with: out.prefix(moved))) as? [String: Any] else { return nil }
        var result: [String: [String: [String: String]]] = [:]
        for (id, scopes) in json {
            var byScope: [String: [String: String]] = [:]
            for (scope, fields) in (scopes as? [String: Any]) ?? [:] {
                byScope[scope] = ((fields as? [String: Any]) ?? [:]).mapValues { text($0) }
            }
            result[id] = byScope
        }
        return result
    }

    // MARK: Small readers

    private static func text(_ value: Any?) -> String {
        switch value {
        case let string as String: string.trimmingCharacters(in: .whitespacesAndNewlines)
        case let number as NSNumber: number.stringValue
        default: ""
        }
    }

    private static func flag(_ value: Any?) -> Bool {
        switch value {
        case let bool as Bool: bool
        case let string as String: string.lowercased() == "true"
        default: false
        }
    }

    private static func firstNonEmpty(_ values: String...) -> String { values.first { !$0.isEmpty } ?? "" }
    private static func nonEmpty(_ value: String) -> String? { value.isEmpty ? nil : value }
}

/// What the DBeaver and DataGrip importers share: a JDBC URL read into its parts, the database a
/// driver word names, and the TLS words of the three drivers mapped into this app's.
enum JDBCImport {
    struct Target {
        var kind: ConnectionKind?
        var host: String
        var port: Int?
        /// The path segments after the authority: the database, and for Trino the schema.
        var path: [String]
        /// Parameter names are `normalize`d: lower case with everything but letters and digits removed.
        var query: [String: String]
    }

    /// `ssl-mode`, `sslMode` and `ssl.mode` are one word.
    static func normalize(_ word: String) -> String {
        word.lowercased().filter { $0.isLetter || $0.isNumber }
    }

    static func kind(ofDriverWord word: String) -> ConnectionKind? {
        // `word` is already `normalize`d: "postgres-jdbc" is "postgresjdbc", "mysql.8" is "mysql8".
        if word.hasPrefix("postgres") && !word.contains("redshift") { return .postgres }
        if word.hasPrefix("mysql") || word.hasPrefix("mariadb") { return .mysql }
        if word.contains("trino") || word.contains("presto") { return .trino }
        return nil
    }

    /// `jdbc:postgresql://user@host1:5432,host2/db?sslmode=require`. A URL with no `//` (the short
    /// `jdbc:postgresql:db` form) has no host to read, and is `nil`.
    static func parse(_ raw: String) -> Target? {
        var rest = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if rest.lowercased().hasPrefix("jdbc:") { rest.removeFirst(5) }
        guard let separator = rest.range(of: "://") else { return nil }
        let subprotocol = normalize(String(rest[..<separator.lowerBound]))
        rest = String(rest[separator.upperBound...])

        var queryText = ""
        if let mark = rest.firstIndex(of: "?") {
            queryText = String(rest[rest.index(after: mark)...])
            rest = String(rest[..<mark])
        }
        let authority: String
        var path: [String] = []
        if let slash = rest.firstIndex(of: "/") {
            authority = String(rest[..<slash])
            path = rest[rest.index(after: slash)...].split(separator: "/").map { String($0).removingPercentEncoding ?? String($0) }
        } else {
            authority = rest
        }
        // Several hosts: the first. Credentials in the authority are not read; they belong in the
        // credential store, and a URL that carries one is the parameter list's business.
        let firstHost = (authority.split(separator: "@").last.map(String.init) ?? authority)
            .split(separator: ",").first.map(String.init) ?? ""
        var host = firstHost
        var port: Int?
        if firstHost.hasPrefix("["), let close = firstHost.firstIndex(of: "]") {
            host = String(firstHost[firstHost.index(after: firstHost.startIndex)..<close])
            port = Int(firstHost[firstHost.index(after: close)...].dropFirst())
        } else if let colon = firstHost.lastIndex(of: ":") {
            host = String(firstHost[..<colon])
            port = Int(firstHost[firstHost.index(after: colon)...])
        }

        var query: [String: String] = [:]
        for pair in queryText.split(whereSeparator: { $0 == "&" || $0 == ";" }) {
            let parts = pair.split(separator: "=", maxSplits: 1).map(String.init)
            guard let name = parts.first else { continue }
            query[normalize(name)] = (parts.count > 1 ? parts[1] : "").removingPercentEncoding ?? ""
        }
        return Target(kind: kind(ofDriverWord: subprotocol), host: host, port: port, path: path, query: query)
    }

    struct TLS: Equatable {
        /// Postgres and MySQL: this app's word, or `nil` when the source said nothing.
        var sslmode: String?
        /// Trino: `https` or `http`, and whether the certificate is checked.
        var scheme: String?
        var verify: Bool?
    }

    private static let libpq = ["disable", "prefer", "require", "verify-ca", "verify-full"]

    /// Maps the source's TLS words to this app's, **never to something weaker** (DBX-29): the same
    /// rules `ConnectionURL` applies to a pasted URL. A word that means more than the app can do, or
    /// that nobody can read, throws a named `DBeaverImport.Failure` and the entry is skipped.
    /// `floor` is "an SSL handler is switched on and named no mode": at least encrypted.
    static func tls(kind: ConnectionKind, query: [String: String], port: Int, floor: Bool) throws -> TLS {
        func fail(_ message: String) -> DBeaverImport.Failure { .init(message: message) }
        switch kind {
        case .postgres:
            if let raw = query["sslmode"], !raw.isEmpty {
                let word = raw.lowercased().replacingOccurrences(of: "_", with: "-")
                // `allow` tries plain text first; `prefer` tries TLS first, so it is never weaker.
                if word == "allow" { return TLS(sslmode: "prefer") }
                guard libpq.contains(word) else {
                    throw fail("sslmode=\(raw) isn't a PostgreSQL SSL mode this app supports. Use \(libpq.joined(separator: ", ")).")
                }
                return TLS(sslmode: word)
            }
            // pgjdbc reads a bare `ssl=true` as verify-full, so that is what it becomes.
            if let ssl = query["ssl"], ["true", "1", ""].contains(ssl.lowercased()) { return TLS(sslmode: "verify-full") }
            return TLS(sslmode: floor ? "require" : nil)
        case .mysql:
            if query["verifyservercertificate"]?.lowercased() == "true", query["usessl"]?.lowercased() != "false" {
                throw fail("verifyServerCertificate=true asks for the certificate to be verified, which the MySQL driver can't do here.")
            }
            if let raw = query["sslmode"], !raw.isEmpty {
                switch raw.uppercased().replacingOccurrences(of: "-", with: "_") {
                case "DISABLED", "DISABLE": return TLS(sslmode: "disable")
                case "PREFERRED", "PREFER": return TLS(sslmode: "prefer")
                // MariaDB's `trust` encrypts without checking, which is what `require` is.
                case "REQUIRED", "REQUIRE", "TRUST": return TLS(sslmode: "require")
                case "VERIFY_CA", "VERIFY_IDENTITY", "VERIFY_FULL":
                    throw fail("sslMode=\(raw) asks for the certificate to be verified, which the MySQL driver can't do here.")
                default:
                    throw fail("sslMode=\(raw) isn't a MySQL SSL mode this app knows. Use DISABLED, PREFERRED or REQUIRED.")
                }
            }
            for name in ["usessl", "requiressl"] {
                switch query[name]?.lowercased() {
                case "true": return TLS(sslmode: "require")
                case "false": return TLS(sslmode: "disable")
                case nil: continue
                default: throw fail("\(name)=\(query[name] ?? "") isn't true or false, so the TLS setting can't be read.")
                }
            }
            return TLS(sslmode: floor ? "require" : nil)
        case .trino:
            let on = query["ssl"]?.lowercased() == "true" || floor
            let off = query["ssl"]?.lowercased() == "false"
            guard on || !off else { return TLS(scheme: "http") }
            guard on else { return TLS(scheme: [443, 8443].contains(port) ? "https" : "http") }
            // The Trino JDBC driver verifies unless told `NONE`; `NONE` is the one answer that is not a check.
            return TLS(scheme: "https", verify: query["sslverification"]?.uppercased() != "NONE")
        }
    }
}
