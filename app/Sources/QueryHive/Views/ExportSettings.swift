import AppKit
import SwiftUI

/// Everything about where an export goes, behind one button.
///
/// These choices used to be scattered across the toolbar — a File/Table switch, a format menu, a
/// folder chip, a name field — which made the toolbar a settings panel that happened to have a Run
/// button on it, and left the export action itself nowhere in particular. Navicat puts the same
/// choices in the wizard its Export command opens; this is that wizard, sized for a popover.
/// The destination, as a popover with no button of its own.
///
/// It used to own a toolbar button; that button's command moved into Run's menu, and all that is
/// left here is where the settings show. The chevron beside Run opens it, and so does
/// "Export Settings…".
struct DestinationPopover: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

    private var showing: Binding<Bool> {
        Binding(get: { model.exportSettingsOpen }, set: { model.exportSettingsOpen = $0 })
    }

    /// Why the wizard's own button is dim — the same reason the old toolbar button was, now that
    /// this is where the decision is confirmed.
    private var blocked: String? { model.runBlockedReason(for: tab) }

    var body: some View {
        // A zero-size anchor: the popover has to hang off something, and the Run group is what it
        // describes (the footer's corner while the rows fill the window; see `Workspace`).
        Color.clear
            .frame(width: 1, height: 1)
            .popover(isPresented: showing, arrowEdge: .bottom) { panel }
            // Here rather than on the Run group: this view is mounted in both layouts, the Run
            // group only while the editor shows.
            .onChange(of: tab.destination) { _, destination in
                if destination == .table { model.prepareTableDestination(tab) }
            }
            .confirmationDialog("Replace \(tab.target(for: targetKind))?",
                                isPresented: $confirmReplace) {
                Button("Replace Table", role: .destructive) { commit() }
            } message: {
                Text("The new table is built beside the existing one and takes its name once every row is there; the existing table, with its keys, indexes and grants, is dropped then. If the query fails first, the existing table stays as it was.")
            }
    }

    /// The driver the target name is written for: the target connection's, when the table goes to
    /// another connection (W13-T20), otherwise this tab's own.
    private var targetKind: ConnectionKind {
        model.tableDestination(for: tab)?.connection?.kind ?? model.connection(for: tab)?.kind ?? .trino
    }

    /// The other connection a table is copied to, when it is one. Its own transfer sheet asks about
    /// a replace, so the dialog above is not asked as well.
    private var transferTarget: Connection? {
        guard tab.destination == .table, case .other(let target) = model.tableDestination(for: tab) else {
            return nil
        }
        return target
    }

    /// Runs it and closes the wizard, so a successful export does not leave a panel sitting over
    /// the result it just produced.
    private func commit() {
        model.exportSettingsOpen = false
        model.run(tab)
    }

    /// A scrolling body between two pinned ends.
    ///
    /// The wizard is taller than a laptop screen's worth of popover once a format brings its own
    /// options, and the first version simply grew until Cancel and Export fell off the bottom —
    /// the two controls you need most were the ones that disappeared. The destination stays at the
    /// top and the buttons at the bottom; only the middle moves.
    private var panel: some View {
        @Bindable var tab = tab
        return VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                SectionLabel(text: "Destination")
                Segmented(selection: $tab.destination, options: [Destination.file, .table]) { $0.label }
                Text(summary)
                    .font(.code(11))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)

            Divider().overlay(Tone.ink.opacity(0.08))

            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    switch tab.destination {
                    case .file:
                        FormatGrid(selection: $tab.format)
                        Divider().overlay(Tone.ink.opacity(0.08))
                        FormatOptionsPanel(tab: tab)
                        Divider().overlay(Tone.ink.opacity(0.08))
                        StreamingOptions(tab: tab)
                        Divider().overlay(Tone.ink.opacity(0.08))
                        LabeledField("Folder") {
                            HStack(spacing: 8) {
                                Text(tab.outputDirectory?.path ?? "No folder chosen")
                                    .font(.code(11))
                                    .foregroundStyle(tab.outputDirectory == nil ? Tone.coral : Tone.ink)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                PillButton(title: "Choose…", symbol: "folder", compact: true) { chooseFolder() }
                            }
                        }
                        LabeledField("File name") { TextField("export", text: $tab.outputName).field() }
                    case .table:
                        TableTargetFields(tab: tab)
                    }
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            // Tall enough for the format grid plus one options block; past that the wizard scrolls
            // rather than growing off the screen.
            .frame(height: 396)
            .scrollBounceBehavior(.basedOnSize)

            Divider().overlay(Tone.ink.opacity(0.08))

            HStack(spacing: 8) {
                if let blocked {
                    Text(blocked).font(.ui(11)).foregroundStyle(Tone.coral)
                        .lineLimit(1).truncationMode(.middle)
                }
                Spacer(minLength: 0)
                PillButton(title: "Cancel", role: .quiet) { model.exportSettingsOpen = false }
                HubButton(title: tab.destination == .table ? "Save to Table" : "Export",
                          symbol: tab.destination == .table ? "square.and.arrow.down" : "arrow.down.doc",
                          hue: tab.isDestructive ? .failure : .exporter) {
                    if tab.isDestructive, transferTarget == nil { confirmReplace = true } else { commit() }
                }
                .disabled(blocked != nil || tab.stage == .running)
            }
            .padding(14)
        }
        .frame(width: 400)
        .onAppear {
            // The target's catalogs and schemas, which are this tab's own unless a copy names
            // another connection.
            let target = model.tableDestination(for: tab)?.connection?.id ?? tab.connectionID
            model.loadCatalogs(for: target)
            model.loadSchemas(for: target, catalog: tab.trimmedCatalog)
        }
    }

    private var summary: String {
        switch tab.destination {
        case .file:
            let folder = tab.outputDirectory?.lastPathComponent ?? "no folder"
            return "\(tab.format.label) → \(folder)/\(tab.trimmedName)"
        case .table:
            let line = "\(tab.writeMode.statement) \(tab.target(for: targetKind))"
            guard let transferTarget else { return line }
            return "\(line) on \(transferTarget.name)"
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

    /// The tab's own connection, which the query reads from.
    private var source: Connection? { model.connection(for: tab) }
    private var destination: TableDestination? { model.tableDestination(for: tab) }
    /// The connection the fields describe: the target when one resolves, otherwise the tab's own.
    private var target: Connection? { destination?.connection ?? source }
    private var targetID: UUID? { target?.id ?? tab.connectionID }
    private var kind: ConnectionKind { target?.kind ?? .trino }
    /// The other connection a copy across connections writes to (W13-T20).
    private var crossTarget: Connection? {
        if case .other(let connection) = destination { return connection }
        return nil
    }
    /// The experimental switch (ADR-0046 decision 13). Off, these fields are exactly what they
    /// were: no connection picker, no "exists" hint, no cross-connection block.
    private var acrossConnections: Bool { model.experiments.saveTableAcrossConnections }

    private var catalogs: [String] {
        model.targetChoices(for: targetID, kind: kind == .mysql ? .database : .catalog)
    }
    private var schemas: [String] {
        model.targetChoices(for: targetID, kind: .schema, database: tab.trimmedCatalog)
    }

    /// Append is not offered across connections (OQ-5).
    private var modes: [WriteMode] { crossTarget == nil ? WriteMode.allCases : [.create, .replace] }

    var body: some View {
        @Bindable var tab = tab
        return VStack(alignment: .leading, spacing: 12) {
            if acrossConnections { connectionPicker }
            switch kind {
            case .trino:
                LabeledField("Catalog") {
                    ComboField(placeholder: "hive", text: $tab.targetCatalog, options: catalogs, width: 372,
                               loading: model.isLoadingOptions(for: targetID))
                }
                LabeledField("Schema") {
                    ComboField(placeholder: "analytics", text: $tab.targetSchema, options: schemas, width: 372,
                               loading: model.isLoadingOptions(for: targetID, catalog: tab.trimmedCatalog))
                }
            case .postgres:
                LabeledField("Schema") {
                    ComboField(placeholder: "public", text: $tab.targetSchema, options: schemas, width: 372,
                               loading: model.isLoadingOptions(for: targetID))
                }
            case .mysql:
                LabeledField("Database") {
                    ComboField(placeholder: "mydb", text: $tab.targetCatalog, options: catalogs, width: 372,
                               loading: model.isLoadingOptions(for: targetID))
                }
            }
            LabeledField("Table") {
                TextField("penerima_manfaat_2026", text: $tab.targetTable).field()
                if acrossConnections, let collision = model.tableCollision(for: tab) {
                    Label(collision, systemImage: "exclamationmark.triangle.fill")
                        .font(.ui(11))
                        .foregroundStyle(tab.writeMode == .create ? Tone.coral : Tone.amber)
                }
            }
            Divider().overlay(Tone.ink.opacity(0.08))
            SectionLabel(text: "If the table already exists")
            ForEach(modes) { mode in modeRow(mode) }
            if let crossTarget, let source {
                CrossConnectionNotes(source: source, target: crossTarget)
            }
        }
        .onChange(of: tab.targetCatalog) { _, catalog in
            model.loadSchemas(for: targetID, catalog: catalog.trimmingCharacters(in: .whitespaces))
            loadTables()
        }
        .onChange(of: tab.targetSchema) { _, _ in loadTables() }
        .onChange(of: tab.targetConnectionID) { _, _ in
            model.loadCatalogs(for: targetID)
            model.loadSchemas(for: targetID, catalog: tab.trimmedCatalog)
            loadTables()
        }
        .onAppear { loadTables() }
    }

    /// The target namespace's tables, for the "exists" hint. Only with the switch on: off, opening
    /// the wizard asks the server for nothing it did not ask for before.
    private func loadTables() {
        guard acrossConnections else { return }
        model.loadTableOptions(for: targetID, catalog: tab.trimmedCatalog, schema: tab.trimmedSchema)
    }

    // MARK: The target connection

    /// Which connection the table is written to. A `Menu` with a fixed width rather than a popover:
    /// this already is one, and a popover inside a popover is the shape `TableTargetButton` left.
    private var connectionPicker: some View {
        LabeledField("Connection") {
            Menu {
                Button(sameTitle) { model.chooseTargetConnection(nil, for: tab) }
                Divider()
                ForEach(model.connections.filter { $0.id != tab.connectionID }) { connection in
                    Button {
                        model.chooseTargetConnection(connection.id, for: tab)
                    } label: {
                        Label(itemTitle(connection), systemImage: connection.kind.symbol)
                    }
                    // Trino is written to only by its own coordinator (blueprint §11).
                    .disabled(connection.kind == .trino)
                }
            } label: {
                HStack(spacing: 7) {
                    if let target {
                        connectionTile(colour: target.color, kind: target.kind, size: 16)
                            .accessibilityHidden(true)
                    }
                    Text(pickerTitle)
                        .font(.ui(12))
                        .foregroundStyle(destination == .missing ? Tone.coral : Tone.ink)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(Tone.secondary)
                }
                .padding(.horizontal, 9)
                .frame(width: 372, height: Metrics.control)
                .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(Tone.ink.opacity(0.10)))
                .contentShape(Rectangle())
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
            .frame(width: 372)
            .accessibilityLabel("Target connection")
            .accessibilityValue(pickerTitle)
        }
    }

    private var sameTitle: String { "Same connection (\(source?.name ?? "this tab's"))" }

    private var pickerTitle: String {
        switch destination {
        case .other(let connection): itemTitle(connection)
        case .missing: "Choose the target connection again"
        case .same, .switchedOff, .none: sameTitle
        }
    }

    /// One connection as the menu lists it: name, driver, label, and the Safe Mode that will decide
    /// what may be written there.
    private func itemTitle(_ connection: Connection) -> String {
        var words = [connection.name, connection.kind.label]
        if let environment = connection.environment { words.append(environment.title) }
        words.append(connection.kind == .trino ? "not a target from another connection"
                                               : "Safe Mode \(connection.safeMode.title)")
        return words.joined(separator: " · ")
    }

    // MARK: Modes

    /// One write mode. A mode the target's Safe Mode refuses is shown and greyed, with the sentence
    /// that says why in place of its help (blueprint §8.1).
    private func modeRow(_ mode: WriteMode) -> some View {
        let refusal = crossTarget.flatMap { TableDestination.safeModeRefusal(target: $0, mode: mode) }
        let selected = tab.writeMode == mode
        return HStack(alignment: .top, spacing: 6) {
            Button { tab.writeMode = mode } label: {
                HStack(alignment: .top, spacing: 9) {
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 12))
                        .foregroundStyle(selected ? mode.tint : Tone.ink.opacity(0.3))
                        .padding(.top, 1)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(mode.statement)
                            .font(.code(11.5, weight: .semibold))
                            .foregroundStyle(mode == .replace ? Tone.coral : Tone.ink)
                        Text(refusal ?? mode.help)
                            .font(.ui(11))
                            .foregroundStyle(refusal == nil ? Tone.secondary : Tone.amber)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
                .opacity(refusal == nil || selected ? 1 : 0.55)
            }
            .buttonStyle(.plain)
            .disabled(refusal != nil && !selected)
            // A tooltip on hover; the popover only on a click, because this row is already inside
            // the wizard's popover.
            if acrossConnections, let detail = mode.detail {
                HelpHint(text: detail)
            }
        }
    }
}

/// What a copy across connections adds to the wizard, inside its scrolling body (blueprint §8.1
/// item 5): who reads and who writes, the bound the source runs under, and the two banners.
private struct CrossConnectionNotes: View {
    let source: Connection
    let target: Connection

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider().overlay(Tone.ink.opacity(0.08))
            SectionLabel(text: "Across connections")
            note("Rows are read from \(source.name) and written to \(target.name) by QueryHive.")
            note("The new table has the query's columns only.")
            note(sourceBound)
            ForEach([source, target], id: \.id) { connection in
                let tls = ConnectionTLSChoice(kind: connection.kind, scheme: connection.scheme,
                                              verify: connection.verify, sslmode: connection.sslmode)
                if tls.title.hasSuffix("· unverified") {
                    TransferBanner(symbol: "lock.open", tint: Tone.amber,
                                   text: "\(connection.name)'s certificate is not checked (\(tls.title)).")
                }
            }
            if source.environment == .prod, target.environment != .prod {
                TransferBanner(symbol: "exclamationmark.triangle.fill", tint: Tone.coral,
                               text: "\(source.name) is labelled Prod and \(target.name) is not. "
                                   + "The copy asks you to confirm it.")
            }
        }
    }

    /// The statement bound the source reads under for this copy (blueprint §6.6, OQ-13).
    private var sourceBound: String {
        guard let own = source.statementTimeoutMS else {
            return "\(source.name) has no statement bound of its own, so none is set for this copy; "
                + "a bound set on the server still applies."
        }
        if own == 0 { return "\(source.name) sets no statement bound." }
        return "\(source.name) stops a statement after \(Self.duration(own)), its own setting: "
            + "a copy that reads for longer is cut off."
    }

    private static func duration(_ milliseconds: Int) -> String {
        milliseconds % 1000 == 0 ? "\((milliseconds / 1000).formatted()) s" : "\(milliseconds.formatted()) ms"
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.ui(11))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A tinted warning, shared by the wizard and the transfer sheet.
struct TransferBanner: View {
    let symbol: String
    let tint: Color
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Image(systemName: symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(tint)
                .padding(.top, 1)
                .accessibilityHidden(true)
            Text(text)
                .font(.ui(11))
                .foregroundStyle(Tone.ink.opacity(0.9))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(tint.opacity(0.35)))
    }
}
