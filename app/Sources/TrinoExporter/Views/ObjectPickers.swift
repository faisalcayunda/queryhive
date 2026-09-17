import AppKit
import SwiftUI

/// Navicat's object pickers, fitted to the layout this app already has rather than copied into a
/// third row of chrome.
///
/// They sit at the right of the editor's own header, which was empty, and they follow the
/// connection's driver levels exactly as the tree does — a schema picker for MySQL would offer
/// nothing, because MySQL has no schema level, and Postgres has no catalog level either. Picking a
/// table inserts its quoted name at the caret through `qualifiedName`, the same routine the tree's
/// double-click uses, so the two cannot produce different SQL for the same table.
struct ObjectPickers: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    private var kind: ConnectionKind { model.connection(for: tab)?.kind ?? .trino }

    var body: some View {
        HStack(spacing: 6) {
            if kind != .postgres {
                // Trino calls it a catalog, MySQL a database; both are the level under the
                // connection, and both are answered by the engine's `catalogs`.
                picker(label: kind.databaseLabel.lowercased(),
                       value: tab.pickerDatabase,
                       options: model.targetChoices(for: tab.connectionID,
                                                    kind: kind == .mysql ? .database : .catalog),
                       loading: model.isLoadingOptions(for: tab.connectionID)) { choice in
                    tab.pickerDatabase = choice
                    tab.pickerSchema = ""
                    model.loadSchemas(for: tab.connectionID, catalog: choice)
                }
            }
            if kind.hasSchemaLevel {
                picker(label: "schema",
                       value: tab.pickerSchema,
                       options: model.targetChoices(for: tab.connectionID, kind: .schema,
                                                    database: tab.pickerDatabase),
                       loading: model.isLoadingOptions(for: tab.connectionID, catalog: tab.pickerDatabase)) { choice in
                    tab.pickerSchema = choice
                    model.loadTables(for: tab.connectionID, catalog: tab.pickerDatabase, schema: choice)
                }
            }
            picker(label: "table",
                   value: "",
                   options: model.tableChoices(for: tab.connectionID, catalog: tab.pickerDatabase,
                                               schema: tab.pickerSchema),
                   loading: model.isLoadingTables(for: tab.connectionID, catalog: tab.pickerDatabase,
                                                  schema: tab.pickerSchema),
                   inserts: true) { choice in
                insert(choice)
            }
        }
        .onAppear {
            model.loadCatalogs(for: tab.connectionID)
            if !tab.pickerDatabase.isEmpty {
                model.loadSchemas(for: tab.connectionID, catalog: tab.pickerDatabase)
            }
            if !tab.pickerSchema.isEmpty {
                model.loadTables(for: tab.connectionID, catalog: tab.pickerDatabase, schema: tab.pickerSchema)
            }
        }
    }

    /// One picker. A `Menu` and not a combo box: unlike the export destination — which is a value
    /// the user may type because it may not exist yet — everything here already exists somewhere,
    /// so offering a free-text field would only invite a typo the picker exists to prevent.
    private func picker(label: String, value: String, options: [String], loading: Bool,
                        inserts: Bool = false, choose: @escaping (String) -> Void) -> some View {
        Menu {
            if options.isEmpty {
                Text(loading ? "Loading…" : "Nothing loaded — expand the connection in the tree")
            } else {
                ForEach(options, id: \.self) { option in
                    Button(option) { choose(option) }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: inserts ? "tablecells" : "cylinder")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(inserts ? Tone.mint : Tone.ice)
                Text(value.isEmpty ? label : value)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(value.isEmpty ? Tone.secondary : .white.opacity(0.9))
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
            .padding(.horizontal, 8)
            .frame(height: 22)
            .background(Color.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(.white.opacity(value.isEmpty ? 0.08 : 0.16)))
            .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(inserts ? "Insert \(label) at the caret" : "Which \(label) the table list comes from")
    }

    private func insert(_ table: String) {
        guard let name = qualifiedName(database: tab.pickerDatabase.isEmpty ? nil : tab.pickerDatabase,
                                       schema: tab.pickerSchema.isEmpty ? nil : tab.pickerSchema,
                                       table: table, for: kind) else { return }
        tab.insertIntoSQL(name)
    }
}
