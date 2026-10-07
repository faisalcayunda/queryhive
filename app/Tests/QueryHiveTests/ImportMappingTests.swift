import XCTest

@testable import QueryHive

/// The import sheet's pure half: the mapping it builds into the engine's `COLUMNS`/`TARGET_*`
/// settings (for rows, JSON and `.sql` files), the header it reads from a file, how it reads the
/// engine's answer, and what it refuses to claim.
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
        XCTAssertNil(rows[0]["name"], "CSV is not checked by name: the app's header and the engine's can differ")
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

    // MARK: Reading the file's values

    func testNoValueSettingIsSentUntilItIsAskedFor() {
        var mapping = ImportMapping()
        mapping.format = .csv
        let env = mapping.settings()
        XCTAssertNil(env["DATE_FORMAT"])
        XCTAssertNil(env["DECIMAL_SEPARATOR"])
        XCTAssertNil(env["GROUPING_SEPARATOR"])
        XCTAssertNil(env["ENCODING"])
        XCTAssertNil(env["ALLOW_SHORT_ROWS"], "a short row is refused unless the sheet opts in (PF-4)")
    }

    func testTheValueSettingsTravelForTheFormatsThatCarryTextThePersonWrote() {
        var mapping = ImportMapping()
        mapping.format = .csv
        mapping.dateFormat = "dd/MM/yyyy"
        mapping.decimalSeparator = ","
        mapping.groupingSeparator = "."
        mapping.encoding = "cp1252"
        mapping.allowShortRows = true
        let env = mapping.settings()
        XCTAssertEqual(env["DATE_FORMAT"], "dd/MM/yyyy")
        XCTAssertEqual(env["DECIMAL_SEPARATOR"], ",")
        XCTAssertEqual(env["GROUPING_SEPARATOR"], ".")
        XCTAssertEqual(env["ENCODING"], "cp1252")
        XCTAssertEqual(env["ALLOW_SHORT_ROWS"], "1")
    }

    func testAGroupingSeparatorIsReadOnlyBesideADifferentDecimalOne() {
        var mapping = ImportMapping()
        mapping.format = .csv
        mapping.groupingSeparator = "."
        // Alone it means nothing: `1.500` could be 1500 or 1.5. The engine refuses it, so the sheet
        // must not send it and read as honoured.
        XCTAssertNil(mapping.settings()["GROUPING_SEPARATOR"])
        mapping.decimalSeparator = "."
        // And the two must differ.
        XCTAssertNil(mapping.settings()["GROUPING_SEPARATOR"])
        mapping.decimalSeparator = ","
        XCTAssertEqual(mapping.settings()["GROUPING_SEPARATOR"], ".")
    }

    func testASeparatorIsSentOnlyWhenItIsExactlyOneCharacter() {
        var mapping = ImportMapping()
        mapping.format = .csv
        mapping.decimalSeparator = ".,"
        XCTAssertNil(mapping.settings()["DECIMAL_SEPARATOR"], "a two-character separator is not one")
        mapping.decimalSeparator = ""
        XCTAssertNil(mapping.settings()["DECIMAL_SEPARATOR"], "blank leaves the engine's default")
    }

    func testAJSONFileIsNotSentValueReadingsItCannotHonour() {
        var mapping = ImportMapping()
        mapping.format = .json
        mapping.dateFormat = "yyyy-MM-dd"
        mapping.decimalSeparator = ","
        mapping.groupingSeparator = "."
        mapping.encoding = "cp1252"
        let env = mapping.settings()
        // A JSON number is always `.`-decimal, so the engine refuses a locale rather than multiply
        // `1.234` by a thousand; and a code page applies to a CSV file, not a JSON one.
        XCTAssertNil(env["DECIMAL_SEPARATOR"])
        XCTAssertNil(env["GROUPING_SEPARATOR"])
        XCTAssertNil(env["DATE_FORMAT"])
        XCTAssertNil(env["ENCODING"])
    }

    func testASQLFileIsSentNoValueReadingsAtAll() {
        var mapping = ImportMapping()
        mapping.format = .sql
        mapping.dateFormat = "dd/MM/yyyy"
        mapping.decimalSeparator = ","
        mapping.allowShortRows = true
        let env = mapping.settings()
        XCTAssertNil(env["DATE_FORMAT"])
        XCTAssertNil(env["DECIMAL_SEPARATOR"])
        XCTAssertNil(env["ALLOW_SHORT_ROWS"], "a statement file has no rows to pad")
    }

    func testASheetNamesNoCodePageButPadsRowsWhenAsked() {
        var mapping = ImportMapping()
        mapping.format = .xlsx
        mapping.encoding = "cp1252"
        mapping.allowShortRows = true
        let env = mapping.settings()
        XCTAssertNil(env["ENCODING"], "a workbook carries its own encoding")
        XCTAssertEqual(env["ALLOW_SHORT_ROWS"], "1")
    }

    // MARK: The running footer's figure

    func testTheProgressFigureUsesTheTotalTheEngineGave() {
        // A file: the bytes read against the file's size.
        XCTAssertEqual(ImportSheet.progressLabel(read: 3_000_000, total: 4_000_000, rows: nil),
                       "Importing… 75%")
        // A sheet: bytes are absent, so its declared rows stand in.
        XCTAssertEqual(ImportSheet.progressLabel(read: nil, total: nil, rows: 1200),
                       "Importing… \(1200.formatted()) rows to read")
        // Nothing has arrived yet: no figure to invent, so the plain word.
        XCTAssertEqual(ImportSheet.progressLabel(read: nil, total: nil, rows: nil), "Importing…")
        // A zero total is not a percentage: guard against a division the file did not ask for.
        XCTAssertEqual(ImportSheet.progressLabel(read: 10, total: 0, rows: nil), "Importing…")
    }

    // MARK: Windows on a code page

    func testTheDeclaredCodePageIsTheOneTheHeaderIsReadWith() {
        XCTAssertEqual(ImportHeaderReader.encoding(for: "cp1252"), .windowsCP1252)
        XCTAssertEqual(ImportHeaderReader.encoding(for: "windows-1252"), .windowsCP1252)
        XCTAssertEqual(ImportHeaderReader.encoding(for: "latin-1"), .isoLatin1)
        XCTAssertEqual(ImportHeaderReader.encoding(for: "UTF-8"), .utf8)
        // An unknown name reads as UTF-8 rather than as something else the app guessed at; the
        // engine still owns which names are valid and refuses the rest by name.
        XCTAssertEqual(ImportHeaderReader.encoding(for: "ebcdic"), .utf8)
    }

    func testAHeaderReadUnderTheByteCapDropsACharacterTheCapSplitInTwo() throws {
        // A whole prefix is left alone: only a read cut at the cap can end mid-character.
        XCTAssertEqual(Data("name,value\n".utf8).dropLastTrailingUTF8Sequence(),
                       Data("name,value\n".utf8))

        // The trim drops just the dangling bytes of an incomplete trailing character.
        var split = Data("caf".utf8)
        split.append(contentsOf: "é".utf8.dropLast())  // `caf` then half of a 2-byte `é`
        XCTAssertEqual(split.dropLastTrailingUTF8Sequence(), Data("caf".utf8))

        // And the reader uses it: a file longer than the cap whose cut lands mid-character reads
        // its header rather than failing the decode as "not a text file".
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("import-header-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        // A header row, then padding up to the cap so the cut is taken, ending half a character in.
        var padded = Data("name,value\n".utf8)
        padded.append(contentsOf: Data(repeating: 0x61, count: ImportHeaderReader.prefixLimit))
        // Trim back to end exactly at the cap with a half `é` as its last byte.
        padded.removeLast(padded.count - ImportHeaderReader.prefixLimit + 1)
        padded.append(contentsOf: "é".utf8.dropLast())
        padded.append(0x62)
        let path = directory.appendingPathComponent("split.csv")
        try padded.write(to: path)
        XCTAssertEqual(try ImportHeaderReader.readHeaders(path: path.path, delimiter: ","),
                       ["name", "value"])
    }

    func testAHeaderReadWithACodePageDecodesBytesUTF8WouldNot() throws {
        // A `0xE9` is `é` in cp1252 and not a byte UTF-8 accepts, so the code page is what makes
        // the file readable at all (DBX-7).
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("import-header-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        var data = Data("caf".utf8)
        data.append(0xE9)  // `é` in cp1252
        let path = directory.appendingPathComponent("cp1252.csv")
        try data.write(to: path)
        XCTAssertThrowsError(try ImportHeaderReader.readHeaders(path: path.path, delimiter: ","))
        XCTAssertEqual(try ImportHeaderReader.readHeaders(path: path.path, delimiter: ",",
                                                         codePage: "cp1252"),
                       ["café"])
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
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.json"), .json)
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.JSONL"), .json)
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/a.ndjson"), .json)
        XCTAssertEqual(ImportSourceFormat.detect(path: "/tmp/dump.sql"), .sql)
        XCTAssertNil(ImportSourceFormat.detect(path: "/tmp/a.parquet"))
        XCTAssertFalse(ImportSourceFormat.xlsx.appCanReadHeader)
        XCTAssertTrue(ImportSourceFormat.csv.appCanReadHeader)
    }

    func testEveryExtensionThePanelOffersIsOneTheSheetOpens() {
        for name in ImportSourceFormat.panelExtensions {
            XCTAssertNotNil(ImportSourceFormat.detect(path: "/tmp/x.\(name)"), name)
        }
    }

    func testOnlyJSONGetsItsFieldListFromTheEngine() {
        // The app reads a CSV header itself and has no JSON reader, so the keys come from
        // `IMPORT_PREVIEW`; XLSX and `.sql` have no field list at all.
        XCTAssertEqual(ImportSourceFormat.allCases.filter(\.hasFieldList), [.csv, .tsv, .json])
        XCTAssertEqual(ImportSourceFormat.allCases.filter(\.enginePreviewsFields), [.json])
        XCTAssertEqual(ImportSourceFormat.allCases.filter(\.importsStatements), [.sql])
        XCTAssertEqual(ImportSourceFormat.allCases.filter(\.hasDelimiter), [.csv, .tsv])
        XCTAssertEqual(ImportSourceFormat.allCases.filter(\.hasHeaderRow), [.csv, .tsv, .xlsx])
    }

    // MARK: JSON

    func testJSONSettingsCarryNoHeaderOrDelimiterAndNoNullTextUnlessAsked() {
        var mapping = ImportMapping()
        mapping.path = "/tmp/people.jsonl"
        mapping.format = .json
        mapping.targetSchema = "public"
        mapping.targetTable = "people"
        mapping.header = false
        mapping.delimiter = "|"

        let env = mapping.settings()
        XCTAssertEqual(env["IMPORT_FORMAT"], "json")
        XCTAssertEqual(env["TARGET_TABLE"], "people")
        XCTAssertNil(env["HEADER"], "JSON names its columns with its keys")
        XCTAssertNil(env["DELIMITER"], "JSON has no separator")
        XCTAssertNil(env["NULL_TEXT"], "blank leaves an empty string a value and only null NULL")

        mapping.nullText = "N/A"
        XCTAssertEqual(mapping.settings()["NULL_TEXT"], "N/A")
    }

    func testJSONColumnsCarryTheKeyNameSoAChangedFileIsRefused() throws {
        var mapping = ImportMapping()
        mapping.format = .json
        mapping.targetColumns = ["id", "email"]
        mapping.fields = [
            ImportField(source: 0, name: "id", include: true, target: "id"),
            ImportField(source: 1, name: "mail", include: true, target: "email"),
        ]
        let json = try XCTUnwrap(mapping.columnsJSON)
        let rows = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(json.utf8)) as? [[String: Any]])
        XCTAssertEqual(rows[1]["name"] as? String, "mail", "the key, not the target it maps to")
        XCTAssertEqual(rows[1]["target"] as? String, "email")
    }

    func testAnXLSXImportSendsItsHeaderToggleButNoDelimiter() {
        var mapping = ImportMapping()
        mapping.format = .xlsx
        mapping.delimiter = "|"
        XCTAssertNil(mapping.settings()["DELIMITER"], "a workbook has no separator to send")
        XCTAssertEqual(mapping.settings()["HEADER"], "1", "XLSX keeps its header toggle")
    }

    // MARK: SQL

    func testASQLImportSendsNoTargetAndNoRowSettings() {
        var mapping = ImportMapping()
        mapping.path = "/tmp/dump.sql"
        mapping.format = .sql
        mapping.targetCatalog = "hive"
        mapping.targetSchema = "analytics"
        mapping.targetTable = "people"
        mapping.nullText = "N/A"
        mapping.batchSize = 500
        mapping.onError = .commit
        mapping.foreignKeys = false
        mapping.targetColumns = ["id"]
        mapping.fields = [ImportField(source: 0, name: "id", include: true, target: "id")]

        XCTAssertEqual(mapping.settings(), [
            "IMPORT_PATH": "/tmp/dump.sql",
            "IMPORT_FORMAT": "sql",
            "ON_ERROR": "commit",
            "FOREIGN_KEYS": "0",
        ], "the file carries its own tables; the policy and the foreign-key switch still apply")
        XCTAssertNil(mapping.columnsJSON)
    }

    func testASQLImportNeedsOnlyAFile() {
        var mapping = ImportMapping()
        mapping.format = .sql
        XCTAssertFalse(mapping.ready)
        mapping.path = "/tmp/dump.sql"
        XCTAssertTrue(mapping.ready, "no table to name: the statements say where they write")
    }

    func testTheErrorWordsFollowTheFamily() {
        XCTAssertEqual(ImportErrorMode.skip.title(for: .csv), "Skip the row")
        XCTAssertEqual(ImportErrorMode.skip.title(for: .sql), "Skip the statement")
        XCTAssertTrue(ImportErrorMode.commit.detail(for: .json).hasPrefix("Rows before a bad row"))
        XCTAssertTrue(ImportErrorMode.commit.detail(for: .sql).hasPrefix("Statements before a bad statement"))
        // The rule does not change with the words: skip is still the one without a transaction.
        for format in ImportSourceFormat.allCases {
            XCTAssertTrue(ImportErrorMode.skip.detail(for: format).contains("No transaction"))
        }
    }

    func testTheFormatNotesSayWhatTheControlsDoNot() throws {
        XCTAssertNil(ImportSourceFormat.csv.note)
        let json = try XCTUnwrap(ImportSourceFormat.json.note)
        XCTAssertTrue(json.contains("empty string stays an empty string"), json)
        let sql = try XCTUnwrap(ImportSourceFormat.sql.note)
        XCTAssertTrue(sql.contains("all or nothing"), sql)
    }

    // MARK: What the engine answered

    func testASQLDoneEventCarriesStatementsAndNotRows() throws {
        let line = #"{"event":"done","statements":3,"mode":"stop","format":"sql","streams":false,"#
            + #""transaction":true,"disposition":"pending","errors":[],"errors_truncated":false,"#
            + #""stopped_at":null,"cancelled":false,"foreign_keys":"on","query_id":"q"}"#
        let outcome = ImportOutcome(done: try XCTUnwrap(EngineWire.event(in: Data(line.utf8))))
        XCTAssertEqual(outcome.statements, 3)
        XCTAssertEqual(outcome.rows, 0)
        XCTAssertTrue(outcome.transaction)
    }

    func testARowDoneEventCarriesItsWarnings() throws {
        let line = #"{"event":"done","rows":2,"table":"people","mode":"commit","format":"json","#
            + #""streams":true,"transaction":true,"disposition":"written","rejected":1,"#
            + #""errors":["line 4: bad"],"errors_truncated":true,"stopped_at":4,"cancelled":false,"#
            + #""warnings":["keys first seen after the rows that named the columns were not imported: 'x'"]}"#
        let outcome = ImportOutcome(done: try XCTUnwrap(EngineWire.event(in: Data(line.utf8))))
        XCTAssertNil(outcome.statements, "a row import has rows, not statements")
        XCTAssertEqual(outcome.rows, 2)
        XCTAssertEqual(outcome.rejected, 1)
        XCTAssertEqual(outcome.errors, ["line 4: bad"])
        XCTAssertTrue(outcome.errorsTruncated)
        XCTAssertEqual(outcome.stoppedAt, 4)
        XCTAssertEqual(outcome.warnings.count, 1)
    }

    func testTheStatementsKeyOfApplyChangesStillDecodes() throws {
        // `apply_changes` writes `statements` as an array of per-statement results. A property typed
        // for the integer `import_data` writes would throw here and the app would lose the whole
        // `done` event, which is the one that says the edits were committed.
        let line = #"{"event":"done","applied":2,"statements":[{"sql":"UPDATE t SET a = 1"},"#
            + #"{"sql":"UPDATE t SET a = 2"}],"transaction":true,"disposition":"written","query_id":"q"}"#
        let event = try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
        XCTAssertEqual(event.applied, 2)
        XCTAssertEqual(event.statements?.count, 2)
    }

    @MainActor
    func testTheFooterSaysWhatWasRunAndWhatWasSkipped() {
        var outcome = ImportOutcome()
        outcome.statements = 41
        outcome.errors = ["line 7: syntax error", "line 9: no such table"]
        outcome.warnings = ["one thing was left out"]
        outcome.transaction = true
        let text = ImportSheet.outcomeText(outcome)
        XCTAssertTrue(text.hasPrefix("Ran 41 statements"), text)
        XCTAssertTrue(text.contains("2 errors listed above"), text)
        XCTAssertTrue(text.contains("1 warning above"), text)
        XCTAssertTrue(ImportSheet.hasReport(outcome))

        outcome.errorsTruncated = true
        XCTAssertTrue(ImportSheet.outcomeText(outcome).contains("2+ errors"), "a cut list is a beginning")

        var rows = ImportOutcome()
        rows.rows = 5
        XCTAssertEqual(ImportSheet.outcomeText(rows), "Imported 5 rows into the target")
        XCTAssertFalse(ImportSheet.hasReport(rows))
    }

    // MARK: Reading a JSON file's keys

    func testThePreviewAsksTheEngineForOneSampleRowAndNeverAConnection() {
        let env = ImportJSONPreview.environment(path: "/tmp/people.json")
        XCTAssertEqual(env["IMPORT_PATH"], "/tmp/people.json")
        XCTAssertEqual(env["IMPORT_FORMAT"], "json")
        XCTAssertEqual(env["IMPORT_PREVIEW"], "1")
        XCTAssertEqual(env["IMPORT_PREVIEW_ROWS"], "1")
        XCTAssertNil(env["DRIVER"], "a preview opens the file and never a connection")
    }

    func testThePreviewReturnsTheKeysInTheOrderTheEngineSawThem() throws {
        let engine = MockEngine()
        engine.answer("import_data", with: .events([
            Event(event: "columns", columns: [.init(name: "id", type: "text"),
                                              .init(name: "email", type: "text")]),
            Event(event: "done"),
        ]))
        let result = load(engine)
        XCTAssertEqual(try result.get(), ["id", "email"])
        XCTAssertEqual(engine.calls.map(\.command), ["import_data"])
    }

    func testThePreviewSaysWhyTheEngineRefusedTheFile() {
        let engine = MockEngine()
        engine.answer("import_data", with: .failure(
            events: [Event(event: "error", message: "element 3 (line 3) is not an object")],
            reason: "stderr noise"))
        XCTAssertEqual(load(engine), .failure(.init(message: "element 3 (line 3) is not an object")))
    }

    func testAFileWithNoObjectsIsAFailureNotAnEmptyFieldList() {
        let engine = MockEngine()
        engine.answer("import_data", with: .events([Event(event: "columns", columns: []),
                                                     Event(event: "done")]))
        XCTAssertEqual(load(engine), .failure(.init(message: "The file has no JSON objects to read keys from.")))
    }

    /// The real engine, no connection and no database: the wire the mock scripts is the one the
    /// importer writes (`IMPORT_PREVIEW`), and a mock cannot prove that.
    func testTheRealEngineListsTheKeysOfAJSONLFileAndRefusesAnArrayOfNumbers() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-import-preview-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let lines = directory.appendingPathComponent("people.jsonl")
        try Data("{\"id\":1,\"nama\":\"a\"}\n{\"id\":2,\"kota\":null,\"nama\":\"b\"}\n".utf8).write(to: lines)
        let numbers = directory.appendingPathComponent("numbers.json")
        try Data("[1, 2, 3]".utf8).write(to: numbers)

        XCTAssertEqual(try load(RustEngine(), path: lines.path).get(), ["id", "nama", "kota"],
                       "the keys of every object, in the order they were first seen")
        guard case .failure(let failure) = load(RustEngine(), path: numbers.path) else {
            return XCTFail("an element that is not an object is refused")
        }
        XCTAssertTrue(failure.message.contains("not an object"), failure.message)
    }

    private func load(_ engine: any DatabaseEngine, path: String = "/tmp/people.json")
        -> Result<[String], ImportJSONPreview.Failure> {
        let done = expectation(description: "preview answered")
        var result: Result<[String], ImportJSONPreview.Failure>?
        ImportJSONPreview.load(path: path, engine: engine) {
            result = $0
            done.fulfill()
        }
        wait(for: [done], timeout: 5)
        return result ?? .failure(.init(message: "no answer"))
    }
}
