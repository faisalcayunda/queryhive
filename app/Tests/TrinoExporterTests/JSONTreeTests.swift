import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The JSON tree: what it parses, what it keeps, where it stops, and how it searches.
///
/// The tree is a reading surface over untrusted text, so the two properties worth pinning down are
/// fidelity — it shows the value the server sent, in the order the server sent it — and bounded
/// cost: over `GridValue.parseLimit` it builds nothing, and past `JSONTree.nodeLimit` it stops with
/// a marker rather than a tree that quietly ends.
final class JSONTreeTests: XCTestCase {
    // MARK: Parsing

    func testTheTreeKeepsTheServersKeyOrder() throws {
        // `JSONSerialization` would return an `NSDictionary` here, and this object would come back
        // alphabetised. The parser exists for exactly this: the tree must not reorder the value it
        // claims to show.
        let node = try root(#"{"b":1,"a":2,"c":3}"#)
        XCTAssertEqual(node.kind, .object)
        XCTAssertEqual(node.children.map(\.label), ["b", "a", "c"])
    }

    func testScalarsKeepTheirSpelling() throws {
        let node = try root(#"{"a":1.10,"b":1e3,"c":-0.0,"d":"x\ny","e":true,"f":null}"#)
        XCTAssertEqual(node.children.map(\.value), ["1.10", "1e3", "-0.0", "x\ny", "true", "null"])
        XCTAssertEqual(node.children.map(\.kind),
                       [.number, .number, .number, .string, .bool, .null])
    }

    func testNestedContainersAndArrayLabels() throws {
        let node = try root(#"{"rows":[{"nama":"Sukamaju"},{"nama":"Cibadak"}]}"#)
        let rows = node.children[0]
        XCTAssertEqual(rows.kind, .array)
        XCTAssertEqual(rows.children.map(\.label), ["[0]", "[1]"])
        XCTAssertEqual(rows.children[0].children[0].label, "nama")
    }

    func testAUnicodeEscapeBecomesTheCharacterItNames() throws {
        let node = try root(#"{"nama":"Sukamaju \u2014 \ud83c\udf3e"}"#)
        XCTAssertEqual(node.children[0].value, "Sukamaju — 🌾")
    }

    func testANonJSONValueHasNoTree() {
        // A PostgreSQL array literal is the real case (`docs/golden-deltas.md` D-9): it is a
        // structured column that is not JSON, and the reader stays on Text for it.
        if case .notJSON = JSONTree.build(from: "{1,NULL,3}") { return }
        XCTFail("a PostgreSQL array literal must not parse as JSON")
    }

    func testATrailingValueIsNotOneJSONValue() {
        if case .notJSON = JSONTree.build(from: "1 2") { return }
        XCTFail("two fragments are not one JSON value")
    }

    // MARK: Caps

    func testAValueOverTheParseLimitIsTooLarge() {
        let big = "[" + String(repeating: "1,", count: GridValue.parseLimit) + "1]"
        XCTAssertGreaterThan((big as NSString).length, GridValue.parseLimit)
        XCTAssertEqual(JSONTree.build(from: big), .tooLarge)
    }

    func testTheNodeCapStopsBuildingAndSaysHowManyWereLeftOut() throws {
        let count = JSONTree.nodeLimit + 500
        let json = "[" + (0..<count).map(String.init).joined(separator: ",") + "]"
        XCTAssertLessThan((json as NSString).length, GridValue.parseLimit, "the cap, not the limit")
        let node = try root(json)

        XCTAssertEqual(node.count(), JSONTree.nodeLimit, "the tree built more nodes than its cap")
        let marker = try XCTUnwrap(node.children.last)
        XCTAssertTrue(marker.isMarker)
        XCTAssertTrue(marker.value.contains("more not shown"))
        // Markers are not real nodes, so the count above excluded them.
        XCTAssertGreaterThan(node.count(includingMarkers: true), JSONTree.nodeLimit)
    }

    func testAShortValueIsNotTruncated() throws {
        let node = try root(#"[1,2,3]"#)
        XCTAssertFalse(node.children.contains(where: { $0.isMarker }))
    }

    // MARK: Display

    func testAStringRowEscapesItsControlCharacters() throws {
        let node = try root(#"{"a":"x\ny"}"#)
        XCTAssertEqual(JSONTree.display(node.children[0]), "\"x\\ny\"")
    }

    func testALongScalarIsShortenedForDisplayOnly() throws {
        let long = String(repeating: "x", count: JSONTree.scalarDisplayLimit + 50)
        let node = try root(#"{"a":"\#(long)"}"#)
        // The node keeps the whole value, so search can still see it...
        XCTAssertEqual(node.children[0].value.count, long.count)
        // ...and only the row's rendering is cut.
        XCTAssertTrue(JSONTree.display(node.children[0]).hasSuffix("…"))
        XCTAssertLessThan(JSONTree.display(node.children[0]).count, long.count + 3)
    }

    // MARK: Search

    func testSearchFindsKeysAndValuesAndOpensTheirAncestors() throws {
        let node = try root(#"{"kecamatan":["Sukamaju","Cibadak"],"kode":"32.01"}"#)
        let kecamatan = node.children[0]   // the array, labelled by its key
        let sukamaju = kecamatan.children[0]

        let found = try XCTUnwrap(JSONTree.search(in: node, query: "sukamaju"))
        XCTAssertEqual(found.hits, [sukamaju.id])
        XCTAssertTrue(found.expand.contains(node.id))
        XCTAssertTrue(found.expand.contains(kecamatan.id))

        // A key matches too, and only its own ancestors are opened.
        let byKey = try XCTUnwrap(JSONTree.search(in: node, query: "KODE"))
        XCTAssertEqual(byKey.hits, [node.children[1].id])
        XCTAssertFalse(byKey.expand.contains(kecamatan.id))
    }

    func testSearchOfAnEmptyQueryIsNoSearchRatherThanNoMatches() {
        // The distinction the view draws: an empty query shows no count, a real miss shows "No
        // matches". `nil` is the first.
        let node = JSONTree.Node(id: 0, kind: .object, label: "", value: "0 keys", children: [])
        XCTAssertNil(JSONTree.search(in: node, query: "   "))
    }

    // MARK: Render

    /// Draws the tree offscreen and leaves a PNG behind. What it proves: the tree lays out and
    /// draws rows rather than a blank panel. What it does not prove: that the app shows it on
    /// screen. The image is the deliverable, the assertion is a smoke test.
    @MainActor
    func testTheTreeRenders() throws {
        let node = try root(#"{"kabupaten":"Bandung","kecamatan":["Sukamaju","Cibadak","Mekarsari"],"jumlah_jiwa":48320,"aktif":true}"#)
        let host = NSHostingView(rootView: JSONTreeView(root: node)
            .frame(width: 520, height: 300)
            .background(Color.white))
        host.frame = NSRect(x: 0, y: 0, width: 520, height: 300)
        host.layoutSubtreeIfNeeded()

        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds),
                                "json-tree: no bitmap to draw into")
        host.cacheDisplay(in: host.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]),
                                 "json-tree could not be encoded as a PNG")

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("json-tree.png")
        try data.write(to: path)
        print("rendered json-tree.png to \(path.path)")
    }

    // MARK: Helpers

    private func root(_ json: String) throws -> JSONTree.Node {
        guard case .tree(let node) = JSONTree.build(from: json) else {
            throw XCTSkip("\(json) did not parse as JSON")
        }
        return node
    }

    private static var outputDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }
}
