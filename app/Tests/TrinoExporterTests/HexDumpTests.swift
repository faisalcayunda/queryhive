import XCTest

@testable import QueryHive

/// The hex dump behind the viewer's Hex mode.
///
/// The engine renders bytes as lowercase hex with no prefix, so decoding that is the common case
/// rather than a heuristic; the tests pin both the decode and the two shapes that must stay honest
/// — a non-hex value falls back to UTF-8 instead of inventing bytes, and a dump past the cap says
/// where it stopped.
final class HexDumpTests: XCTestCase {
    func testHexIsDecodedForABinaryCell() {
        XCTAssertEqual([UInt8](HexDump.bytes(from: "00ff10")), [0x00, 0xFF, 0x10])
        XCTAssertEqual([UInt8](HexDump.bytes(from: "\\x00ff10")), [0x00, 0xFF, 0x10])
        XCTAssertEqual([UInt8](HexDump.bytes(from: "0x00ff10")), [0x00, 0xFF, 0x10])
        XCTAssertEqual([UInt8](HexDump.bytes(from: "")), [])
    }

    func testANonHexValueFallsBackToUTF8() {
        // "hello" contains an `e`, and that must not be read as a nibble: odd and non-hex input
        // comes back as the text's own bytes, which is what a driver that decoded already sends.
        XCTAssertEqual([UInt8](HexDump.bytes(from: "hello")), Array("hello".utf8))
        XCTAssertEqual([UInt8](HexDump.bytes(from: "abc")), Array("abc".utf8))
    }

    func testTheDumpNamesTheOffsetAndShowsASCII() {
        let dump = HexDump.dump(Data([0x00, 0x41, 0xFF]))
        XCTAssertTrue(dump.hasPrefix("00000000  00 41 ff"), dump)
        XCTAssertTrue(dump.contains("|.A."), dump)
        XCTAssertEqual(HexDump.dump(Data()), "(no bytes)")
    }

    func testADumpPastTheLimitSaysHowManyBytesWereLeftOut() {
        let data = Data(repeating: 0xAB, count: HexDump.limit + 5)
        let dump = HexDump.dump(data)
        XCTAssertTrue(dump.contains("5 more bytes"), dump)
        XCTAssertLessThan(dump.components(separatedBy: "\n").count, HexDump.limit / 16 + 3)
    }

    func testTheRowsAreSixteenBytes() {
        let data = Data(repeating: 0x41, count: 32)
        let lines = HexDump.dump(data).components(separatedBy: "\n")
        XCTAssertEqual(lines.count, 2)
        XCTAssertTrue(lines[1].hasPrefix("00000010"))
    }
}
