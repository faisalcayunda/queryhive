import AppKit
import SwiftUI

/// Everything about where an export goes, behind one button.
///
/// These choices used to be scattered across the toolbar — a File/Table switch, a format menu, a
/// folder chip, a name field — which made the toolbar a settings panel that happened to have a Run
/// button on it, and left the export action itself nowhere in particular. Navicat puts the same
/// choices in the wizard its Export command opens; this is that wizard, sized for a popover.
struct ExportSettingsButton: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

    private var showing: Binding<Bool> {
        Binding(get: { model.exportSettingsOpen }, set: { model.exportSettingsOpen = $0 })
    }

    private var blocked: String? { model.runBlockedReason(for: tab) }
    private var disabled: Bool { blocked != nil || tab.stage == .running }

    var body: some View {
        HStack(spacing: 2) {
            HubButton(title: tab.destination == .table ? "Save" : "Export",
                      symbol: tab.destination == .table ? "square.and.arrow.down" : "arrow.down.doc",
                      hue: tab.isDestructive ? .failure : .exporter) {
                if tab.isDestructive { confirmReplace = true } else { model.run(tab) }
            }
            .disabled(disabled)
            .keyboardShortcut("e", modifiers: .command)
            .help(blocked ?? summary)

            Button { showing.wrappedValue.toggle() } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(disabled ? .white.opacity(0.3) : .white.opacity(0.75))
                    .frame(width: 18, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(width: 18)
            .help("Where it goes, and in what format")
            .popover(isPresented: showing, arrowEdge: .bottom) { panel }
        }
        .confirmationDialog("Replace \(tab.target(for: model.connection(for: tab)?.kind ?? .trino))?",
                            isPresented: $confirmReplace) {
            Button("Drop and Recreate", role: .destructive) { model.run(tab) }
        } message: {
            Text("The existing table is dropped before the query runs. If the query then fails, the table is already gone.")
        }
    }

    /// One line naming what pressing Export will do, used as the tooltip and as the popover's
    /// heading — so the button never has to be pressed to find out.
    private var summary: String {
        switch tab.destination {
        case .file:
            let folder = tab.outputDirectory?.lastPathComponent ?? "no folder"
            return "\(tab.format.label) → \(folder)/\(tab.trimmedName)"
        case .table:
            return "\(tab.writeMode.statement) \(tab.target(for: model.connection(for: tab)?.kind ?? .trino))"
        }
    }

    private var panel: some View {
        @Bindable var tab = tab
        return VStack(alignment: .leading, spacing: 12) {
            SectionLabel(text: "Destination")
            Segmented(selection: $tab.destination, options: [Destination.file, .table]) { $0.label }
            Text(summary)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Tone.secondary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)

            Divider().overlay(.white.opacity(0.08))

            switch tab.destination {
            case .file:
                FormatGrid(selection: $tab.format)
                Divider().overlay(.white.opacity(0.08))
                FormatOptionsPanel(tab: tab)
                Divider().overlay(.white.opacity(0.08))
                StreamingOptions(tab: tab)
                Divider().overlay(.white.opacity(0.08))
                LabeledField("Folder") {
                    HStack(spacing: 8) {
                        Text(tab.outputDirectory?.path ?? "No folder chosen")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(tab.outputDirectory == nil ? Tone.coral : .white)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        PillButton(title: "Choose…", symbol: "folder", compact: true) { chooseFolder() }
                    }
                }
                LabeledField("File name") {
                    TextField("export", text: $tab.outputName).field()
                }
            case .table:
                TableTargetFields(tab: tab)
            }
        }
        .padding(14)
        .frame(width: 400)
        .onAppear {
            model.loadCatalogs(for: tab.connectionID)
            model.loadSchemas(for: tab.connectionID, catalog: tab.trimmedCatalog)
        }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        if let directory = tab.outputDirectory { panel.directoryURL = directory }
        if panel.runModal() == .OK, let url = panel.url { tab.outputDirectory = url }
    }
}

/// The table destination's fields: catalog/schema/table and what to do if it already exists.
/// Moved out of `TableTargetButton` so the popover can own the whole destination at once rather
/// than nesting a second popover inside the first.
struct TableTargetFields: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    private var kind: ConnectionKind { model.connection(for: tab)?.kind ?? .trino }

    private var catalogs: [String] {
        model.targetChoices(for: tab.connectionID, kind: kind == .mysql ? .database : .catalog)
    }
    private var schemas: [String] {
        model.targetChoices(for: tab.connectionID, kind: .schema, database: tab.trimmedCatalog)
    }

    var body: some View {
        @Bindable var tab = tab
        return VStack(alignment: .leading, spacing: 12) {
            switch kind {
            case .trino:
                LabeledField("Catalog") {
                    ComboField(placeholder: "hive", text: $tab.targetCatalog, options: catalogs, width: 372,
                               loading: model.isLoadingOptions(for: tab.connectionID))
                }
                LabeledField("Schema") {
                    ComboField(placeholder: "analytics", text: $tab.targetSchema, options: schemas, width: 372,
                               loading: model.isLoadingOptions(for: tab.connectionID, catalog: tab.trimmedCatalog))
                }
            case .postgres:
                LabeledField("Schema") {
                    ComboField(placeholder: "public", text: $tab.targetSchema, options: schemas, width: 372,
                               loading: model.isLoadingOptions(for: tab.connectionID))
                }
            case .mysql:
                LabeledField("Database") {
                    ComboField(placeholder: "mydb", text: $tab.targetCatalog, options: catalogs, width: 372,
                               loading: model.isLoadingOptions(for: tab.connectionID))
                }
            }
            LabeledField("Table") {
                TextField("penerima_manfaat_2026", text: $tab.targetTable).field()
            }
            Divider().overlay(.white.opacity(0.08))
            SectionLabel(text: "If the table already exists")
            ForEach(WriteMode.allCases) { mode in
                Button { tab.writeMode = mode } label: {
                    HStack(alignment: .top, spacing: 9) {
                        Image(systemName: tab.writeMode == mode ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 12))
                            .foregroundStyle(tab.writeMode == mode ? mode.tint : .white.opacity(0.3))
                            .padding(.top, 1)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(mode.statement)
                                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                                .foregroundStyle(mode == .replace ? Tone.coral : .white)
                            Text(mode.help)
                                .font(.system(size: 11))
                                .foregroundStyle(Tone.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .onChange(of: tab.targetCatalog) { _, catalog in
            model.loadSchemas(for: tab.connectionID, catalog: catalog.trimmingCharacters(in: .whitespaces))
        }
    }
}
