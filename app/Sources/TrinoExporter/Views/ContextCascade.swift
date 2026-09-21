import AppKit
import SwiftUI

/// The toolbar's context breadcrumb: which connection, and which of its levels, this tab runs
/// against.
///
/// Navicat's query window does this with a row of dropdowns, and the point is not decoration: on
/// Trino a bare `SELECT * FROM wilayah` only resolves if a catalog *and* a schema are set, and
/// picking them here is how you say which. The levels follow the driver exactly as the tree does —
/// Postgres has no catalog level because it cannot query across databases, MySQL has no schema
/// level because a schema *is* a database there — so the breadcrumb can never offer a slot the
/// engine has nowhere to put.
struct ContextCascade: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    private var connection: Connection? { model.connection(for: tab) }
    private var kind: ConnectionKind { connection?.kind ?? .trino }

    /// Trino's first level is a catalog and the other two call it a database; both are answered by
    /// the engine's `catalogs` and both are what a bare table name resolves against.
    private var databaseOptions: [String] {
        model.targetChoices(for: tab.connectionID, kind: kind == .mysql ? .database : .catalog)
    }

    /// One width for every level, fixed.
    ///
    /// Fixed because the names are data: a 30-character catalog must not be able to move the
    /// buttons beside it. Equal because they are the same kind of thing — a row of pickers at
    /// different widths reads as though one of them matters more, which is not a claim this bar is
    /// making. 200pt fits `datawarehouse-main-pusdatin-masked`; anything longer loses its middle
    /// rather than its tail.
    private var levelWidth: CGFloat { 200 }

    private var schemaOptions: [String] {
        model.targetChoices(for: tab.connectionID, kind: .schema, database: model.database(for: tab))
    }

    var body: some View {
        HStack(spacing: 6) {
            // The same width as the levels beside it: all four are pickers, and a row of them at
            // different widths reads as a claim about which one matters more.
            ConnectionPickerButton(selection: $tab.connectionID)
                .frame(width: levelWidth)

            if connection != nil {
                ToolbarSeparator()
                if kind.contextLevels.contains(.catalog) || kind.contextLevels.contains(.database) {
                    picker(label: kind.databaseLabel,
                           value: model.database(for: tab),
                           options: databaseOptions,
                           loading: model.isLoadingOptions(for: tab.connectionID),
                           width: levelWidth) { choice in
                        // Clearing the level below is not optional: a schema from the old catalog
                        // does not exist in the new one, and leaving it would run the next query
                        // against a context that cannot resolve.
                        tab.contextDatabase = choice
                        tab.contextSchema = ""
                        model.loadSchemas(for: tab.connectionID, catalog: choice)
                    }
                }
                if kind.contextLevels.contains(.schema) {
                    picker(label: "Schema",
                           value: model.schema(for: tab),
                           options: schemaOptions,
                           loading: model.isLoadingOptions(for: tab.connectionID,
                                                           catalog: model.database(for: tab)),
                           width: levelWidth) { choice in
                        tab.contextSchema = choice
                    }
                }
            }
        }
        .onAppear { reload() }
        .onChange(of: tab.connectionID) { _, _ in
            // A different connection means a different server: keep nothing from the old one.
            tab.contextDatabase = ""
            tab.contextSchema = ""
            reload()
        }
    }

    private func reload() {
        model.loadCatalogs(for: tab.connectionID)
        if !model.database(for: tab).isEmpty {
            model.loadSchemas(for: tab.connectionID, catalog: model.database(for: tab))
        }
    }

    /// One level. A menu, not a text field: these name things that already exist, and the engine
    /// resolves a bare table name against whatever is chosen here, so a typo would fail at run time
    /// with a message about the query rather than about the picker.
    /// One level, at a fixed width.
    ///
    /// Fixed rather than sized to its content, because the names are user data: a catalog called
    /// `aktivitas-produksi-harian` used to push the whole breadcrumb across the toolbar and squash
    /// Run into the corner. Truncating in the middle is what keeps the level readable — the head
    /// and tail are what distinguish one long name from another.
    ///
    /// A menu, not a text field: these name things that already exist, and the engine resolves a
    /// bare table name against whatever is chosen here, so a typo would fail at run time with a
    /// message about the query rather than about the picker.
    private func picker(label: String, value: String, options: [String], loading: Bool,
                        width: CGFloat, choose: @escaping (String) -> Void) -> some View {
        let showing = !value.isEmpty
        return Menu {
            if options.isEmpty {
                Text(loading ? "Loading…" : "Nothing loaded — expand the connection in the tree")
            } else {
                ForEach(options, id: \.self) { option in
                    Button(option) { choose(option) }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Text(value.isEmpty ? label : value)
                    .font(.system(size: 11.5, design: .monospaced))
                    .foregroundStyle(showing ? Tone.ink.opacity(0.92) : Tone.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 2)
                if loading {
                    ProgressView().controlSize(.mini)
                } else {
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(Tone.secondary)
                }
            }
            .padding(.horizontal, 9)
            .frame(width: width, height: 28)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(Tone.ink.opacity(showing ? 0.16 : 0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .frame(width: width)
        .help(showing ? "\(label): \(value)" : "Choose a \(label.lowercased()) for this query")
    }
}
