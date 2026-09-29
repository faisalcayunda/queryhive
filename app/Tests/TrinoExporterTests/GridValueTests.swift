import XCTest

@testable import QueryHive

/// The two limits on the structured-cell reader, and the marker that keeps truncation honest.
///
/// A cell is untrusted input: it comes from a server, and a JSON value of a few megabytes is a
/// normal thing to find in a warehouse. Parsing it, or laying it out, on the main thread is a hang
/// the user cannot cancel — so `GridValue` refuses to parse above one limit and truncates above
/// another, and this is the file that keeps those numbers meaning what their names say.
final class GridValueTests: XCTestCase {
    func testAValueOverTheParseLimitIsNotParsed() {
        let small = #"{"a":1}"#
        XCTAssertNotNil(GridValue.prettyPrinted(small), "a small object still formats")

        // A JSON array long enough to be over the limit but still valid JSON, so the only reason
        // it is refused is the limit and not a parse error.
        let big = "[" + String(repeating: "1,", count: GridValue.parseLimit) + "1]"
        XCTAssertGreaterThan((big as NSString).length, GridValue.parseLimit)
        XCTAssertNil(GridValue.prettyPrinted(big), "over the parse limit no object is built")
    }

    func testAnOverlongValueIsTruncatedWithAMarkerRatherThanSilently() {
        let short = "hello"
        XCTAssertEqual(GridValue.displayText(short), short, "a short value is shown whole")

        let long = String(repeating: "x", count: GridValue.textLimit + 10)
        let shown = GridValue.displayText(long)
        XCTAssertTrue(shown.contains("truncated for display"),
                      "the tail is cut with a marker, not quietly")
        XCTAssertTrue(shown.hasSuffix("Copy keeps the whole value."))
        XCTAssertLessThan((shown as NSString).length, (long as NSString).length + 200,
                          "the marker is a sentence, not a second copy of the value")
    }

    func testAStructuredColumnStaysOpenableEvenWhenItsValueIsTooBigToParse() {
        // The escape the limit leaves: a structured cell always opens, and the reader shows the
        // server's raw text when it will not parse. A cell that became unopenable would be a limit
        // that hides data instead of bounding work.
        let big = "{" + String(repeating: "1", count: GridValue.parseLimit + 1) + "}"
        XCTAssertTrue(GridValue.isOpenable(value: big, type: "map(varchar,bigint)"))
        XCTAssertNil(GridValue.prettyPrinted(big))
    }
}
