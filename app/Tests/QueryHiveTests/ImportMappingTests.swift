import XCTest

@testable import QueryHive

/// The import sheet's pure half: the mapping it builds into the engine's `COLUMNS`/`TARGET_*`
/// settings, the header it reads from a file, and what it refuses to claim.
///
/// The engine's own import is tested in Rust (`crates/qh-ffi/tests/`); these tests pin the
/// settings the app sends it, so a sheet that showed one mapping and sent another would fail here.
final class ImportMappingTests: XCTestCase {
    // MARK: Fields and matching

    func testFieldsMatchTheTargetByNameNotByPosition() {
        let fields = ImportMapping.mappedFields(headers: ["ID", "nama", "extra"],
                                                targetColumns: ["id", "nama"])
        XCTAssertEqual(fields.map(\.name), ["ID", "nama", "extra"])
        XCTAssertTrue(fields[0].include, "ID matches id case-insensitively")
        XCTAssertEqual(fields[0].target, "id")
        XCTAssertFalse(fields[2].include, "extra has no target column")
        XCTAssertEqual(fields[2].target, "")
    }

    func testWithNoTargetColumnsEveryHeaderFieldIsKeptAndNamedAfterItself() {
        // XLSX, or a table the app could not describe: this is the engine's own header-derived
        // default, and it is what the sheet falls back to rather than an empty mapping.
        let fields = ImportMapping.mappedFields(headers: ["a", "b"], targetColumns: [])
        XCTAssertEqual(fields.map(\.include), [true, true])
        XCTAssertEqual(fields.map(\.target), ["a", "b"])
    }

    // MARK: COLUMNS

    func testColumnsJSONCarriesSourceTargetAndInclude() throws {
        var mapping = ImportMapping()
        mapping.targetColumns = ["id", "nama"]
        mapping.fields = [
            ImportField(source: 0, name: "ID", include: true, target: "id"),
            ImportField(source: 1, name: "nama", include: false, target: "nama"),
        ]
        let json = try XCTUnwrap(mapping.columnsJSON)
        let rows = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [[String: Any]])
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0]["source"] as? Int, 0)
        XCTAssertEqual(rows[0]["target"] as? String, "id")
        XCTAssertEqual(rows[0]["include"] as? Bool, true)
        XCTAssertEqual(rows[1]["include"] as? Bool, false)
    }

    func testColumnsJSONIsAbsentWhenTheTargetsColumnsAreUnknown() {
        var mapping = ImportMapping()
        mapping.fields = [ImportField(source: 0, name: "a", include: true, target: "a")]
        XCTAssertNil(mapping.columnsJSON, "nothing to pick from means the engine maps by header")
    }

    func testAnIncludedFieldWithNoTargetFallsBackToItsOwnName() throws {
        var mapping = ImportMapping()
        mapping.targetColumns = ["a"]
        mapping.fields = [ImportField(source: 0, name: "a", include: true, target: "")]
        let json = try XCTUnwrap(mapping.columnsJSON)
        let rows = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [[String: Any]])
        XCTAssertEqual(rows[0]["target"] as? String, "a")
    }

    // MARK: Settings

    func testSettingsCarryTheKeysTheEngineReads() {
        var mapping = ImportMapping()
        mapping.path = "/tmp/people.csv"
        mapping.format = .csv
        mapping.targetCatalog = "hive"
        mapping.targetSchema = "analytics"
        mapping.targetTable = "people"
        mapping.onError = .skip
        mapping.foreignKeys = false
        mapping.header = false

        let env = mapping.settings()
        XCTAssertEqual(env["IMPORT_PATH"], "/tmp/people.csv")
        XCTAssertEqual(env["IMPORT_FORMAT"], "csv")
        XCTAssertEqual(env["TARGET_CATALOG"], "hive")
        XCTAssertEqual(env["TARGET_SCHEMA"], "analytics")
        XCTAssertEqual(env["TARGET_TABLE"], "people")
        XCTAssertEqual(env["ON_ERROR"], "skip")
        XCTAssertEqual(env["FOREIGN_KEYS"], "0")
        XCTAssertEqual(env["HEADER"], "0")
        XCTAssertNil(env["COLUMNS"], "no target columns means no COLUMNS")
        XCTAssertNil(env["DELIMITER"], "the comma is the engine's own default")
    }

    func testTSVAlwaysCarriesItsTabDelimiter() {
        var mapping = ImportMapping()
        mapping.format = .tsv
        mapping.delimiter = "\t"
        XCTAssertEqual(mapping.settings()["DELIMITER"], "\t")
        XCTAssertEqual(mapping.settings()["IMPORT_FORMAT"], "csv", "a TSV is a CSV with a tab")
    }

    func testACustomDelimiterAndBatchTravel() {
        var mapping = ImportMapping()
        mapping.delimiter = "|"
        mapping.batchSize = 500
        let env = mapping.settings()
        XCTAssertEqual(env["DELIMITER"], "|")
        XCTAssertEqual(env["IMPORT_BATCH"], "500")
    }

    // MARK: What it refuses to claim

    func testTheImportIsRefusedAtConfirmAndReadOnlyAndNowhereElse() {
        XCTAssertNil(ImportMapping.refusalReason(safeMode: .full))
        XCTAssertNil(ImportMapping.refusalReason(safeMode: .noDDL))
        XCTAssertNotNil(ImportMapping.refusalReason(safeMode: .confirm))
        XCTAssertNotNil(ImportMapping.refusalReason(safeMode: .readOnly))
        // The confirm sentence names the engine's reason: one approval cannot cover a plan.
        let confirm = ImportMapping.refusalReason(safeMode: .confirm) ?? ""
        XCTAssertTrue(confirm.contains("one confirmation"), confirm)
    }

    func testReadyNeedsAFileATargetAndAtLeastOneField() {
        var mapping = ImportMapping()
        XCTAssertFalse(mapping.ready, "no file and no target")
        mapping.path = "/tmp/x.csv"
        mapping.targetTable = "t"
        XCTAssertTrue(mapping.ready, "a header-derived mapping has every field")

        // With a real mapping, excluding every field leaves nothing to write.
        mapping.targetColumns = ["a"]
        mapping.fields = [ImportField(source: 0, name: "a", include: false, target: "a")]
        XCTAssertFalse(mapping.ready)
    }

    // MARK: Remapping against the loaded target

    func testTheFirstColumnLoadRebuildsTheMappingAgainstTheTarget() {
        var mapping = ImportMapping()
        mapping.fields = ImportMapping.mappedFields(headers: ["id", "nama"], targetColumns: [])
        let remapped = mapping.remapped(to: ["id", "nama", "lain"])
        XCTAssertEqual(remapped.targetColumns, ["id", "nama", "lain"])
        XCTAssertEqual(remapped.fields.map(\.target), ["id", "nama"])
    }

    func testAReloadKeepsTheUsersChoicesAndDropsATargetThatIsGone() {
        var mapping = ImportMapping()
        mapping.targetColumns = ["id", "nama"]
        mapping.fields = [
            ImportField(source: 0, name: "ID", include: true, target: "id"),
            ImportField(source: 1, name: "nama", include: false, target: "nama"),
        ]
        let remapped = mapping.remapped(to: ["id"])
        XCTAssertEqual(remapped.fields[0].include, true, "the user's include survives")
        XCTAssertEqual(remapped.fields[1].target, "", "nama is no longer a target column")
        XCTAssertFalse(remapped.fields[1].include)
    }

    // MARK: Reading the file's own header

    func testAHeaderSplitsOnTheDelimiterAndKeepsQuotedContent() {
        XCTAssertEqual(ImportHeaderReader.firstRecord(in: "a,b,c\n1,2,3", delimiter: ","),
                       ["a", "b", "c"])
        XCTAssertEqual(ImportHeaderReader.firstRecord(in: "\"a,b\",c\n", delimiter: ","),
                       ["a,b", "c"])
        XCTAssertEqual(ImportHeaderReader.firstRecord(in: "\"say \"\"hi\"\"\",b", delimiter: ","),
                       ["say \"hi\"", "b"])
        XCTAssertEqual(ImportHeaderReader.firstRecord(in: "a\tb\tc\n", delimiter: "\t"),
                       ["a", "b", "c"])
        XCTAssertEqual(ImportHeaderReader.firstRecord(in: "only", delimiter: ","), ["only"])
    }

    func testReadHeadersReadsTheFirstRecordOfARealFile() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-import-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("people.csv")
        try Data("ID,nama,kode\n1,a,2\n".utf8).write(to: path)

        XCTAssertEqual(try ImportHeaderReader.readHeaders(path: path.path, delimiter: ","),
                       ["ID", "nama", "kode"])
        XCTAssertThrowsError(try ImportHeaderReader.readHeaders(path: path.path + ".nope", delimiter: ","))
    }

    func testTheFormatComesFromTheExtension() {
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.csv"), .csv)
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.TSV"), .tsv)
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.xlsx"), .xlsx)
        XCTAssertNil(ImportSourceFormat.detect(path: "/tmp/a.sql"))
        XCTAssertNil(ImportSourceFormat.detect(path: "/tmp/a.json"))
        XCTAssertFalse(ImportSourceFormat.xlsx.appCanReadHeader)
        XCTAssertTrue(ImportSourceFormat.csv.appCanReadHeader)
    }
}
