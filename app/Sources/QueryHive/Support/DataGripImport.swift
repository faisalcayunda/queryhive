import Foundation
import Security

/// What reading one item out of another app's Keychain entry came to. Cancel and not-found are
/// different answers and are reported differently: "you said no" is the person's to act on, "DataGrip
/// never saved one" is not.
enum ForeignKeychainResult: Equatable {
    case found(String)
    case notFound
    /// The person dismissed or denied the system prompt.
    case cancelled
    case failed(String)
}

/// Reads another app's generic-password item by its service name.
protocol ForeignKeychain {
    func password(service: String) -> ForeignKeychainResult
}

/// `SecItemCopyMatching`, with the system's own prompt. The item belongs to DataGrip, so macOS asks
/// the person for each one (Touch ID or the login password) and "Always Allow" is theirs to give or
/// withhold; nothing here sets `kSecUseAuthenticationUIFail`, because refusing to prompt would make
/// every item look like it was not there.
struct SystemForeignKeychain: ForeignKeychain {
    func password(service: String) -> ForeignKeychainResult {
        let search: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
                                     kSecAttrService as String: service,
                                     kSecReturnData as String: true,
                                     kSecMatchLimit as String: kSecMatchLimitOne]
        var result: AnyObject?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            guard let data = result as? Data else { return .failed("the item had no data") }
            return .found(String(decoding: data, as: UTF8.self))
        case errSecItemNotFound:
            return .notFound
        // -128 is the prompt's Cancel; -25293 is its Deny.
        case errSecUserCanceled, errSecAuthFailed:
            return .cancelled
        default:
            return .failed((SecCopyErrorMessageString(status, nil) as String?) ?? "Keychain error \(status)")
        }
    }
}

/// Reads DataGrip's `dataSources.xml`, with `dataSources.local.xml` (user names, the product) and
/// `db-forest-config.xml` (the folder tree) when they were picked too, and DataGrip's saved
/// passwords from the Keychain when the person agreed to that.
///
/// DataGrip keeps no password in those files. They live in the macOS Keychain under the service
/// `IntelliJ Platform DB — <data source uuid>`, so the passwords are read there, one item at a time,
/// through `ForeignKeychain` and only when the caller passes one in. Cancelling the system prompt
/// stops the reading (a prompt per item, dismissed thirty times, is not what anyone wants), and what
/// was and was not read is reported.
///
/// An SSH tunnel or a proxy DataGrip has configured is not imported: its format here is the one dbx
/// does not read either, and a guessed tunnel is worse than a named gap. The entry says it had one.
enum DataGripImport {
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    /// The three files, found among what the person picked.
    struct Files {
        var dataSources: URL
        var local: URL?
        var forest: URL?
    }

    /// Picks the files by name, from files or from a folder (the project's `.idea`, or the folder
    /// holding `dataSources.xml`). `dataSources.xml` is required.
    static func locate(_ urls: [URL]) throws -> Files {
        var candidates: [URL] = []
        for url in urls {
            var isDirectory: ObjCBool = false
            if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue {
                for folder in [url, url.appendingPathComponent(".idea")] {
                    candidates += ["dataSources.xml", "dataSources.local.xml", "db-forest-config.xml"]
                        .map { folder.appendingPathComponent($0) }
                        .filter { FileManager.default.fileExists(atPath: $0.path) }
                }
            } else {
                candidates.append(url)
            }
        }
        func find(_ name: String) -> URL? { candidates.first { $0.lastPathComponent.lowercased() == name.lowercased() } }
        guard let dataSources = find("dataSources.xml") else {
            throw Failure(message: "Pick dataSources.xml (required), and dataSources.local.xml and db-forest-config.xml if you have them.")
        }
        return Files(dataSources: dataSources, local: find("dataSources.local.xml"), forest: find("db-forest-config.xml"))
    }

    static func read(_ files: Files, keychain: (any ForeignKeychain)?) throws -> ImportBatch {
        guard let shared = try? Data(contentsOf: files.dataSources) else {
            throw Failure(message: "Couldn't read \(files.dataSources.lastPathComponent).")
        }
        return try read(dataSources: shared,
                        local: files.local.flatMap { try? Data(contentsOf: $0) },
                        forest: files.forest.flatMap { try? Data(contentsOf: $0) },
                        keychain: keychain)
    }

    static func read(dataSources: Data, local: Data?, forest: Data?,
                     keychain: (any ForeignKeychain)?) throws -> ImportBatch {
        guard let shared = parse(dataSources) else {
            throw Failure(message: "That isn't the XML file DataGrip writes as dataSources.xml.")
        }
        let localByID = Dictionary((local.flatMap(parse) ?? []).map { ($0.uuid, $0) }, uniquingKeysWith: { first, _ in first })
        let forestPaths = forest.flatMap(parseForest) ?? [:]
        guard !shared.isEmpty else { throw Failure(message: "dataSources.xml has no data sources in it.") }

        var batch = ImportBatch()
        var seen = Set<String>()
        var cancelledAfter: String?
        var unread: [String] = []
        var notFound: [String] = []

        for source in shared {
            let merged = source.merged(with: localByID[source.uuid])
            let url = JDBCImport.parse(merged.jdbcURL)
            let driverWord = JDBCImport.normalize(String(merged.driverRef.split(separator: ".").first ?? ""))
            // The driver reference decides when there is one; a data source with none (a custom
            // driver) falls back to what its URL says.
            let kind = driverWord.isEmpty ? url?.kind : JDBCImport.kind(ofDriverWord: driverWord)
            let label = merged.name.isEmpty ? merged.uuid : merged.name
            guard let kind else {
                batch.skipped.append(.init(name: label, reason: "DataGrip driver \(merged.driverRef.isEmpty ? "unknown" : merged.driverRef) has no driver here"))
                continue
            }
            guard let url, !url.host.isEmpty else {
                batch.skipped.append(.init(name: label, reason: "the data source has no host in its URL"))
                continue
            }
            let port = url.port ?? kind.defaultPort

            var query = url.query
            if let mode = merged.sslMode, query["sslmode"] == nil { query["sslmode"] = mode }
            let tls: JDBCImport.TLS
            do {
                tls = try JDBCImport.tls(kind: kind, query: query, port: port, floor: merged.sslEnabled)
            } catch let failure as DBeaverImport.Failure {
                batch.skipped.append(.init(name: label, reason: failure.message))
                continue
            }

            let identity = [label, kind.rawValue, url.host, String(port), url.path.first ?? ""].joined(separator: "\u{0}")
            guard seen.insert(identity).inserted else {
                batch.skipped.append(.init(name: label, reason: "the same data source is listed twice"))
                continue
            }

            var imported = ImportedConnection(
                name: label, kind: kind, host: url.host, port: port,
                user: merged.userName, database: url.path.first ?? "",
                schema: kind == .trino ? (url.path.dropFirst().first ?? "") : "",
                password: nil, remarks: "", sourceType: merged.driverRef, sshHost: "")
            imported.sslmode = tls.sslmode
            imported.trinoScheme = tls.scheme
            imported.trinoVerify = tls.verify
            imported.group = (forestPaths[merged.uuid] ?? merged.group).split(separator: "/")
                .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " / ")
            if merged.sshEnabled { imported.notImported.append("SSH tunnel") }

            if let keychain {
                if cancelledAfter != nil {
                    unread.append(label)
                } else {
                    switch keychain.password(service: Self.keychainService(uuid: merged.uuid)) {
                    case .found(let password):
                        imported.password = password.isEmpty ? nil : password
                    case .notFound:
                        notFound.append(label)
                    case .cancelled:
                        cancelledAfter = label
                        unread.append(label)
                    case .failed(let message):
                        batch.notes.append("Couldn't read the password for \(label) from Keychain: \(message).")
                    }
                }
            }
            batch.connections.append(imported)
        }

        if keychain == nil {
            batch.notes.append("No passwords were read: DataGrip keeps them in the Keychain, and you chose not to read them.")
        } else {
            if let cancelledAfter {
                batch.notes.append("Reading from Keychain was cancelled at \(cancelledAfter), so these have no password: "
                                   + unread.joined(separator: ", ") + ". Import again to be asked again.")
            }
            if !notFound.isEmpty {
                batch.notes.append("DataGrip had no saved password in Keychain for: " + notFound.joined(separator: ", ") + ".")
            }
        }
        if batch.connections.isEmpty && batch.skipped.isEmpty {
            throw Failure(message: "dataSources.xml has no data sources in it.")
        }
        return batch
    }

    /// The service DataGrip files a data source's password under.
    static func keychainService(uuid: String) -> String { "IntelliJ Platform DB \u{2014} \(uuid)" }

    // MARK: dataSources.xml

    struct Fragment {
        var uuid = ""
        var name = ""
        var group = ""
        var driverRef = ""
        var jdbcURL = ""
        var userName = ""
        var sshEnabled = false
        var sslEnabled = false
        var sslMode: String?

        /// The shared file has the driver and URL, the local one the user name; each fills what the other lacks.
        func merged(with local: Fragment?) -> Fragment {
            guard let local else { return self }
            var out = self
            if out.name.isEmpty { out.name = local.name }
            if out.userName.isEmpty { out.userName = local.userName }
            out.sshEnabled = out.sshEnabled || local.sshEnabled
            out.sslEnabled = out.sslEnabled || local.sslEnabled
            if out.sslMode == nil { out.sslMode = local.sslMode }
            return out
        }
    }

    /// `nil` when the bytes are not XML at all.
    static func parse(_ data: Data) -> [Fragment]? {
        let collector = Collector()
        let parser = XMLParser(data: data)
        parser.delegate = collector
        guard parser.parse() else { return nil }
        return collector.sources.filter { !$0.uuid.isEmpty }
    }

    private final class Collector: NSObject, XMLParserDelegate {
        var sources: [Fragment] = []
        private var current: Fragment?
        private var stack: [String] = []
        private var text = ""

        func parser(_ parser: XMLParser, didStartElement elementName: String, namespaceURI: String?,
                    qualifiedName: String?, attributes: [String: String] = [:]) {
            stack.append(elementName)
            text = ""
            if elementName == "data-source" {
                // A `data-source` inside one (never seen) would restart the fragment, not nest.
                current = Fragment(uuid: attributes["uuid"] ?? "", name: attributes["name"] ?? "",
                                   group: attributes["group"] ?? attributes["group-name"] ?? "")
            }
        }

        func parser(_ parser: XMLParser, foundCharacters string: String) { text += string }

        func parser(_ parser: XMLParser, didEndElement elementName: String, namespaceURI: String?,
                    qualifiedName: String?) {
            defer { stack.removeLast(); text = "" }
            guard current != nil else { return }
            let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
            switch stack.suffix(2).joined(separator: ".") {
            case "data-source.driver-ref": current?.driverRef = value
            case "data-source.jdbc-url": current?.jdbcURL = value
            case "data-source.user-name": current?.userName = value
            case "ssh-properties.enabled": current?.sshEnabled = value.lowercased() == "true"
            // `<ssl-config><enabled>` and `<mode>`: read on the strength of the names, not checked
            // against a real file. A mode this does not understand is refused by `JDBCImport.tls`.
            case "ssl-config.enabled": current?.sslEnabled = value.lowercased() == "true"
            case "ssl-config.mode": current?.sslMode = value.isEmpty ? nil : value
            default: break
            }
            if elementName == "data-source", let done = current {
                sources.append(done)
                current = nil
            }
        }
    }

    // MARK: db-forest-config.xml

    /// uuid to folder path, from the legacy folder tree: group lines `id:parent:uuid:name`, then a
    /// dashed line, then connection lines `id:parent:uuid`.
    static func parseForest(_ data: Data) -> [String: String]? {
        let collector = ForestCollector()
        let parser = XMLParser(data: data)
        parser.delegate = collector
        guard parser.parse(), !collector.data.isEmpty else { return nil }

        var groups: [String: (parent: String, name: String)] = [:]
        var members: [(uuid: String, parent: String)] = []
        var readingMembers = false
        for raw in collector.data.split(whereSeparator: \.isNewline) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line == "." { continue }
            if line.count >= 8, line.allSatisfy({ $0 == "-" }) { readingMembers = true; continue }
            // `id:parent:uuid[:name]`; the name may hold a colon, so the split is at most four ways.
            let parts = line.split(separator: ":", maxSplits: 3, omittingEmptySubsequences: false).map(String.init)
            guard parts.count >= 3 else { continue }
            if readingMembers {
                members.append((parts[2], parts[1]))
            } else if parts.count == 4 {
                // A zero-width space can lead a name DataGrip wrote.
                let name = parts[3].trimmingCharacters(in: CharacterSet(charactersIn: "\u{200B}\u{FEFF}").union(.whitespaces))
                if !name.isEmpty { groups[parts[0]] = (parts[1], name) }
            }
        }

        func path(of id: String, depth: Int = 0) -> String {
            guard depth < 32, let group = groups[id] else { return "" }
            let parent = group.parent == "0" ? "" : path(of: group.parent, depth: depth + 1)
            return parent.isEmpty ? group.name : parent + "/" + group.name
        }
        var result: [String: String] = [:]
        for member in members {
            let found = path(of: member.parent)
            if !found.isEmpty { result[member.uuid] = found }
        }
        return result
    }

    private final class ForestCollector: NSObject, XMLParserDelegate {
        var data = ""
        private var inside = false

        func parser(_ parser: XMLParser, didStartElement elementName: String, namespaceURI: String?,
                    qualifiedName: String?, attributes: [String: String] = [:]) {
            inside = elementName == "data"
        }

        func parser(_ parser: XMLParser, foundCharacters string: String) {
            if inside { data += string }
        }

        func parser(_ parser: XMLParser, didEndElement elementName: String, namespaceURI: String?,
                    qualifiedName: String?) {
            if elementName == "data" { inside = false }
        }
    }
}
