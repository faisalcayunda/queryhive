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
