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

/// A saved Trino coordinator, Navicat style. The password never lives here: it is stored
/// separately in the Keychain, keyed by `id`, and a connection with no stored password simply
/// connects without BasicAuth.
struct Connection: Identifiable, Codable, Equatable {
    var id: UUID
    var name: String
    var color: ConnectionColor
    var host: String
    var port: Int
    /// "http" or "https". A port of 443/8443 or a stored password upgrades this to https in
    /// the engine, matching `TrinoConfig.__post_init__`.
    var httpScheme: String
    var user: String
    var catalog: String
    var schema: String
    var verify: Bool

    /// One-line identity for the sidebar and the picker: `host:port/catalog.schema`.
    var displayTarget: String {
        var text = "\(host):\(port)"
        if !catalog.isEmpty {
            text += "/\(catalog)"
            if !schema.isEmpty { text += ".\(schema)" }
        }
        return text
    }
}

/// Parses a `https://user:secret@host:8443/catalog/schema` string into the fields of a
/// Connection. Used by "Add from URL…", the one-step way to get a connection without typing
/// six fields. Mirrors the engine's `TrinoConfig.from_url`, including the scheme default.
enum TrinoURL {
    struct Parsed {
        var host: String
        var port: Int
        var httpScheme: String
        var user: String
        var password: String?
        var catalog: String
        var schema: String
    }

    static func parse(_ raw: String) -> Parsed? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        // A bare `host:8443/path` would be read as the scheme `host`, so only an explicit
        // `://` counts as one; anything else is assumed to be plain http.
        let text = trimmed.contains("://") ? trimmed : "http://" + trimmed
        guard let parts = URLComponents(string: text), let host = parts.host, !host.isEmpty else { return nil }
        let scheme = parts.scheme?.lowercased() == "https" ? "https" : "http"
        let path = parts.path.split(separator: "/").map(String.init)
        return Parsed(
            host: host,
            port: parts.port ?? (scheme == "https" ? 443 : 8080),
            httpScheme: scheme,
            user: parts.user?.removingPercentEncoding ?? "",
            password: parts.password?.removingPercentEncoding,
            catalog: path.count > 0 ? path[0] : "",
            schema: path.count > 1 ? path[1] : ""
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
