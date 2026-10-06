import XCTest

@testable import QueryHive

/// DataGrip's `dataSources.xml` and friends, written by the test, with a stand-in for the Keychain:
/// the real one is never asked (W13-T16, DBX-73).
final class DataGripImportTests: XCTestCase {
    private final class FakeKeychain: ForeignKeychain {
        var answers: [String: ForeignKeychainResult] = [:]
        private(set) var asked: [String] = []
        func password(service: String) -> ForeignKeychainResult {
            asked.append(service)
            return answers[service] ?? .notFound
        }
    }

    private let shared = """
    <?xml version="1.0" encoding="UTF-8"?>
    <project version="4">
      <component name="DataSourceManagerImpl" format="xml" multifile-model="true">
        <data-source source="LOCAL" name="Prod PG" uuid="uuid-1" group="Prod">
          <driver-ref>postgresql</driver-ref>
          <jdbc-driver>org.postgresql.Driver</jdbc-driver>
          <jdbc-url>jdbc:postgresql://pg.corp:5433/app?sslmode=verify-ca</jdbc-url>
        </data-source>
        <data-source source="LOCAL" name="Shop" uuid="uuid-2">
          <driver-ref>mysql.8</driver-ref>
          <jdbc-url>jdbc:mysql://my.corp:3306/shop</jdbc-url>
          <ssh-properties><enabled>true</enabled></ssh-properties>
        </data-source>
        <data-source source="LOCAL" name="Lake" uuid="uuid-3">
          <driver-ref>trino</driver-ref>
          <jdbc-url>jdbc:trino://t.corp:8080/hive/default</jdbc-url>
        </data-source>
        <data-source source="LOCAL" name="Orders" uuid="uuid-4">
          <driver-ref>oracle</driver-ref>
          <jdbc-url>jdbc:oracle:thin:@//o.corp:1521/svc</jdbc-url>
        </data-source>
      </component>
    </project>
    """

    private let local = """
    <?xml version="1.0" encoding="UTF-8"?>
    <project version="4">
      <component name="dataSourceStorageLocal">
        <data-source name="Prod PG" uuid="uuid-1">
          <database-info product="PostgreSQL" version="15" />
          <user-name>ana</user-name>
        </data-source>
        <data-source name="Shop" uuid="uuid-2"><user-name>shopper</user-name></data-source>
      </component>
    </project>
    """

    private let forest = """
    <?xml version="1.0" encoding="UTF-8"?>
    <project version="4">
      <component name="DbForest"><data>
    1:0:00000000:Prod
    2:1:00000000:EU
    ------------------------
    9:2:uuid-2
      </data></component>
    </project>
    """

    private func read(_ keychain: (any ForeignKeychain)?, withLocal: Bool = true, withForest: Bool = true) throws -> ImportBatch {
        try DataGripImport.read(dataSources: Data(shared.utf8), local: withLocal ? Data(local.utf8) : nil,
                                forest: withForest ? Data(forest.utf8) : nil, keychain: keychain)
    }

    func testTheThreeFilesMergeIntoConnectionsWithUsersAndGroups() throws {
        let batch = try read(nil)
        let pg = try XCTUnwrap(batch.connections.first { $0.name == "Prod PG" })
        XCTAssertEqual(pg.kind, .postgres)
        XCTAssertEqual(pg.host, "pg.corp")
        XCTAssertEqual(pg.port, 5433)
        XCTAssertEqual(pg.database, "app")
        XCTAssertEqual(pg.user, "ana")
        XCTAssertEqual(pg.sslmode, "verify-ca")
        XCTAssertEqual(pg.group, "Prod")
        let shop = try XCTUnwrap(batch.connections.first { $0.name == "Shop" })
        XCTAssertEqual(shop.kind, .mysql)
        XCTAssertEqual(shop.group, "Prod / EU", "the forest file nests folders")
        XCTAssertEqual(shop.notImported, ["SSH tunnel"], "a tunnel DataGrip has is named, not guessed")
        let lake = try XCTUnwrap(batch.connections.first { $0.name == "Lake" })
        XCTAssertEqual(lake.kind, .trino)
        XCTAssertEqual(lake.trinoScheme, "http")
        XCTAssertEqual(lake.schema, "default")
        XCTAssertEqual(batch.skipped.map(\.name), ["Orders"])
    }

    func testWithoutAKeychainNothingIsAskedAndTheSummarySaysSo() throws {
        let batch = try read(nil)
        XCTAssertTrue(batch.connections.allSatisfy { $0.password == nil })
        XCTAssertTrue(batch.notes.joined().contains("No passwords were read"))
    }

    func testPasswordsAreReadByServiceAndNotFoundIsReportedApartFromCancel() throws {
        let keychain = FakeKeychain()
        keychain.answers[DataGripImport.keychainService(uuid: "uuid-1")] = .found("pg-secret")
        let batch = try read(keychain)
        XCTAssertEqual(batch.connections.first { $0.name == "Prod PG" }?.password, "pg-secret")
        XCTAssertEqual(keychain.asked.first, "IntelliJ Platform DB \u{2014} uuid-1")
        let notes = batch.notes.joined(separator: "\n")
        XCTAssertTrue(notes.contains("DataGrip had no saved password in Keychain for: Shop, Lake"), notes)
        XCTAssertFalse(notes.contains("cancelled"), notes)
    }

    func testCancellingAPromptStopsTheReadingAndNamesWhatWasNotRead() throws {
        let keychain = FakeKeychain()
        keychain.answers[DataGripImport.keychainService(uuid: "uuid-1")] = .found("pg-secret")
        keychain.answers[DataGripImport.keychainService(uuid: "uuid-2")] = .cancelled
        let batch = try read(keychain)
        XCTAssertEqual(keychain.asked.count, 2, "no third prompt after a cancel: \(keychain.asked)")
        XCTAssertEqual(batch.connections.first { $0.name == "Prod PG" }?.password, "pg-secret")
        XCTAssertEqual(batch.connections.count, 3, "a cancelled password never costs a connection")
        let notes = batch.notes.joined(separator: "\n")
        XCTAssertTrue(notes.contains("cancelled at Shop"), notes)
        XCTAssertTrue(notes.contains("Shop, Lake"), notes)
    }

    func testAKeychainFailureIsNamedForThatConnection() throws {
        let keychain = FakeKeychain()
        keychain.answers[DataGripImport.keychainService(uuid: "uuid-1")] = .failed("boom")
        let notes = try read(keychain).notes.joined(separator: "\n")
        XCTAssertTrue(notes.contains("Couldn't read the password for Prod PG from Keychain: boom"), notes)
    }

    func testLocateFindsTheFilesByNameAndRefusesAPickWithoutDataSources() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("qh-dg-\(UUID().uuidString)")
        let idea = dir.appendingPathComponent(".idea")
        try FileManager.default.createDirectory(at: idea, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        try shared.write(to: idea.appendingPathComponent("dataSources.xml"), atomically: true, encoding: .utf8)
        try local.write(to: idea.appendingPathComponent("dataSources.local.xml"), atomically: true, encoding: .utf8)

        let files = try DataGripImport.locate([dir])
        XCTAssertEqual(files.dataSources.lastPathComponent, "dataSources.xml")
        XCTAssertEqual(files.local?.lastPathComponent, "dataSources.local.xml")
        XCTAssertNil(files.forest)
        XCTAssertThrowsError(try DataGripImport.locate([idea.appendingPathComponent("dataSources.local.xml")]))
        XCTAssertThrowsError(try DataGripImport.read(dataSources: Data("nope".utf8), local: nil, forest: nil, keychain: nil))
    }

    @MainActor
    func testAnImportIntoTheModelWritesThePasswordsToQueryHivesOwnItemsOnly() throws {
        isolateConnectionStore()
        let store = useMemorySecretStore()
        let model = AppModel(persistsSession: false)
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("qh-dg-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        try shared.write(to: dir.appendingPathComponent("dataSources.xml"), atomically: true, encoding: .utf8)
        try local.write(to: dir.appendingPathComponent("dataSources.local.xml"), atomically: true, encoding: .utf8)
        try forest.write(to: dir.appendingPathComponent("db-forest-config.xml"), atomically: true, encoding: .utf8)

        let keychain = FakeKeychain()
        keychain.answers[DataGripImport.keychainService(uuid: "uuid-1")] = .found("pg-secret")
        model.importDataGripConnections(from: [dir], keychain: keychain)

        let pg = try XCTUnwrap(model.connections.first { $0.name == "Prod PG" })
        XCTAssertEqual(pg.sslmode, "verify-ca")
        XCTAssertEqual(try store.get(slot: .database, for: pg.id), "pg-secret")
        XCTAssertEqual(model.groups.map(\.name).sorted(), ["Prod", "Prod / EU"])
        XCTAssertEqual(model.notice?.title.hasSuffix("from DataGrip"), true)
        XCTAssertTrue(try XCTUnwrap(model.notice?.message).contains("Not imported: Shop: SSH tunnel"))
    }
}
