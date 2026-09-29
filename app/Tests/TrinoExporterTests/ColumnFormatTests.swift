import XCTest

@testable import QueryHive

/// Per-column display formats: each format's rendering, and the store that keeps a choice.
///
/// The contract under test is "rendering only": a format changes the characters on screen and never
/// the value. Every unrecognised input comes back unchanged, because a display format that rejected
/// values would be a second way for the grid to be wrong.
final class ColumnFormatTests: XCTestCase {
    // MARK: Rendering

    func testUUIDIsCanonicalisedFromEitherSpelling() {
        let canonical = "550e8400-e29b-41d4-a716-446655440000"
        XCTAssertEqual(ColumnFormat.uuid.render("550e8400e29b41d4a716446655440000"), canonical)
        XCTAssertEqual(ColumnFormat.uuid.render(canonical), canonical)
        XCTAssertEqual(ColumnFormat.uuid.render("not-a-uuid"), "not-a-uuid")
    }

    func testUnixSecondsAndMillisecondsLandOnTheSameInstant() {
        XCTAssertEqual(ColumnFormat.unixTimestamp.render("1700000000"), "2023-11-14 22:13:20")
        // The other plausible unit: 13 digits is milliseconds, or a seconds reading would be the
        // year 55,000.
        XCTAssertEqual(ColumnFormat.unixTimestamp.render("1700000000000"), "2023-11-14 22:13:20")
        XCTAssertEqual(ColumnFormat.unixTimestamp.render("0"), "1970-01-01 00:00:00")
        XCTAssertEqual(ColumnFormat.unixTimestamp.render("abc"), "abc")
    }

    func testTextDecodesBytesOnlyForABinaryColumn() {
        let hex = "6869"   // "hi"
        XCTAssertEqual(ColumnFormat.text.render(hex, type: "bytea"), "hi")
        XCTAssertEqual(ColumnFormat.text.render(hex, type: "varbinary"), "hi")
        // A text column has nothing to decode, so the value is left alone.
        XCTAssertEqual(ColumnFormat.text.render(hex, type: "varchar"), hex)
    }

    func testJSONIsCollapsedToOneLine() {
        let rendered = ColumnFormat.json.render(#"{"b":2,"a":1}"#)
        XCTAssertFalse(rendered.contains("\n"), "a grid row must not be broken by a pretty object")
        XCTAssertTrue(rendered.contains("\"a\""))
        XCTAssertEqual(ColumnFormat.json.render("not json"), "not json")
    }

    func testRawLeavesTheValueAlone() {
        XCTAssertEqual(ColumnFormat.raw.render("anything\nat all"), "anything\nat all")
    }

    // MARK: Store

    func testStoreRoundTripsAndRawClearsTheEntry() throws {
        let name = "qh-columnformat-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }

        let id = try XCTUnwrap(ColumnFormatStore.identity(
            connection: UUID(), table: "hive.analytics.penerima", column: "nik"))
        XCTAssertEqual(ColumnFormatStore.format(id, in: defaults), .raw)

        ColumnFormatStore.set(.uuid, for: id, in: defaults)
        XCTAssertEqual(ColumnFormatStore.format(id, in: defaults), .uuid)

        // Raw is stored as the absence of an entry, so clearing leaves nothing behind.
        ColumnFormatStore.set(.raw, for: id, in: defaults)
        XCTAssertEqual(ColumnFormatStore.format(id, in: defaults), .raw)
        XCTAssertNil(defaults.object(forKey: ColumnFormatStore.defaultsKey))
    }

    func testTwoColumnsOfOneTableKeepTheirOwnFormats() throws {
        let name = "qh-columnformat-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let connection = UUID()
        let nik = try XCTUnwrap(ColumnFormatStore.identity(connection: connection,
                                                           table: "t", column: "nik"))
        let at = try XCTUnwrap(ColumnFormatStore.identity(connection: connection,
                                                          table: "t", column: "created_at"))
        ColumnFormatStore.set(.uuid, for: nik, in: defaults)
        ColumnFormatStore.set(.unixTimestamp, for: at, in: defaults)
        XCTAssertEqual(ColumnFormatStore.format(nik, in: defaults), .uuid)
        XCTAssertEqual(ColumnFormatStore.format(at, in: defaults), .unixTimestamp)
    }

    func testIdentityNeedsAConnectionAndATable() {
        XCTAssertNil(ColumnFormatStore.identity(connection: nil, table: "t", column: "c"))
        XCTAssertNil(ColumnFormatStore.identity(connection: UUID(), table: nil, column: "c"))
        XCTAssertNil(ColumnFormatStore.identity(connection: UUID(), table: "t", column: ""))
        XCTAssertNotNil(ColumnFormatStore.identity(connection: UUID(), table: "t", column: "c"))
    }

    // MARK: The viewer's offer

    /// The viewer only shows a mode the value supports, and the format's identity only when the
    /// cell can be filed somewhere.
    func testTheViewerOffersOnlyTheModesTheValueSupports() {
        let json = CellValueViewer(value: #"{"a":1}"#, column: "payload", type: "json")
        XCTAssertEqual(json.available, [.text, .tree])

        let blob = CellValueViewer(value: "00ff10", column: "data", type: "bytea")
        XCTAssertEqual(blob.available, [.text, .hex])

        let plain = CellValueViewer(value: "hello", column: "nama", type: "varchar")
        XCTAssertEqual(plain.available, [.text])

        // Over the parse limit the tree is not offered at all — the escape is Text, as the docs say.
        let big = "[" + String(repeating: "1,", count: GridValue.parseLimit) + "1]"
        let tooBig = CellValueViewer(value: big, column: "payload", type: "json")
        XCTAssertEqual(tooBig.available, [.text])
    }
}
