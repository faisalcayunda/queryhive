import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Quoting on accept: a name is written quoted when, and only when, its connection's dialect needs
/// quotes to read it back. The rule itself (`SQLIdentifierQuoting`) is tested with its own file;
/// these tests are about where the suggestions apply it.
///
/// The owner's schemas look like `prod-ops-aktivitas-produksi-harian-ro` (a hyphen: quote) and
/// `prod_datalake_pii_ro` (snake_case: never quote).
@MainActor
final class SuggestionQuotingTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func model(kind: ConnectionKind, database: String, schema: String) -> (AppModel, QueryTab, TreeNode) {
        let model = AppModel()
        let engine = MockEngine()
        for command in ["catalogs", "schemas", "tables"] { engine.answer(command, with: .events([])) }
        model.engine = engine
        let connection = Connection(id: UUID(), name: "c", color: .violet, kind: kind, host: "h.internal",
                                    port: 1, sslmode: "prefer", user: "u", database: database,
                                    schema: schema, verify: true)
        model.connections = [connection]
        model.rebuildTree()
        let tab = QueryTab(title: "q")
        tab.connectionID = connection.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab, model.tree[0])
    }

    private func insert(_ items: [SQLSuggestion], _ text: String) -> String? {
        items.first { $0.text == text }?.insert
    }

    // MARK: Schemas and catalogs

    func testAHyphenatedSchemaIsQuotedAndASnakeCaseOneIsNot() {
        let (model, tab, root) = model(kind: .postgres, database: "warehouse", schema: "public")
        root.children = ["prod-ops-aktivitas-produksi-harian-ro", "prod_datalake_pii_ro", "Sales", "public"]
            .map { TreeNode.schema($0, parent: root) }

        let found = model.suggestions(for: tab, prefix: "prod", path: [])
        XCTAssertEqual(insert(found, "prod-ops-aktivitas-produksi-harian-ro"),
                       "\"prod-ops-aktivitas-produksi-harian-ro\"")
        XCTAssertEqual(insert(found, "prod_datalake_pii_ro"), "prod_datalake_pii_ro",
                       "an underscore never needs quotes")
        XCTAssertEqual(found.first { $0.text.hasPrefix("prod-ops") }?.text, "prod-ops-aktivitas-produksi-harian-ro",
                       "the list shows the name, not its quoted spelling")
        // Upper case folds on PostgreSQL, so "Sales" is a different schema from sales.
        XCTAssertEqual(insert(model.suggestions(for: tab, prefix: "sal", path: []), "Sales"), "\"Sales\"")
    }

    func testACatalogIsQuotedByTheSameRule() {
        let (model, tab, root) = model(kind: .trino, database: "hive", schema: "analytics")
        root.children = [TreeNode.catalog("lake-1", parent: root), TreeNode.catalog("hive", parent: root)]
        let found = model.suggestions(for: tab, prefix: "", path: []).filter { $0.kind == .catalog }
        XCTAssertEqual(insert(found, "lake-1"), "\"lake-1\"")
        XCTAssertEqual(insert(found, "hive"), "hive")
    }

    func testUnderAPathOnlyTheLastPartIsWritten() {
        let (model, tab, root) = model(kind: .trino, database: "hive", schema: "analytics")
        let catalog = TreeNode.catalog("lake-1", parent: root)
        catalog.children = [TreeNode.schema("Public", parent: catalog), TreeNode.schema("raw", parent: catalog)]
        root.children = [catalog]
        let found = model.suggestions(for: tab, prefix: "", path: ["lake-1"])
        XCTAssertEqual(insert(found, "Public"), "\"Public\"")
        XCTAssertEqual(insert(found, "raw"), "raw")
    }

    // MARK: Tables

    func testATableIsQuotedForWhatItNeeds() {
        let (model, tab, root) = model(kind: .postgres, database: "warehouse", schema: "public")
        let schema = TreeNode.schema("public", parent: root)
        schema.children = ["My Table", "2024_sales", "order", "plain_table", "has\"quote"]
            .map { TreeNode.table($0, parent: schema) }
        root.children = [schema]

        let found = model.suggestions(for: tab, prefix: "", path: ["public"])
        XCTAssertEqual(insert(found, "My Table"), "\"My Table\"")
        XCTAssertEqual(insert(found, "2024_sales"), "\"2024_sales\"", "a leading digit")
        XCTAssertEqual(insert(found, "order"), "\"order\"", "a reserved word")
        XCTAssertEqual(insert(found, "plain_table"), "plain_table")
        XCTAssertEqual(insert(found, "has\"quote"), "\"has\"\"quote\"", "an embedded quote is doubled")
    }

    func testMySQLUsesBackticks() {
        let (model, tab, root) = model(kind: .mysql, database: "reporting", schema: "")
        let database = TreeNode.database("reporting", parent: root)
        database.children = [TreeNode.table("laporan-harian", parent: database), TreeNode.table("kpm", parent: database)]
        root.children = [database]
        let found = model.suggestions(for: tab, prefix: "", path: ["reporting"])
        XCTAssertEqual(insert(found, "laporan-harian"), "`laporan-harian`")
        XCTAssertEqual(insert(found, "kpm"), "kpm")
    }

    func testAQualifiedTableQuotesEachPartForItself() {
        let (model, tab, root) = model(kind: .trino, database: "hive", schema: "analytics")
        let catalog = TreeNode.catalog("prod-ops-x", parent: root)
        let schema = TreeNode.schema("public", parent: catalog)
        schema.children = [TreeNode.table("My Table", parent: schema)]
        catalog.children = [schema]
        root.children = [catalog]

        // Away from the tab's hive.analytics, so the name is written with all three parts.
        let found = model.suggestions(for: tab, prefix: "My", path: [])
        XCTAssertEqual(found.first?.text, "prod-ops-x.public.My Table")
        XCTAssertEqual(found.first?.insert, "\"prod-ops-x\".public.\"My Table\"")
    }

    func testTheAliasIsMadeFromTheBareNameAndNeedsNoQuotes() {
        let (model, tab, root) = model(kind: .trino, database: "hive", schema: "analytics")
        let catalog = TreeNode.catalog("hive", parent: root)
        let schema = TreeNode.schema("analytics", parent: catalog)
        schema.children = [TreeNode.table("Data Harian", parent: schema), TreeNode.table("laporan-harian", parent: schema),
                           TreeNode.table("2024_sales", parent: schema)]
        catalog.children = [schema]
        root.children = [catalog]
        let refs = EditorReferences(revision: 1, statementStartUtf16: 0, statementLenUtf16: 0,
                                    relations: [], ctes: [], source: .tree)
        let found = model.suggestions(for: tab, prefix: "", path: [],
                                      scope: EditorCaretScope(clause: .queryTable, selectAliases: []),
                                      references: refs).filter { $0.kind == .table }
        XCTAssertEqual(found.count, 3)
        for item in found {
            let alias = String(item.insert.split(separator: " ").last ?? "")
            XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(alias, for: .trino),
                           "the alias of \(item.text) is \(alias), which would itself need quotes")
        }
        XCTAssertTrue(found.contains { $0.insert.hasPrefix("\"laporan-harian\" ") })
    }

    // MARK: Columns, aliases and keywords

    func testAColumnIsQuotedAndAKeywordAndASelectAliasAreNot() {
        let (model, tab, _) = model(kind: .postgres, database: "warehouse", schema: "public")
        tab.columns = [Event.Column(name: "Nama-Kolom", type: "varchar"),
                       Event.Column(name: "jumlah_jiwa", type: "bigint")]
        let scope = EditorCaretScope(clause: .orderBy, selectAliases: ["\"Net Total\"", "total"])
        let found = model.suggestions(for: tab, prefix: "", path: [], scope: scope)
        XCTAssertEqual(insert(found, "Nama-Kolom"), "\"Nama-Kolom\"")
        XCTAssertEqual(insert(found, "jumlah_jiwa"), "jumlah_jiwa")
        XCTAssertEqual(insert(found, "\"Net Total\""), "\"Net Total\"", "an alias is already spelled as written")
        let keywords = model.suggestions(for: tab, prefix: "ord", path: [], scope: scope)
        XCTAssertEqual(insert(keywords, "ORDER BY"), "ORDER BY", "a keyword is never quoted")
    }

    func testAQuoteTheUserTypedIsKeptEvenWhereNoneIsNeeded() {
        let (model, tab, root) = model(kind: .postgres, database: "warehouse", schema: "public")
        let schema = TreeNode.schema("public", parent: root)
        schema.children = [TreeNode.table("orders", parent: schema)]
        root.children = [schema]
        XCTAssertEqual(insert(model.suggestions(for: tab, prefix: "ord", path: ["public"]), "orders"), "orders")
        XCTAssertEqual(insert(model.suggestions(for: tab, prefix: "ord", path: ["public"], typedQuote: true), "orders"),
                       "\"orders\"")
    }

    func testWithoutAConnectionNothingIsQuotedBecauseNothingIsKnown() {
        let model = AppModel()
        let tab = QueryTab(title: "q")
        tab.columns = [Event.Column(name: "Nama-Kolom", type: "varchar")]
        model.tabs = [tab]
        XCTAssertEqual(insert(model.suggestions(for: tab, prefix: "Nama", path: []), "Nama-Kolom"), "Nama-Kolom")
    }

    // MARK: What the editor's own lexer makes of a quoted name with a hyphen

    func testAHyphenInsideAQuotedNameIsOneIdentifierToTheEditor() throws {
        let sql = "select * from \"prod-ops-x\".public.\"My Table\" m where m.a = 1"
        let analysis = try EditorAnalysis(text: sql, dialect: .trino)
        let (paint, outline) = try analysis.drainForTesting()
        let quoted = paint.runs.filter { $0.colorClass == .quotedIdentifier }.map { (sql as NSString).substring(with: $0.range) }
        XCTAssertEqual(quoted, ["\"prod-ops-x\"", "\"My Table\""], "the hyphen split the name")
        XCTAssertTrue(outline.issues.isEmpty, "\(outline.issues)")

        let refs = try analysis.references(at: 8)
        XCTAssertEqual(refs.relations.count, 1)
        XCTAssertEqual(refs.relations.first?.catalog, "prod-ops-x")
        XCTAssertEqual(refs.relations.first?.name, "My Table")
        XCTAssertEqual(refs.relations.first?.alias, "m")
    }
}
