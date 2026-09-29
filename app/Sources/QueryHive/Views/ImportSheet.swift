import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The import mapping sheet: the source file's columns on the left, the target's on the right.
///
/// `import_data` reads `COLUMNS` as `[{source, target, include}]` and derives the mapping from the
/// header row when it is absent (`crates/qh-ffi/src/import.rs`). The engine is the importer; this
/// sheet only says which file, which target and which field goes where — and says out loud what it
/// cannot do rather than showing a control that does nothing. The three limits worth naming, all
/// stated in the sheet: the app cannot read an XLSX header, the engine does not create the target
/// table from the file's inferred types, and a `confirm` connection refuses an import outright.
struct ImportSheet: View {
    @Environment(AppModel.self) private var model
    @Bindable var draft: ImportDraft

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Tone.ink.opacity(0.08))
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    sourceSection
                    targetSection
                    mappingSection
                    policySection
                    limitsSection
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(height: 400)
            .scrollBounceBehavior(.basedOnSize)
            Divider().overlay(Tone.ink.opacity(0.08))
            footer
        }
        .frame(width: 660)
        .onAppear { reloadColumns() }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "square.and.arrow.down.on.square")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Tone.mint)
            VStack(alignment: .leading, spacing: 1) {
                Text("Import Data")
                    .font(.ui(13, weight: .semibold))
                    .foregroundStyle(Tone.ink)
                Text(draft.mapping.fileName.isEmpty ? "No file chosen" : draft.mapping.fileName)
                    .font(.code(10.5))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            Chip(text: draft.mapping.format.label, tint: Tone.mint)
        }
        .padding(14)
    }

    // MARK: Source

    private var sourceSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Source")
            HStack(spacing: 8) {
                Text(draft.mapping.path.isEmpty ? "Choose a CSV, TSV or XLSX file…" : draft.mapping.path)
                    .font(.code(11))
                    .foregroundStyle(draft.mapping.path.isEmpty ? Tone.coral : Tone.ink)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)
                PillButton(title: "Choose…", symbol: "folder", compact: true) { chooseFile() }
            }
            HStack(alignment: .top, spacing: 12) {
                LabeledField("Format") {
                    Segmented(selection: $draft.mapping.format,
                              options: ImportSourceFormat.allCases) { $0.label }
                }
                .frame(width: 190)
                if draft.mapping.format != .xlsx {
                    LabeledField("Delimiter") {
                        TextField(",", text: $draft.mapping.delimiter).field()
                            .frame(width: 70)
                    }
                }
            }
            .onChange(of: draft.mapping.format) { _, format in
                // A format carries its own default separator, and the user has not diverged yet.
                draft.mapping.delimiter = format.delimiter
                reloadColumns()
            }
            ChipToggle(label: "The first row holds the column names", isOn: $draft.mapping.header)
        }
    }

    // MARK: Target

    private var targetSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "Target")
            connectionPicker
            driverTargetFields
            HStack(spacing: 8) {
                if draft.loadingColumns {
                    ProgressView().controlSize(.mini)
                    Text("Reading the target's columns…").font(.ui(11)).foregroundStyle(Tone.secondary)
                } else if let error = draft.columnsError {
                    Text(error).font(.ui(11)).foregroundStyle(Tone.coral)
                        .lineLimit(2).truncationMode(.middle)
                } else if !draft.mapping.targetColumns.isEmpty {
                    Text(pluralized(draft.mapping.targetColumns.count, "column") + " in the target")
                        .font(.ui(11)).foregroundStyle(Tone.secondary)
                }
                Spacer(minLength: 0)
                PillButton(title: "Load Columns", symbol: "arrow.clockwise", compact: true) {
                    reloadColumns()
                }
                .disabled(draft.mapping.trimmedTable.isEmpty || draft.loadingColumns)
            }
            if draft.mapping.usesHeaderMapping, !draft.loadingColumns {
                Text("The target's own columns are not known, so the import maps each file column to "
                     + "the target column of the same name — the engine's own default. Name the "
                     + "table and press Load Columns to map field by field.")
                    .font(.ui(10.5))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var connectionPicker: some View {
        HStack(spacing: 8) {
            Text("Connection").font(.ui(11, weight: .semibold)).foregroundStyle(Tone.secondary)
            Menu {
                ForEach(model.connections) { connection in
                    Button("\(connection.name) · \(connection.kind.label)") {
                        draft.connectionID = connection.id
                        // A new connection is a new target: the old columns described the old one.
                        draft.mapping.targetColumns = []
                        draft.mapping.fields = ImportMapping.mappedFields(
                            headers: draft.mapping.fields.map(\.name), targetColumns: [])
                        reloadColumns()
                    }
                }
            } label: {
                Text(connectionName)
                    .font(.ui(12))
                    .frame(minWidth: 200, alignment: .leading)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
    }

    private var connectionName: String {
        guard let id = draft.connectionID,
              let connection = model.connections.first(where: { $0.id == id }) else {
            return "Choose a connection"
        }
        return "\(connection.name) · \(connection.kind.label)"
    }

    @ViewBuilder private var driverTargetFields: some View {
        if let kind = connectionKind {
            HStack(alignment: .top, spacing: 10) {
                switch kind {
                case .trino:
                    LabeledField("Catalog") {
                        ComboField(placeholder: "hive", text: $draft.mapping.targetCatalog,
                                   options: model.targetChoices(for: draft.connectionID, kind: .catalog),
                                   width: 150,
                                   loading: model.isLoadingOptions(for: draft.connectionID))
                    }
                    LabeledField("Schema") {
                        ComboField(placeholder: "analytics", text: $draft.mapping.targetSchema,
                                   options: model.targetChoices(for: draft.connectionID, kind: .schema,
                                                                database: draft.mapping.trimmedCatalog),
                                   width: 150,
                                   loading: model.isLoadingOptions(for: draft.connectionID,
                                                                   catalog: draft.mapping.trimmedCatalog))
                    }
                case .postgres:
                    LabeledField("Schema") {
                        ComboField(placeholder: "public", text: $draft.mapping.targetSchema,
                                   options: model.targetChoices(for: draft.connectionID, kind: .schema),
                                   width: 150,
                                   loading: model.isLoadingOptions(for: draft.connectionID))
                    }
                case .mysql:
                    LabeledField("Database") {
                        ComboField(placeholder: "mydb", text: $draft.mapping.targetCatalog,
                                   options: model.targetChoices(for: draft.connectionID, kind: .database),
                                   width: 150,
                                   loading: model.isLoadingOptions(for: draft.connectionID))
                    }
                }
                LabeledField("Table") {
                    TextField("new_table", text: $draft.mapping.targetTable).field()
                }
            }
            // Deliberately no `onChange` reload here: the table name is typed a character at a
            // time, and a preview round trip per keystroke is a network call nobody asked for.
            // "Load Columns" is the explicit way to ask, and the sheet loads once on appear.
        }
    }

    private var connectionKind: ConnectionKind? {
        guard let id = draft.connectionID else { return nil }
        return model.connections.first { $0.id == id }?.kind
    }

    // MARK: Mapping

    @ViewBuilder private var mappingSection: some View {
        if draft.mapping.format.appCanReadHeader {
            VStack(alignment: .leading, spacing: 8) {
                SectionLabel(text: "Fields")
                if draft.mapping.fields.isEmpty {
                    Text("Choose a file to read its columns.")
                        .font(.ui(11)).foregroundStyle(Tone.secondary)
                } else {
                    VStack(spacing: 0) {
                        ForEach($draft.mapping.fields) { $field in
                            ImportFieldRow(field: $field, targetColumns: draft.mapping.targetColumns)
                            if field.source < draft.mapping.fields.count - 1 {
                                Divider().overlay(Tone.ink.opacity(0.06))
                            }
                        }
                    }
                    .padding(.vertical, 4)
                    .background(Tone.recess.opacity(0.22),
                                in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            }
        } else {
            VStack(alignment: .leading, spacing: 6) {
                SectionLabel(text: "Fields")
                Text("This app cannot read an XLSX header without the engine, so there is no field list "
                     + "here. The import maps each workbook column to the target column of the same "
                     + "name, which is what the engine does when `COLUMNS` is absent.")
                    .font(.ui(11)).foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    // MARK: Policy

    private var policySection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel(text: "If a row is bad")
            Segmented(selection: $draft.mapping.onError, options: ImportErrorMode.allCases) { $0.title }
            Text(draft.mapping.onError.detail)
                .font(.ui(10.5)).foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(alignment: .top, spacing: 10) {
                LabeledField("Empty cell means") {
                    TextField("NULL", text: $draft.mapping.nullText).field().frame(width: 120)
                }
                LabeledField("Rows per INSERT") {
                    TextField("200", value: $draft.mapping.batchSize, format: .number.grouping(.never))
                        .field().frame(width: 90)
                }
            }
            ChipToggle(label: "Keep the server's foreign-key checks on", isOn: $draft.mapping.foreignKeys)
        }
    }

    // MARK: Limits

    private var limitsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let refusal = refusalReason {
                Label(refusal, systemImage: "exclamationmark.triangle.fill")
                    .font(.ui(11))
                    .foregroundStyle(Tone.amber)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text("The target table must already exist: this engine imports rows into a table, and "
                 + "creating one from the file's inferred types is not built (ADR-0019). Naming a "
                 + "table that is not there is a failed import, not a new table.")
                .font(.ui(10.5)).foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text("A `.sql` file is a statement import, not a row import, and is not offered here. "
                 + "The engine reads one through `import_data` on the command line or the MCP server.")
                .font(.ui(10.5)).foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var refusalReason: String? {
        ImportMapping.refusalReason(safeMode: connectionSafeMode)
    }

    private var connectionSafeMode: ConnectionSafeMode {
        guard let id = draft.connectionID,
              let connection = model.connections.first(where: { $0.id == id }) else { return .full }
        return connection.safeMode
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 8) {
            if draft.running {
                ProgressView().controlSize(.small)
                Text("Importing…").font(.ui(11)).foregroundStyle(Tone.secondary)
            } else if let outcome = draft.outcome {
                Label(ImportSheet.outcomeText(outcome), systemImage: "checkmark.circle.fill")
                    .font(.ui(11)).foregroundStyle(Tone.mint)
            } else if let failure = draft.failure {
                Text(failure).font(.ui(11)).foregroundStyle(Tone.coral)
                    .lineLimit(2).truncationMode(.middle)
            }
            Spacer(minLength: 0)
            if draft.outcome != nil || draft.failure != nil {
                PillButton(title: "Close", action: dismiss)
            } else {
                PillButton(title: "Cancel", role: .quiet, action: dismiss)
                HubButton(title: "Import", symbol: "square.and.arrow.down", hue: .exporter) {
                    model.runImport(draft)
                }
                .disabled(!canImport)
            }
        }
        .padding(14)
    }

    private var canImport: Bool {
        draft.mapping.ready && !draft.running && refusalReason == nil
    }

    /// What the engine reported, in the words a person reads.
    static func outcomeText(_ outcome: ImportOutcome) -> String {
        var text = "Imported \(pluralized(outcome.rows, "row")) into the target"
        if outcome.rejected > 0 {
            text += " · \(pluralized(outcome.rejected, "row")) rejected"
        }
        if let stoppedAt = outcome.stoppedAt {
            text += " · stopped at line \(stoppedAt)"
        }
        if !outcome.transaction, outcome.disposition == "written" {
            text += " · no transaction (this driver cannot roll back)"
        }
        return text
    }

    // MARK: Actions

    private func dismiss() {
        model.importDraft = nil
    }

    /// Rebuilds the mapping from the file's header and asks the server for the target's columns.
    private func reloadColumns() {
        guard let id = draft.connectionID else { return }
        refreshFieldsFromFile()
        guard let qualified = qualifiedTarget() else {
            draft.mapping = draft.mapping.remapped(to: [])
            return
        }
        draft.loadingColumns = true
        draft.columnsError = nil
        model.loadImportColumns(connectionID: id, qualifiedName: qualified) { [draft] columns in
            draft.loadingColumns = false
            guard !columns.isEmpty else {
                draft.columnsError = "The target did not report any columns. It may not exist yet, "
                    + "or the driver cannot describe it before a row arrives."
                return
            }
            draft.mapping = draft.mapping.remapped(to: columns)
        }
    }

    /// Reads the file's own header, when the format allows it, and rebuilds the field list.
    private func refreshFieldsFromFile() {
        guard draft.mapping.format.appCanReadHeader, !draft.mapping.path.isEmpty else { return }
        let delimiter: Character
        if draft.mapping.delimiter.count == 1, let only = draft.mapping.delimiter.first {
            delimiter = only
        } else {
            delimiter = draft.mapping.format == .tsv ? "\t" : ","
        }
        do {
            let headers = try ImportHeaderReader.readHeaders(path: draft.mapping.path, delimiter: delimiter)
            guard !headers.isEmpty else { return }
            draft.mapping.fields = ImportMapping.mappedFields(headers: headers,
                                                              targetColumns: draft.mapping.targetColumns)
            draft.columnsError = nil
        } catch {
            draft.columnsError = error.localizedDescription
        }
    }

    private func qualifiedTarget() -> String? {
        let table = draft.mapping.trimmedTable
        guard !table.isEmpty, let kind = connectionKind else { return nil }
        return qualifiedName(database: draft.mapping.trimmedCatalog.isEmpty ? nil : draft.mapping.trimmedCatalog,
                             schema: draft.mapping.trimmedSchema.isEmpty ? nil : draft.mapping.trimmedSchema,
                             table: table, for: kind) ?? table
    }

    private func chooseFile() {
        let panel = NSOpenPanel()
        panel.title = "Import Data from File"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = ["csv", "tsv", "xlsx"].compactMap { UTType(filenameExtension: $0) }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        model.configure(draft, for: url)
        reloadColumns()
    }
}

/// One source field, its include toggle, and the target column it writes.
private struct ImportFieldRow: View {
    @Binding var field: ImportField
    let targetColumns: [String]

    var body: some View {
        HStack(spacing: 8) {
            Toggle("", isOn: $field.include)
                .toggleStyle(.checkbox)
                .labelsHidden()
                .help(field.include ? "Write this column" : "Leave this column out")
            Text(field.name)
                .font(.code(11.5))
                .foregroundStyle(field.include ? Tone.ink : Tone.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Image(systemName: "arrow.right")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.secondary)
            Menu {
                Button("Column named “\(field.name)” (by header)") { field.target = "" }
                if !targetColumns.isEmpty {
                    Divider()
                    ForEach(targetColumns, id: \.self) { column in
                        Button(column) { field.target = column }
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text(mappingLabel)
                        .font(.code(11))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(Tone.secondary)
                }
                .frame(width: 220, alignment: .leading)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(!field.include)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .opacity(field.include ? 1 : 0.55)
    }

    private var mappingLabel: String {
        guard !targetColumns.isEmpty else { return field.effectiveTarget }
        if field.target.isEmpty { return "“\(field.effectiveTarget)” (by header)" }
        guard targetColumns.contains(field.target) else { return "\(field.target) — not in the target" }
        return field.target
    }
}
