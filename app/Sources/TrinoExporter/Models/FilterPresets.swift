import Foundation

/// A saved filter set, keyed by column **name** rather than position.
///
/// Name-keyed because the same table can come back in a different column order between runs, and a
/// preset that pointed at "column 3" would then filter a different column without saying so.
/// Applying maps each name back to the current result's source index; a name the result does not
/// have is reported rather than silently dropped, because a preset that quietly filtered less than
/// it said would be a lie about the rows on screen.
struct FilterPreset: Codable, Equatable, Identifiable {
    var name: String
    var filters: [Entry]

    var id: String { name }

    /// One column's filter, in the two shapes `ColumnFilter` has. `values` and `text` are mutually
    /// exclusive; which one is set is the mode.
    struct Entry: Codable, Equatable {
        var column: String
        var values: [String]?
        var text: String?
    }

    /// The preset the filters currently set on a result become, or `nil` when nothing is filtered.
    static func from(name: String, filters: [Int: ColumnFilter],
                     columns: [Event.Column]) -> FilterPreset? {
        var entries: [Entry] = []
        for index in filters.keys.sorted() {
            guard columns.indices.contains(index), let filter = filters[index] else { continue }
            entries.append(entry(column: columns[index].name, filter: filter))
        }
        guard !entries.isEmpty else { return nil }
        return FilterPreset(name: name, filters: entries)
    }

    private static func entry(column: String, filter: ColumnFilter) -> Entry {
        switch filter {
        // Sorted, so the same selection writes the same preset and two presets with the same values
        // compare equal rather than differing by the set's iteration order.
        case .values(let picked): Entry(column: column, values: picked.sorted(), text: nil)
        case .text(let needle): Entry(column: column, values: nil, text: needle)
        }
    }

    /// The filters this preset becomes against a result's columns: keyed by source index, plus the
    /// column names that matched nothing. A repeated column name resolves to its first column, which
    /// is the only choice that does not need a second identity.
    func resolve(columns: [Event.Column]) -> (filters: [Int: ColumnFilter], missing: [String]) {
        var resolved: [Int: ColumnFilter] = [:]
        var missing: [String] = []
        for entry in filters {
            guard let index = columns.firstIndex(where: { $0.name == entry.column }) else {
                missing.append(entry.column)
                continue
            }
            if let values = entry.values {
                resolved[index] = .values(Set(values))
            } else if let text = entry.text {
                resolved[index] = .text(text)
            }
        }
        return (resolved, missing)
    }
}

/// Where saved filter sets live.
///
/// A preset describes a table's columns, so it is filed per **connection + table** — the same unit a
/// per-column display format uses (`ColumnFormatStore`). A preset made on
/// `hive.analytics.penerima_manfaat` is there the next time that table is opened, and does not
/// appear on a different server's table that happens to share the name.
///
/// A hand-written query has no table to file against, so there is nowhere durable to put a preset:
/// it stays with the tab for as long as the tab lives (`QueryTab.localPresets`) and is not written
/// to disk. That split is the decision — durable where the app genuinely knows the table, session
/// where it does not — rather than a second identity invented for the hand-written case.
enum FilterPresetStore {
    static let defaultsKey = "filterPresets"

    /// The key a set of presets is filed under, or `nil` when the tab has no connection or no table.
    static func identity(connection: UUID?, table: String?) -> String? {
        guard let connection, let table, !table.isEmpty else { return nil }
        return "\(connection.uuidString)|\(table)"
    }

    static func load(in defaults: UserDefaults = .standard) -> [String: [FilterPreset]] {
        guard let data = defaults.data(forKey: defaultsKey),
              let stored = try? JSONDecoder().decode([String: [FilterPreset]].self, from: data)
        else { return [:] }
        return stored
    }

    static func save(_ presets: [String: [FilterPreset]], in defaults: UserDefaults = .standard) {
        guard !presets.isEmpty else {
            defaults.removeObject(forKey: defaultsKey)
            return
        }
        guard let data = try? JSONEncoder().encode(presets) else { return }
        defaults.set(data, forKey: defaultsKey)
    }
}
