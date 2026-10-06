import XCTest

@testable import QueryHive

/// `Event` against the engine's own frozen output.
///
/// `tests/golden/**` is the recorded stdout of the Python engine for a fixed set of cases
/// (blueprint §1.8): it is the only place in the tree where a real line of this protocol is
/// written down. So it is the fixture, and these tests answer a question the app could not answer
/// before — does what the engine writes still decode into what the UI reads? A field that stopped
/// decoding does not crash: it arrives as `nil`, the panel shows an empty value, and nobody learns
/// anything until someone looks at the screen. That is the failure mode this file exists to catch.
///
/// What it cannot check is what `RustEngine` will *send*: the FFI's `run` returns lines this same
/// decoder has to read (`crates/qh-ffi/src/uniffi_api.rs`, "Why it returns JSON lines"), and the
/// day it streams typed events instead, this fixture stops describing the app's input.
final class EventDecodingTests: XCTestCase {
    /// One decoded event, with the recorded case it came from.
    private struct Recorded {
        let file: String
        let text: String
        let event: Event
    }

    /// The recorded cases, found from this file rather than from the current directory: `swift
    /// test` runs from the package directory today, and a test that depends on where it was
    /// started is a test that fails on the next tool.
    private static let goldenRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // QueryHiveTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // app
        .deletingLastPathComponent()  // the repository root
        .appendingPathComponent("tests/golden")

    /// Undoes the recording harness's placeholders, which exist so a recorded file is diffable
    /// (`tools/golden/record.py`: `"elapsed_ms": "<TIME>"`, `"query_id": "<QUERY_ID>"`, `<TMP>`).
    ///
    /// It has to be undone because `<TIME>` is a *string* where the wire has a number, so as
    /// recorded it is not a line this app ever sees. The other two are strings in both worlds and
    /// are replaced only so the assertions have something stable to compare against.
    private static func wire(_ recorded: String) -> Data {
        let text = recorded
            .replacingOccurrences(of: "\"<TIME>\"", with: "0")
            .replacingOccurrences(of: "<QUERY_ID>", with: "20260131_120412_00042_abcde")
            .replacingOccurrences(of: "<TMP>", with: "/tmp")
        return Data(text.utf8)
    }

    /// Every recorded line, decoded. Nil-decoding lines are returned with their text so a caller
    /// can say which one failed.
    private func recorded() throws -> (decoded: [Recorded], unreadable: [(file: String, text: String)]) {
        guard let walker = FileManager.default.enumerator(at: Self.goldenRoot,
                                                          includingPropertiesForKeys: nil) else {
            throw XCTSkip("no recorded cases at \(Self.goldenRoot.path)")
        }
        var decoded: [Recorded] = []
        var unreadable: [(String, String)] = []
        for case let file as URL in walker where file.pathExtension == "ndjson" {
            let text = try String(contentsOf: file, encoding: .utf8)
            for line in text.split(separator: "\n") where !line.trimmingCharacters(in: .whitespaces).isEmpty {
                let raw = String(line)
                if let event = EngineWire.event(in: Self.wire(raw)) {
                    decoded.append(Recorded(file: file.lastPathComponent, text: raw, event: event))
                } else {
                    unreadable.append((file.lastPathComponent, raw))
                }
            }
        }
        XCTAssertFalse(decoded.isEmpty, "the golden directory exists but holds no events")
        return (decoded, unreadable)
    }

    /// The events of one name, in the order the files were walked.
    private func events(_ name: String, in decoded: [Recorded]) -> [Event] {
        decoded.map(\.event).filter { $0.event == name }
    }

    func testEveryLineTheEngineEverWroteDecodesIntoAnEvent() throws {
        let (decoded, unreadable) = try recorded()

        XCTAssertEqual(unreadable.map { "\($0.file): \($0.text.prefix(120))" }, [],
                       "every recorded line is an event the app can read")
        XCTAssertGreaterThan(decoded.count, 100, "the fixture is the frozen record, not a sample")

        // The event names are themselves a contract (`lib.rs`'s table of command → events). A name
        // that stopped arriving would be a line that failed to decode above, so this only says
        // which names the record still covers.
        let names = Set(decoded.map(\.event.event))
        for expected in ["catalogs", "columns", "count", "done", "drivers", "error", "objects",
                         "progress", "rows", "schemas", "start", "step", "tables", "test"] {
            XCTAssertTrue(names.contains(expected), "the record no longer covers '\(expected)'")
        }
    }

    func testTheSnakeCaseKeysReachTheirProperties() throws {
        // Four keys with an underscore, each read through a differently named property, each one a
        // decoding bug at some point in this migration. A `nil` here reads as "the engine sent
        // nothing" rather than as a decoder that stopped converting, which is why they are
        // asserted one by one.
        let (decoded, _) = try recorded()

        let objects = events("objects", in: decoded)
        XCTAssertFalse(objects.isEmpty)
        // The frozen Trino case, not a live one: both declare the same columns since the live
        // fixtures were added, so the three-row shape is what names this one.
        let typed = try XCTUnwrap(objects.first {
            $0.objectColumns == ["Name", "Type"] && $0.data?.count == 3
        }, "the Trino case declares its own object columns")
        XCTAssertEqual(typed.data?[2], ["legacy", ""], "an empty string stays empty, not nil")

        // An `explain` done carries no files, so this is the export half of the record. The
        // `elapsed_ms` check lives below on the explain case, which is the one that sends it.
        let exports = events("done", in: decoded).filter { !($0.files ?? []).isEmpty }
        XCTAssertFalse(exports.isEmpty)
        let csv = try XCTUnwrap(exports.first { event in
            (event.files ?? []).contains { $0.name == "people.csv" }
        }, "the CSV case reports the file it wrote")
        XCTAssertEqual(csv.queryId, "20260131_120412_00042_abcde")
        let explainDone = try XCTUnwrap(events("done", in: decoded).first { $0.elapsedMs != nil },
                                        "the explain cases report how long they took")
        XCTAssertGreaterThanOrEqual(explainDone.elapsedMs ?? -1, 0,
                                    "`elapsed_ms` is a number on the wire, not a string")
        XCTAssertEqual(csv.cancelled, false)
        XCTAssertEqual(csv.warnings, [])

        let tests = events("test", in: decoded)
        XCTAssertFalse(tests.isEmpty)
        for event in tests {
            XCTAssertNotNil(event.catalogCount,
                            "`catalog_count` is not `catalogs`, and one key cannot be two types")
            XCTAssertNotNil(event.ok)
        }
        XCTAssertTrue(tests.allSatisfy { ($0.host ?? "").isEmpty == false })
    }

    func testTheCountCommandAnswersInRows() throws {
        // Worth pinning because it is the one event whose payload key is not the event's own name:
        // `{"event": "count", "rows": 4321}`. It is also the decode half of a real defect — the
        // footer read a `count` property no engine ever writes, so the total silently stayed nil.
        // That property is gone; the key is `rows`, and `AppModel.applyCountEvent` is what reads it.
        let (decoded, _) = try recorded()
        let counts = events("count", in: decoded)

        XCTAssertFalse(counts.isEmpty)
        for event in counts {
            XCTAssertNotNil(event.rows, "the number arrives in `rows`")
        }
        XCTAssertTrue(counts.contains { $0.rows == 4321 }, "and it is the number the server gave")
    }

    func testNullsInsideRowsSurviveAsNulls() throws {
        // The grid's one rule about values: a NULL cell and an empty string are different things,
        // and a decoder that folded them together would show the user "" where the database said
        // nothing at all. `[[String?]]` is the type that keeps them apart.
        let (decoded, _) = try recorded()
        let rows = events("rows", in: decoded)
        // Exactly one NULL: a live explain row carries several, and the point here is that one
        // nil survives decoding as nil — counting is only meaningful against the case with one.
        let withNulls = try XCTUnwrap(rows.first {
            ($0.data ?? []).contains { $0.filter { $0 == nil }.count == 1 }
        }, "the record has a row with exactly one NULL cell in it")
        let row = try XCTUnwrap(withNulls.data?.first { $0.filter { $0 == nil }.count == 1 })

        XCTAssertEqual(row.filter { $0 == nil }.count, 1)
        XCTAssertTrue(row.contains(where: { $0 == nil }), "the NULL cell is nil, not \"\"")
    }

    func testAFailureLineArrivesAsAnEventWithItsMessage() throws {
        let (decoded, _) = try recorded()
        let failures = events("error", in: decoded)

        XCTAssertFalse(failures.isEmpty)
        for event in failures {
            XCTAssertNotNil(event.message, "the message is what the error bar shows")
        }
    }

    func testALineThatIsNotAnEventDecodesToNothingRatherThanThrowing() {
        // Both engines need the same answer here, and it is `nil`: they report an unreadable line
        // to the user instead of failing the run. Valid JSON with no `event` field is not an event,
        // and that is the easier mistake to make.
        XCTAssertNil(EngineWire.event(in: Data("usage: queryhive-engine objects".utf8)))
        XCTAssertNil(EngineWire.event(in: Data("".utf8)))
        XCTAssertNil(EngineWire.event(in: Data("{\"message\": \"no event field\"}".utf8)))
    }

    // MARK: History and saved queries
    //
    // These four commands are the only ones with no case in `tests/golden`: the corpus predates
    // them, so the fixture here is the engine's own output, copied verbatim from a run against a
    // temporary database. The failure this still catches is the quiet one — a field the engine
    // renamed arrives as `nil`, the panel shows an empty cell, and nothing anywhere says so.

    func testTheHistoryEventDecodesIntoItsRecords() throws {
        let event = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"history","entries":[{"id":"01a0eaa3-5927-767a-9e56-d0ce0e8d95a7","connection_id":"8d7e7312-a9d5-43df-9b4e-8e64b964a587","sql":"SELECT count(*) FROM t","started_at":1790642968000,"elapsed_ms":42,"row_count":1,"outcome":"ok","error":null,"deleted":false,"version":1}]}"#.utf8)))
        let entry = try XCTUnwrap(event.entries?.first)

        XCTAssertEqual(entry.sql, "SELECT count(*) FROM t")
        XCTAssertEqual(entry.startedAt, 1_790_642_968_000)
        XCTAssertEqual(entry.elapsedMs, 42)
        XCTAssertEqual(entry.rowCount, 1)
        XCTAssertEqual(entry.outcome, "ok")
        XCTAssertEqual(entry.deleted, false)
        XCTAssertEqual(entry.version, 1)
        XCTAssertEqual(entry.connectionId, "8d7e7312-a9d5-43df-9b4e-8e64b964a587",
                       "connection_id reaches connectionId, which is also the Keychain account")
    }

    func testAnUnfinishedHistoryEntryKeepsItsMissingAnswerAsNothing() throws {
        // A row can be written before its run has an answer, and "no answer yet" is not the same
        // as zero: a zero would draw a finished run that took no time and returned no rows.
        let event = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"history","entries":[{"id":"x","connection_id":null,"sql":"SELECT 1","started_at":1,"elapsed_ms":null,"row_count":null,"outcome":null,"error":null,"deleted":false,"version":1}]}"#.utf8)))
        let entry = try XCTUnwrap(event.entries?.first)

        XCTAssertNil(entry.outcome)
        XCTAssertNil(entry.elapsedMs)
        XCTAssertNil(entry.rowCount)
        XCTAssertNil(entry.connectionId)
    }

    func testTheSavedQueryEventsDecodeIntoTheirRecords() throws {
        let listed = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"saved_queries","queries":[{"id":"01a0ea97-d51a-74f1-a00c-f103e4770ad3","name":"Penerima 2026","sql":"SELECT * FROM penerima_manfaat","connection_id":"8d7e7312-a9d5-43df-9b4e-8e64b964a587","folder_id":null,"favourite":true,"deleted":false,"version":1}]}"#.utf8)))
        let query = try XCTUnwrap(listed.queries?.first)

        XCTAssertEqual(query.name, "Penerima 2026")
        XCTAssertEqual(query.version, 1)
        XCTAssertNil(query.folderId)
        XCTAssertTrue(query.favourite, "the flag decides whether the sidebar lists it")

        // The single-row reply is a different shape: the record sits under `query`, and a `get`
        // that found nothing leaves it null rather than failing the command.
        let single = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"saved_query","action":"get","id":"01a0ea97-d51a-74f1-a00c-f103e4770ad3","query":null}"#.utf8)))
        XCTAssertEqual(single.action, "get")
        XCTAssertNil(single.query)

        let renamed = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"saved_query","action":"rename","id":"01a0ea97-d51a-74f1-a00c-f103e4770ad3","renamed":true}"#.utf8)))
        XCTAssertEqual(renamed.renamed, true)

        let written = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"history_entry","id":"01a0eaa3-5927-767a-9e56-d0ce0e8d95a7","merged":false}"#.utf8)))
        XCTAssertEqual(written.merged, false)

        let cleared = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"history_clear","cleared":2}"#.utf8)))
        XCTAssertEqual(cleared.cleared, 2)
    }

    func testTheSessionEventsDecodeIntoTheTabsTheyCarry() throws {
        // The shape the app wrote is the shape it reads back: the engine stores the blob opaquely
        // and hands it over parsed. A key renamed here is a tab that comes back without its SQL,
        // and nothing would say so — the field would just be empty.
        let saved = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"session","action":"load","saved":true,"tabs":[{"id":"01A0EAA3-0000-7000-8000-000000000001","title":"Query 1","sql":"SELECT 1","connection":null,"destination":"file","format":"csv","outputName":"export","outputDirectory":null,"rowLimit":1000,"contextDatabase":"","contextSchema":"","targetCatalog":"","targetSchema":"","targetTable":"","writeMode":"create"}],"active_tab_id":"01A0EAA3-0000-7000-8000-000000000001"}"#.utf8)))
        XCTAssertEqual(saved.saved, true)
        let tab = try XCTUnwrap(saved.tabs?.first)
        XCTAssertEqual(tab.sql, "SELECT 1")
        XCTAssertEqual(tab.title, "Query 1")
        XCTAssertEqual(tab.rowLimit, 1000)
        XCTAssertNil(tab.connection)
        XCTAssertEqual(saved.activeTabId, "01A0EAA3-0000-7000-8000-000000000001",
                       "`active_tab_id` reaches `activeTabId`")

        // A fresh install: no session is a normal answer, and there is nothing to restore.
        let none = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"session","action":"load","saved":false}"#.utf8)))
        XCTAssertEqual(none.saved, false)
        XCTAssertNil(none.tabs)
    }

    // MARK: - the metadata events (W11-T1)

    func testTheMetadataEventsDecodeUnderTheKeysNothingElseUses() throws {
        // `kinds` rides beside `names`, `fields` is not `columns`, `object` is not `table`, and
        // `object_kind` reaches `objectKind`. A key that stopped decoding arrives as `nil` and the
        // tree just shows no kind, so each one is asserted by what it carries.
        let tables = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"tables","names":["a","b","c"],"kinds":["table","view",null]}"#.utf8)))
        XCTAssertEqual(tables.names, ["a", "b", "c"])
        XCTAssertEqual(tables.kinds?.count, 3)
        XCTAssertEqual(tables.kinds?[1], "view")
        XCTAssertEqual(tables.kinds?[2] ?? nil, nil, "a kind the server did not say stays unsaid")
        let plain = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"tables","names":["a"]}"#.utf8)))
        XCTAssertNil(plain.kinds, "no `OBJECT_KINDS`, no `kinds`: the old listing decodes as it did")

        let columns = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"table_columns","object":{"catalog":null,"schema":"public","table":"t"},"fields":[{"name":"id","type":"bigint","nullable":false,"default":null,"extra":"identity"},{"name":"doubled","type":"bigint","nullable":true,"default":"(id * 2)","extra":"generated"},{"name":"u","type":"text","nullable":null,"default":null,"extra":""}],"truncated":false}"#.utf8)))
        XCTAssertEqual(columns.object, Event.ObjectName(catalog: nil, schema: "public", table: "t"))
        XCTAssertEqual(columns.fields?.map(\.name), ["id", "doubled", "u"])
        XCTAssertEqual(columns.fields?[0].nullable, false)
        XCTAssertEqual(columns.fields?[0].extra, "identity")
        XCTAssertEqual(columns.fields?[1].default, "(id * 2)")
        XCTAssertEqual(columns.fields?[1].extra, "generated", "the expression is not a default, and `extra` says so")
        XCTAssertNil(columns.fields?[2].nullable, "an unknown nullability is not a `false`")
        XCTAssertEqual(columns.truncated, false)

        let ddl = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"table_ddl","object":{"catalog":"hive","schema":"a","table":"v"},"object_kind":"view","ddl":"CREATE VIEW v AS SELECT 1;\n","truncated":false,"redacted":true}"#.utf8)))
        XCTAssertEqual(ddl.objectKind, "view")
        XCTAssertEqual(ddl.ddl, "CREATE VIEW v AS SELECT 1;\n")
        XCTAssertEqual(ddl.redacted, true)
        XCTAssertEqual(ddl.object?.catalog, "hive")
    }

    func testTheExecutionLogDecodesWithItsChainWhetherOrNotItVerifies() throws {
        let line = #"{"event":"execution_log","decisions":[{"seq":12,"id":"01a0ea97-d51a-74f1-a00c-f103e4770ad3","at":1790000000000,"safe_mode":"read_only","decision":"refused","statement_kind":"dml","statement_index":1,"statement_hash":"ab12","reason":"this connection is read-only"}],"chain":{"verified":true,"rows":214},"writer":true}"#
        let log = try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
        let decision = try XCTUnwrap(log.decisions?.first)
        XCTAssertEqual(decision.seq, 12)
        XCTAssertEqual(decision.safeMode, "read_only")
        XCTAssertEqual(decision.statementKind, "dml")
        XCTAssertEqual(decision.statementIndex, 1)
        XCTAssertEqual(decision.statementHash, "ab12")
        XCTAssertEqual(decision.reason, "this connection is read-only")
        XCTAssertEqual(log.chain, Event.LogChain(verified: true, rows: 214, seq: nil, detail: nil))
        XCTAssertEqual(log.writer, true)

        // A broken chain still carries its rows: seeing them is how it is diagnosed. And a caller
        // that asked not to verify gets neither a yes nor a no.
        let broken = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"execution_log","decisions":[],"chain":{"verified":false,"seq":7,"detail":"chain broken"},"writer":false}"#.utf8)))
        XCTAssertEqual(broken.chain?.verified, false)
        XCTAssertEqual(broken.chain?.seq, 7)
        XCTAssertEqual(broken.writer, false)
        let unverified = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"execution_log","decisions":[],"chain":{"verified":null},"writer":true}"#.utf8)))
        XCTAssertNil(unverified.chain?.verified)
    }

    func testAnErrorCarriesItsHostKeyOnlyWhenTheEngineWasAskedFor() throws {
        // The key W11-T2 will send under `SSH_HOST_KEY_DETAIL=1`. It is read here because this
        // struct is where a key is added once, and the connection work then adds none.
        let line = #"{"event":"error","message":"the bastion's host key is not known","host_key":{"state":"unknown","host":"bastion.corp","port":22,"alias":"prod-bastion","key_type":"ssh-ed25519","fingerprint":"SHA256:abc","app_known_hosts":"/Users/x/Library/Application Support/QueryHive/known_hosts","ca_covered":false,"recorded":[{"fingerprint":"SHA256:def","key_type":"ssh-rsa","source":"user","path":"/Users/x/.ssh/known_hosts","line":3}],"pinned":null}}"#
        let failed = try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
        let key = try XCTUnwrap(failed.hostKey)
        XCTAssertEqual(key.state, "unknown")
        XCTAssertEqual(key.host, "bastion.corp")
        XCTAssertEqual(key.port, 22)
        XCTAssertEqual(key.keyType, "ssh-ed25519")
        XCTAssertEqual(key.appKnownHosts, "/Users/x/Library/Application Support/QueryHive/known_hosts")
        XCTAssertEqual(key.caCovered, false)
        XCTAssertEqual(key.recorded?.first?.source, "user")
        XCTAssertEqual(key.recorded?.first?.line, 3)
        XCTAssertNil(key.pinned)
        XCTAssertEqual(failed.message, "the bastion's host key is not known")

        let plain = try XCTUnwrap(EngineWire.event(in: Data(#"{"event":"error","message":"boom"}"#.utf8)))
        XCTAssertNil(plain.hostKey, "an error without the setting decodes exactly as it did")
    }

    func testTheRecordedMetadataCasesDecodeIntoWhatTheTreeAndTheDDLTabRead() throws {
        // The three servers' real answers, frozen under `tests/golden/{columns,ddl,tables}`: the
        // typed shapes above are only worth something if what an engine really wrote fits them.
        let (decoded, _) = try recorded()
        let described = events("table_columns", in: decoded)
        XCTAssertGreaterThanOrEqual(described.count, 3, "one `columns` case per driver")
        for event in described {
            XCTAssertFalse((event.fields ?? []).isEmpty)
            XCTAssertNotNil(event.object?.table)
            XCTAssertTrue((event.fields ?? []).allSatisfy { !$0.type.isEmpty })
        }
        // Each driver names only the slots it has: MySQL's database is its `catalog`, and it has
        // no schema.
        XCTAssertTrue(described.contains { $0.object?.catalog == "qh" && $0.object?.schema == nil })
        XCTAssertTrue(described.contains { $0.object?.catalog == nil && $0.object?.schema == "public" })
        XCTAssertTrue(described.contains { $0.object?.catalog == "tpch" && $0.object?.schema == "tiny" })

        let written = events("table_ddl", in: decoded)
        XCTAssertGreaterThanOrEqual(written.count, 3)
        for event in written {
            XCTAssertEqual(event.objectKind, "table")
            XCTAssertTrue((event.ddl ?? "").contains("CREATE TABLE"))
            XCTAssertEqual(event.redacted, false)
        }

        let kinds = events("tables", in: decoded).filter { $0.kinds != nil }
        XCTAssertGreaterThanOrEqual(kinds.count, 3, "one `OBJECT_KINDS` case per driver")
        for event in kinds {
            XCTAssertEqual(event.kinds?.count, event.names?.count, "`kinds` runs beside `names`")
        }
    }
}
