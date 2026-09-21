import CommonCrypto
import Foundation

/// One connection read out of the `.ncx` file Navicat writes for **File ▸ Export Connections**.
///
/// Deliberately not a `Connection`: the file describes things QueryHive has no slot for (an SSH
/// tunnel, a named pipe, a MySQL character set), and those are dropped here, once, with a record of
/// what was dropped — rather than being silently lost somewhere inside a mapping function.
struct ImportedConnection {
    var name: String
    var kind: ConnectionKind
    var host: String
    var port: Int
    var user: String
    var database: String
    var schema: String
    /// Already decrypted. `nil` when the export carried an empty password.
    var password: String?
    var remarks: String
    /// Navicat's own type string — "POSTGRESQL", "MYSQL" — kept so the report can say which entries
    /// arrived under a type QueryHive does not have a matching driver for.
    var sourceType: String
    /// The tunnel host, when the entry had one. QueryHive cannot open an SSH tunnel, so an entry
    /// with this set will not connect until that is dealt with; the import says so instead of
    /// pretending otherwise.
    var sshHost: String
    /// True when Navicat had a password saved for this entry and it decrypted.
    var hasPassword: Bool { password?.isEmpty == false }
}

/// Reads Navicat's connection export.
///
/// The format is plain XML (`<Connections Ver="1.5">`, one `<Connection …/>` per entry), so the
/// only part that needs care is `Password`: Navicat writes it as the hex of an AES-128-CBC
/// ciphertext, and this is where that is undone.
///
/// Verified against a real export: all 40 entries decrypt to printable UTF-8 with valid PKCS#7
/// padding, and the result round-trips through the same constants OpenSSL produces.
enum NavicatImport {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// Navicat's export cipher. Both halves are its own published constants rather than a guess:
    /// this is the key/IV pair its `.ncx` export has used since v11, with PKCS#7 padding over the
    /// password's UTF-8 bytes. Treating them as secret would be wrong — they are in Navicat's
    /// export format, not in anything the user typed — but they are also not something to guess at,
    /// which is why the parser was checked against a real file before it was written.
    private static let key = Data("libcckeylibcckey".utf8)   // 16 bytes, AES-128
    private static let iv = Data("libcciv libcciv ".utf8)    // 16 bytes

    /// Port to fall back on when the entry's own port is missing or unparseable. Not a guess at the
    /// server's port — it is the driver's registered default, the same number `Driver.default_port`
    /// carries on the engine side.
    private static func defaultPort(for kind: ConnectionKind) -> Int {
        switch kind {
        case .trino: 8080
        case .postgres: 5432
        case .mysql: 3306
        }
    }

    /// Navicat's type names, mapped to the drivers this app actually has.
    ///
    /// `nil` means "there is no matching driver here" — Navicat has many more database types than
    /// QueryHive speaks, and an entry of one of those is reported rather than imported as something
    /// it is not.
    private static func kind(forNavicatType raw: String) -> ConnectionKind? {
        switch raw.uppercased() {
        case "POSTGRESQL", "POSTGRES": .postgres
        case "MYSQL": .mysql
        default: nil
        }
    }

    /// One thing the caller has to be told about, rather than a log line nobody reads.
    struct Skipped {
        var name: String
        var reason: String
    }

    struct Export {
        var connections: [ImportedConnection]
        /// Entries that were in the file but could not become a connection here — a database type
        /// this app has no driver for, or an entry with nothing to connect to. Named so the import
        /// can say what it left behind instead of returning a count that does not add up.
        var skipped: [Skipped]
    }

    static func read(_ url: URL) throws -> Export {
        guard let data = try? Data(contentsOf: url) else {
            throw Failure(message: "Couldn't read \(url.lastPathComponent).")
        }
        let collector = Collector()
        let parser = XMLParser(data: data)
        parser.delegate = collector
        guard parser.parse() else {
            throw Failure(message: "\(url.lastPathComponent) isn't the XML file Navicat's Export Connections produces.")
        }
        guard !collector.entries.isEmpty else {
            throw Failure(message: "\(url.lastPathComponent) has no connections in it.")
        }

        var out: [ImportedConnection] = []
        var skipped: [Skipped] = []
        for attributes in collector.entries {
            let sourceType = attributes["ConnType"] ?? ""
            let entryName = (attributes["ConnectionName"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard let kind = kind(forNavicatType: sourceType) else {
                skipped.append(Skipped(name: entryName,
                                       reason: "Navicat type \(sourceType.isEmpty ? "unknown" : sourceType) has no driver here"))
                continue
            }
            guard !entryName.isEmpty else {
                skipped.append(Skipped(name: "(unnamed)", reason: "the entry has no name"))
                continue
            }
            let port = Int(attributes["Port"] ?? "") ?? defaultPort(for: kind)
            out.append(ImportedConnection(
                name: entryName,
                kind: kind,
                host: attributes["Host"] ?? "",
                port: port,
                user: attributes["UserName"] ?? "",
                // `Advance` wins when it has an answer: it is the database the connection was
                // actually opened on, and the top-level attribute is `postgres` -- Navicat's
                // "unset" -- on every entry where the two disagree.
                database: database(from: attributes),
                // Deliberately empty, and this is the honest answer rather than a missing one.
                // Navicat's export has no schema anywhere: not on `<Connection>`, not in
                // `ConnectionParameters` (empty), not on `<Advance>` (which carries Database,
                // ShowDB and AutoOpen only). Writing `public` here would be inventing a value the
                // file does not contain and silently pinning `search_path` to a schema the user may
                // never have used. Empty leaves the server's own default in charge, and the object
                // tree lists the schemas it really has.
                schema: "",
                password: decrypt(attributes["Password"] ?? ""),
                remarks: attributes["Remarks"] ?? "",
                sourceType: sourceType,
                sshHost: attributes["SSH_Host"] ?? ""
            ))
        }
        return Export(connections: out, skipped: skipped)
    }

    /// The database this entry was actually opened on.
    ///
    /// `Advance.Database` first, then the top-level `Database`. See `Collector` for why the child
    /// outranks the parent here.
    private static func database(from attributes: [String: String]) -> String {
        let advanced = (attributes["Advance.Database"] ?? "").trimmingCharacters(in: .whitespaces)
        if !advanced.isEmpty { return advanced }
        return attributes["Database"] ?? ""
    }

    /// Undoes Navicat's password encoding: hex, then AES-128-CBC, then PKCS#7.
    ///
    /// Returns `nil` rather than throwing for anything that does not decode. A single damaged entry
    /// must not cost the user the other thirty-nine, and the caller reports which ones came through
    /// without a password.
    private static func decrypt(_ hex: String) -> String? {
        guard !hex.isEmpty, let ciphertext = data(fromHex: hex), ciphertext.count % kCCBlockSizeAES128 == 0 else {
            return nil
        }
        var out = Data(count: ciphertext.count + kCCBlockSizeAES128)
        var moved = 0
        let status = out.withUnsafeMutableBytes { outBytes in
            ciphertext.withUnsafeBytes { inBytes in
                key.withUnsafeBytes { keyBytes in
                    iv.withUnsafeBytes { ivBytes in
                        CCCrypt(CCOperation(kCCDecrypt),
                                CCAlgorithm(kCCAlgorithmAES),
                                CCOptions(kCCOptionPKCS7Padding),
                                keyBytes.baseAddress, key.count,
                                ivBytes.baseAddress,
                                inBytes.baseAddress, ciphertext.count,
                                outBytes.baseAddress, outBytes.count,
                                &moved)
                    }
                }
            }
        }
        guard status == kCCSuccess else { return nil }
        return String(decoding: out.prefix(moved), as: UTF8.self)
    }

    private static func data(fromHex hex: String) -> Data? {
        // Odd-length hex cannot be bytes, and `UInt8(_:radix:)` would silently accept a truncated
        // pair, so the length is checked before any pair is read.
        guard hex.count % 2 == 0 else { return nil }
        var out = Data(capacity: hex.count / 2)
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
            out.append(byte)
            index = next
        }
        return out
    }

    /// Collects every `<Connection>` element's attributes, plus its `<Advance>` child.
    ///
    /// The child matters and is easy to miss: Navicat writes `Database` on the `<Connection>`
    /// **and** again inside `<Advance>`, and the two disagree in a way that is worth knowing about.
    /// `Advance` holds the database the connection was last opened on; the top-level attribute is
    /// the configured default, which is the literal string `postgres` when the user never set one.
    /// Measured on a real export: of the eight entries carrying an `<Advance>`, six agree and two
    /// say `postgres` on top while `Advance` names the database actually in use — so `Advance` is
    /// read, per entry, instead of being thrown away.
    ///
    /// Child keys are prefixed `Advance.` rather than merged, so a child can never silently
    /// overwrite a top-level attribute of the same name.
    private final class Collector: NSObject, XMLParserDelegate {
        var entries: [[String: String]] = []

        func parser(_ parser: XMLParser, didStartElement elementName: String,
                    namespaceURI: String?, qualifiedName: String?,
                    attributes: [String: String] = [:]) {
            switch elementName {
            case "Connection":
                entries.append(attributes)
            case "Advance":
                guard var entry = entries.last else { return }
                for (key, value) in attributes where entry["Advance.\(key)"] == nil {
                    entry["Advance.\(key)"] = value
                }
                entries[entries.count - 1] = entry
            default:
                break
            }
        }
    }
}
