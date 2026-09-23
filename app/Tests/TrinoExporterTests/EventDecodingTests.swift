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
        .deletingLastPathComponent()  // TrinoExporterTests
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
        let typed = try XCTUnwrap(objects.first { $0.objectColumns == ["Name", "Type"] },
                                  "the Trino case declares its own object columns")
        XCTAssertEqual(typed.data?.count, 3)
        XCTAssertEqual(typed.data?[2], ["legacy", ""], "an empty string stays empty, not nil")

        let exports = events("done", in: decoded).filter { !($0.files ?? []).isEmpty }
        XCTAssertFalse(exports.isEmpty)
        let csv = try XCTUnwrap(exports.first { event in
            (event.files ?? []).contains { $0.name == "people.csv" }
        }, "the CSV case reports the file it wrote")
        XCTAssertEqual(csv.queryId, "20260131_120412_00042_abcde")
        XCTAssertEqual(csv.elapsedMs, 0, "`elapsed_ms` is a number on the wire, not a string")
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
        // `{"event": "count", "rows": 4321}`. `Event` also declares a `count` property for a shape
        // neither engine writes, and a test that asserted *that* would be asserting fiction.
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
        let withNulls = try XCTUnwrap(rows.first { ($0.data ?? []).contains { $0.contains(nil) } },
                                      "the record has a row with a NULL cell in it")
        let row = try XCTUnwrap(withNulls.data?.first { $0.contains(nil) })

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
}
