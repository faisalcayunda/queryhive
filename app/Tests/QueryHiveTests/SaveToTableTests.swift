import XCTest

@testable import QueryHive

/// "Save to Table" asks for exactly the target parts the connection's driver has: Trino a catalog,
/// a schema and a table; PostgreSQL a schema and a table (the database is the connection); MySQL a
/// database (kept in `targetCatalog`) and a table. The rule used to demand all three for every
/// driver, so on PostgreSQL and MySQL the button stayed dim for a field the wizard never showed.
@MainActor
final class SaveToTableTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        useMemorySecretStore()
    }

    private func connection(_ kind: ConnectionKind, database: String = "", schema: String = "") -> Connection {
        Connection(id: UUID(), name: "Dev", color: .violet, kind: kind, host: "localhost",
                   port: kind.defaultPort, sslmode: "", user: "qh", database: database, schema: schema,
                   verify: false)
    }

    /// A model holding one connection of `kind` and a table-destination tab with a statement.
    private func fixture(_ kind: ConnectionKind, database: String = "", schema: String = "")
        -> (AppModel, QueryTab, Connection) {
        let model = AppModel()
        model.engine = MockEngine()
        let connection = connection(kind, database: database, schema: schema)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = "SELECT 1 AS id"
        tab.destination = .table
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab, connection)
    }

    // MARK: The rule, driver by driver

    func testTrinoNeedsACatalogASchemaAndATable() {
        let (model, tab, _) = fixture(.trino)
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target catalog")
        tab.targetCatalog = "hive"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target schema")
        tab.targetSchema = "analytics"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target table")
        tab.targetTable = "people"
        XCTAssertNil(model.runBlockedReason(for: tab))
    }

    func testPostgresNeedsASchemaAndATableAndNoCatalog() {
        let (model, tab, _) = fixture(.postgres)
        tab.targetTable = "people"
        // The wizard shows a schema field and nothing else: the catalog stays empty for good.
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target schema")
        tab.targetSchema = "public"
        XCTAssertNil(model.runBlockedReason(for: tab), "a PostgreSQL target has no catalog part")
        tab.targetTable = ""
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target table")
    }

    func testMySQLNeedsADatabaseAndATableAndNoSchema() {
        let (model, tab, _) = fixture(.mysql)
        tab.targetTable = "people"
        // The field is the wizard's "Database", stored in `targetCatalog`; the reason uses its word.
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target database")
        tab.targetCatalog = "shop"
        XCTAssertNil(model.runBlockedReason(for: tab), "a MySQL target has no schema part")
        tab.targetTable = ""
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target table")
    }

    func testAPartTheDriverDoesNotHaveIsNeverRequiredEvenWhenFilled() {
        // A tab that was a Trino target before its connection changed carries the other field.
        let (pg, pgTab, _) = fixture(.postgres)
        pgTab.targetCatalog = "hive"
        pgTab.targetSchema = "public"
        pgTab.targetTable = "people"
        XCTAssertNil(pg.runBlockedReason(for: pgTab))

        let (my, myTab, _) = fixture(.mysql)
        myTab.targetSchema = "analytics"
        myTab.targetCatalog = "shop"
        myTab.targetTable = "people"
        XCTAssertNil(my.runBlockedReason(for: myTab))
    }

    func testWhitespaceDoesNotCountAsAName() {
        let (model, tab, _) = fixture(.mysql)
        tab.targetCatalog = "   "
        tab.targetTable = "people"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target database")
    }

    func testTheFileDestinationIsUnchanged() {
        let (model, tab, _) = fixture(.postgres)
        tab.destination = .file
        tab.outputName = "export"
        tab.outputDirectory = nil
        XCTAssertEqual(model.runBlockedReason(for: tab), "Choose an output folder")
        tab.outputDirectory = URL(fileURLWithPath: "/tmp")
        XCTAssertNil(model.runBlockedReason(for: tab), "no target part is looked at for a file")
        tab.outputName = " "
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the output file")
    }

    // MARK: From the wizard's defaults to a runnable target

    func testSwitchingToTableOnPostgresLeavesNothingBlockedButTheTableName() {
        let (model, tab, _) = fixture(.postgres, database: "app", schema: "public")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        XCTAssertEqual(tab.targetSchema, "public")
        XCTAssertEqual(tab.targetCatalog, "", "PostgreSQL has no catalog field to default")
        XCTAssertEqual(tab.targetTable, "people_copy")
        XCTAssertNil(model.runBlockedReason(for: tab))
    }

    func testSwitchingToTableOnMySQLLeavesNothingBlockedButTheTableName() {
        let (model, tab, _) = fixture(.mysql, database: "shop", schema: "")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        XCTAssertEqual(tab.targetCatalog, "shop")
        XCTAssertEqual(tab.targetSchema, "", "MySQL has no schema field to default")
        XCTAssertEqual(tab.targetTable, "people_copy")
        XCTAssertNil(model.runBlockedReason(for: tab))
    }

    func testSwitchingToTableOnTrinoStillDefaultsBothParts() {
        let (model, tab, _) = fixture(.trino, database: "hive", schema: "analytics")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        XCTAssertEqual(tab.targetCatalog, "hive")
        XCTAssertEqual(tab.targetSchema, "analytics")
        XCTAssertNil(model.runBlockedReason(for: tab))
    }

    func testAnEmptyDefaultIsTheUsersToFillAndTheReasonSaysWhich() {
        let (model, tab, _) = fixture(.mysql, database: "", schema: "")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target database")
    }

    // MARK: What the engine is sent

    /// `run` hands `overrides` straight to the engine's `to_table`, so this is the environment the
    /// engine receives. (`run` itself reaches `Engine.current`, a `static let` no test can swap.)
    func testThePostgresRunSendsASchemaAndTableTheEngineCanQualify() throws {
        let (model, tab, connection) = fixture(.postgres, database: "app", schema: "public")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        tab.writeMode = .create
        let env = try model.overrides(for: tab, connection: connection, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["DB_KIND"], "postgres")
        XCTAssertEqual(env["TARGET_SCHEMA"], "public")
        XCTAssertEqual(env["TARGET_TABLE"], "people_copy")
        XCTAssertEqual(env["WRITE_MODE"], "create")
        XCTAssertEqual(env["TARGET_CATALOG"], "", "the engine ignores it for PostgreSQL")
        XCTAssertEqual(tab.target(for: .postgres), "public.people_copy")
    }

    func testTheMySQLRunSendsTheDatabaseAsTheCatalogSlot() throws {
        let (model, tab, connection) = fixture(.mysql, database: "shop")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        let env = try model.overrides(for: tab, connection: connection, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["DB_KIND"], "mysql")
        XCTAssertEqual(env["TARGET_CATALOG"], "shop")
        XCTAssertEqual(env["TARGET_TABLE"], "people_copy")
        XCTAssertEqual(env["TARGET_SCHEMA"], "", "the engine ignores it for MySQL")
        XCTAssertEqual(tab.target(for: .mysql), "shop.people_copy")
    }

    func testTheTrinoRunSendsAllThreeParts() throws {
        let (model, tab, connection) = fixture(.trino, database: "hive", schema: "analytics")
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        let env = try model.overrides(for: tab, connection: connection, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["TARGET_CATALOG"], "hive")
        XCTAssertEqual(env["TARGET_SCHEMA"], "analytics")
        XCTAssertEqual(env["TARGET_TABLE"], "people_copy")
        XCTAssertEqual(tab.target(for: .trino), "hive.analytics.people_copy")
    }
}

extension XCTestCase {
    /// The experimental switches over a scratch `UserDefaults`, removed after the test, so turning
    /// one on never writes to anyone's preferences.
    func scratchExperiments(saveTableAcrossConnections: Bool) -> ExperimentalFeatures {
        let suite = "qh-test-experiments-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        let features = ExperimentalFeatures(defaults: defaults)
        features.saveTableAcrossConnections = saveTableAcrossConnections
        return features
    }
}

/// W13-T20: a table destination on **another** connection. The rule for what has to be named is
/// the target's driver, not the source's, and every way the target can fail to resolve blocks the
/// run instead of falling back to the tab's own connection. Every test starts from a fresh tab: a
/// restored session's leftovers are what hid T-2.
@MainActor
final class SaveToTableAcrossConnectionsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        useMemorySecretStore()
    }

    private func connection(_ name: String, _ kind: ConnectionKind, database: String = "",
                            schema: String = "", safeMode: ConnectionSafeMode = .full) -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: kind, host: "localhost",
                   port: kind.defaultPort, sslmode: "", user: "qh", database: database, schema: schema,
                   verify: false, safeMode: safeMode)
    }

    /// A model with a source and a target connection, the switch as asked, and a fresh table tab
    /// on the source pointed at the target.
    private func fixture(source: Connection, target: Connection, switchOn: Bool = true)
        -> (AppModel, QueryTab) {
        let model = AppModel()
        model.engine = MockEngine()
        model.experiments = scratchExperiments(saveTableAcrossConnections: switchOn)
        model.connections = [source, target]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = source.id
        tab.sql = "SELECT 1 AS id"
        tab.destination = .table
        tab.targetConnectionID = target.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab)
    }

    // MARK: The target's driver decides

    func testACopyFromTrinoToPostgresAsksForTheTargetsSchemaAndNeverACatalog() {
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres))
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target schema",
                       "the source is Trino, but PostgreSQL has no catalog part")
        tab.targetSchema = "public"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target table")
        tab.targetTable = "people"
        XCTAssertNil(model.runBlockedReason(for: tab))
    }

    func testACopyFromPostgresToMySQLAsksForADatabase() {
        let (model, tab) = fixture(source: connection("Warehouse", .postgres), target: connection("Shop", .mysql))
        tab.targetTable = "people"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target database")
        tab.targetCatalog = "shop"
        XCTAssertNil(model.runBlockedReason(for: tab), "MySQL has no schema part")
    }

    func testTrinoIsNotATargetAcrossConnections() {
        let (model, tab) = fixture(source: connection("Warehouse", .postgres), target: connection("Lake", .trino))
        tab.targetCatalog = "hive"; tab.targetSchema = "a"; tab.targetTable = "t"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Trino can't be written to from another connection")
    }

    func testAppendIsNotOfferedAcrossConnections() {
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres))
        tab.targetSchema = "public"; tab.targetTable = "people"
        tab.writeMode = .append
        XCTAssertEqual(model.runBlockedReason(for: tab), "Append isn't offered across connections")
    }

    func testTheTargetsOwnSafeModeDecidesWhichModesCanRun() {
        let cases: [(ConnectionSafeMode, WriteMode, String?)] = [
            (.full, .create, nil), (.full, .replace, nil),
            (.confirm, .create, nil), (.confirm, .replace, "Warehouse is Confirm: it refuses replacing a table"),
            (.noDDL, .create, "Warehouse is No DDL: it refuses CREATE TABLE"),
            (.noDDL, .replace, "Warehouse is No DDL: it refuses CREATE TABLE"),
            (.readOnly, .create, "Warehouse is Read only: nothing is written there"),
            (.readOnly, .replace, "Warehouse is Read only: nothing is written there"),
        ]
        for (level, mode, expected) in cases {
            // The source's level is the opposite end of the scale: it only reads, and never decides.
            let source = connection("Lake", .trino, safeMode: .readOnly)
            let (model, tab) = fixture(source: source, target: connection("Warehouse", .postgres, safeMode: level))
            tab.targetSchema = "public"; tab.targetTable = "people"; tab.writeMode = mode
            XCTAssertEqual(model.runBlockedReason(for: tab), expected, "\(level) \(mode)")
        }
    }

    // MARK: Never the tab's own connection by accident

    func testATargetThatIsNoLongerSavedBlocksTheRunAndIsNeverTheTabsOwnConnection() throws {
        let source = connection("Warehouse", .postgres, database: "app", schema: "public")
        let (model, tab) = fixture(source: source, target: connection("Shop", .mysql))
        model.connections = [source]  // the target was deleted
        tab.targetSchema = "public"; tab.targetTable = "people"
        XCTAssertEqual(model.tableDestination(for: tab), .missing)
        XCTAssertEqual(model.runBlockedReason(for: tab), "Choose the target connection again",
                       "a PostgreSQL target with every part named would otherwise run here, on the source")
        model.run(tab)
        XCTAssertNil(model.transfer)
        XCTAssertNil(model.pendingConfirmation)
        XCTAssertNotEqual(tab.stage, .running)
        XCTAssertEqual(tab.logLines.last?.text, "Choose the target connection again")
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
        XCTAssertFalse(env.keys.contains { $0.hasPrefix("DEST_") })
    }

    func testWithTheSwitchOffATabNamingAnotherConnectionIsBlockedNotRedirected() {
        let (model, tab) = fixture(source: connection("Warehouse", .postgres), target: connection("Shop", .mysql),
                                   switchOn: false)
        tab.targetSchema = "public"; tab.targetCatalog = "shop"; tab.targetTable = "people"
        XCTAssertEqual(model.tableDestination(for: tab), .switchedOff)
        XCTAssertEqual(model.runBlockedReason(for: tab), "Saving to another connection is off in Settings")
        model.run(tab)
        XCTAssertNil(model.transfer)
        XCTAssertNotEqual(tab.stage, .running)
    }

    func testWithTheSwitchOffTheEnvironmentIsExactlyTodays() throws {
        let source = connection("Warehouse", .postgres, database: "app", schema: "public")
        let target = connection("Shop", .mysql, database: "shop")
        let (model, tab) = fixture(source: source, target: target, switchOn: false)
        tab.targetSchema = "public"; tab.targetTable = "people"
        let named = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id", confirmed: true)
        tab.targetConnectionID = nil
        let plain = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id", confirmed: true)
        XCTAssertEqual(named, plain, "a target the switch hides adds nothing to what is sent")
        XCTAssertFalse(named.keys.contains { $0.hasPrefix("DEST_") })
    }

    func testTheTabsOwnConnectionByIdentityIsTheServerSideCopy() throws {
        let source = connection("Warehouse", .postgres, database: "app", schema: "public")
        let (model, tab) = fixture(source: source, target: connection("Shop", .mysql))
        tab.targetConnectionID = source.id
        XCTAssertEqual(model.tableDestination(for: tab), .same(source))
        tab.targetSchema = "public"; tab.targetTable = "people"
        XCTAssertNil(model.runBlockedReason(for: tab))
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
        XCTAssertFalse(env.keys.contains { $0.hasPrefix("DEST_") })
    }

    // MARK: Choosing the target

    func testChoosingATargetStartsItsPartsFromThatConnectionAndKeepsTheTableName() {
        let source = connection("Lake", .trino, database: "hive", schema: "analytics")
        let pg = connection("Warehouse", .postgres, database: "app", schema: "public")
        let my = connection("Shop", .mysql, database: "shop")
        let (model, tab) = fixture(source: source, target: pg)
        model.connections = [source, pg, my]
        tab.targetConnectionID = nil
        tab.outputName = "people_copy"
        model.prepareTableDestination(tab)
        XCTAssertEqual([tab.targetCatalog, tab.targetSchema, tab.targetTable], ["hive", "analytics", "people_copy"])

        model.chooseTargetConnection(pg.id, for: tab)
        XCTAssertEqual(tab.targetConnectionID, pg.id)
        XCTAssertEqual([tab.targetCatalog, tab.targetSchema, tab.targetTable], ["", "public", "people_copy"],
                       "PostgreSQL has no catalog part, so Trino's is not left behind")

        model.chooseTargetConnection(my.id, for: tab)
        XCTAssertEqual([tab.targetCatalog, tab.targetSchema, tab.targetTable], ["shop", "", "people_copy"])
        XCTAssertNil(model.runBlockedReason(for: tab))

        model.chooseTargetConnection(source.id, for: tab)
        XCTAssertNil(tab.targetConnectionID, "the tab's own connection is stored as no target")
        XCTAssertEqual([tab.targetCatalog, tab.targetSchema], ["hive", "analytics"])
    }
}
