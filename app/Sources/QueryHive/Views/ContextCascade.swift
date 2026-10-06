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
                    .font(.code(11.5))
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

// MARK: Badges

/// One badge as a value: what it draws, what it says, and what VoiceOver reads, so a test can
/// check all of it without a window.
///
/// Information, not decoration: the glyph and the words tell the badges apart, and the hue only
/// strengthens them (colour alone fails contrast on the light canvas).
struct BadgeSpec: Equatable, Identifiable {
    enum Hue: Equatable { case ink, mint, amber, coral }
    /// Where the badge is drawn. The status bar always shows both; the breadcrumb and the tab
    /// chip stay quiet for an unrestricted connection with no tag, because no badge means no limit.
    enum Place { case statusBar, breadcrumb, tabChip }

    let glyph: String
    let label: String
    let hue: Hue
    /// What VoiceOver reads: "Environment: production", "Safe Mode: Confirm".
    let accessibility: String
    /// The consequence, as a hint. Empty for an environment, which has none.
    let detail: String

    var id: String { accessibility }

    static func environment(_ environment: ConnectionEnvironment) -> BadgeSpec {
        switch environment {
        case .dev: BadgeSpec(glyph: "hammer", label: "DEV", hue: .mint,
                             accessibility: "Environment: development", detail: "")
        case .staging: BadgeSpec(glyph: "testtube.2", label: "STAGING", hue: .amber,
                                 accessibility: "Environment: staging", detail: "")
        case .prod: BadgeSpec(glyph: "exclamationmark.octagon.fill", label: "PROD", hue: .coral,
                              accessibility: "Environment: production", detail: "")
        }
    }

    static func safeMode(_ mode: ConnectionSafeMode) -> BadgeSpec {
        let glyph = switch mode {
        case .full: "lock.open"
        case .noDDL: "lock.shield"
        case .confirm: "shield.lefthalf.filled"
        case .readOnly: "lock.fill"
        }
        return BadgeSpec(glyph: glyph, label: mode.title, hue: .ink,
                         accessibility: "Safe Mode: \(mode.title)", detail: mode.detail)
    }

    /// The badges for a connection at one place, environment first.
    static func badges(for connection: Connection?, in place: Place) -> [BadgeSpec] {
        guard let connection else { return [] }
        var specs: [BadgeSpec] = []
        if let environment = connection.environment { specs.append(.environment(environment)) }
        if place == .statusBar || connection.safeMode != .full {
            specs.append(.safeMode(connection.safeMode))
        }
        return specs
    }
}

/// The badges of one connection at one place. With `glyphOnly` the words go and the glyph stays,
/// with the full label still read aloud and shown as a tooltip.
struct ConnectionBadges: View {
    let specs: [BadgeSpec]
    var glyphOnly = false

    var body: some View {
        HStack(spacing: 5) {
            ForEach(specs) { spec in
                let tint = Self.tint(spec.hue)
                HStack(spacing: 4) {
                    Image(systemName: spec.glyph).font(.system(size: 10, weight: .semibold))
                    if !glyphOnly {
                        Text(spec.label).font(.code(11, weight: .semibold)).lineLimit(1)
                    }
                }
                .foregroundStyle(spec.hue == .ink ? Tone.ink.opacity(0.85) : tint)
                .padding(.horizontal, glyphOnly ? 6 : 8)
                .padding(.vertical, 3)
                .background(tint.opacity(0.14), in: Capsule())
                .fixedSize()
                .help(spec.detail.isEmpty ? spec.accessibility : "\(spec.accessibility). \(spec.detail)")
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(spec.accessibility)
                .accessibilityHint(spec.detail)
            }
        }
    }

    private static func tint(_ hue: BadgeSpec.Hue) -> Color {
        switch hue {
        case .ink: Tone.ink
        case .mint: Tone.markMint
        case .amber: Tone.markAmber
        case .coral: Tone.markCoral
        }
    }
}

/// The breadcrumb's badges, between the levels and the Run group. They drop their words before
/// they would push anything, so the three fixed-width levels never move from tab to tab.
struct BreadcrumbBadges: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab

    var body: some View {
        let specs = BadgeSpec.badges(for: model.connection(for: tab), in: .breadcrumb)
        if !specs.isEmpty {
            ViewThatFits(in: .horizontal) {
                ConnectionBadges(specs: specs)
                ConnectionBadges(specs: specs, glyphOnly: true)
            }
        }
    }
}
