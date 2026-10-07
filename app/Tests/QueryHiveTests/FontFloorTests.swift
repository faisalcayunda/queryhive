import Foundation
import XCTest

/// Nothing a person must read is smaller than 11 pt (W9-T9, blueprint w9 §12). A scan of the
/// sources for `.ui(` and `.code(` literals under 11, and, since W10-T2, for the AppKit fonts the
/// grid and the editor draw their numbers in (`codeNSFont(size:` and `ofSize:`), which SwiftUI's
/// `.font` scan could not see. The grid's gutter is 11 and is off the list.
///
/// A line below 11 passes only when it is a decorative label (`DecorativeLabel`, or a
/// `// decorative-label` comment) at 10 or more, or when its file is on `pending` below, which counts
/// the lines still owed and names the task that closes them. SF Symbol glyphs (`.system(size:)`) are
/// out of scope (§12.1 item 3) and so is text AppKit draws (§12.1 item 4).
final class FontFloorTests: XCTestCase {
    /// File name to (lines under 11 still owed, the task that raises them). The list shrinks to empty
    /// at the W10 gate; a count that falls fails here until the entry is lowered.
    private static let pending: [String: (lines: Int, task: String)] = [
        "SidebarTree.swift": (2, "W9-T3 (QUERYHIVE and FAVOURITES become DecorativeLabel)"),
        "ResultGrid.swift": (12, "W10-T3 (V-9)"),
    ]

    private static let sources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources/QueryHive")

    private static let literal = try! NSRegularExpression(
        pattern: #"(?:\.(?:ui|code)\(|codeNSFont\(size:|ofSize:)\s*([0-9]+(?:\.[0-9]+)?)"#)

    /// The sizes under 11 that a line of source writes as a literal in a font call.
    static func lowSizes(in text: String) -> [Double] {
        let range = NSRange(text.startIndex..., in: text)
        return literal.matches(in: text, range: range).compactMap { match in
            Range(match.range(at: 1), in: text).flatMap { Double(text[$0]) }.flatMap { $0 < 11 ? $0 : nil }
        }
    }

    /// Every `.ui(` / `.code(` whose first argument is a number below 11, and every AppKit
    /// `codeNSFont(size:` / `ofSize:` literal below 11: (file, line, size, text).
    private func lowCalls() throws -> [(file: String, line: Int, size: Double, text: String)] {
        let files = try XCTUnwrap(FileManager.default.enumerator(at: Self.sources,
                                                                 includingPropertiesForKeys: nil))
        var found: [(String, Int, Double, String)] = []
        for case let url as URL in files where url.pathExtension == "swift" {
            let lines = try String(contentsOf: url, encoding: .utf8).components(separatedBy: "\n")
            for (index, text) in lines.enumerated() {
                for size in Self.lowSizes(in: text) { found.append((url.lastPathComponent, index + 1, size, text)) }
            }
        }
        return found
    }

    func testNoTextIsBelowElevenPointsExceptDecorativeLabelsAndTheListedPending() throws {
        let calls = try lowCalls()
        var owed: [String: Int] = [:]
        for call in calls {
            let decorative = call.text.contains("DecorativeLabel") || call.text.contains("// decorative-label")
            if decorative {
                XCTAssertGreaterThanOrEqual(call.size, 10,
                                            "\(call.file):\(call.line): a decorative label is at least 10 pt")
            } else if Self.pending[call.file] != nil {
                owed[call.file, default: 0] += 1
            } else {
                XCTFail("\(call.file):\(call.line): \(call.size) pt is under the 11 pt floor: "
                        + call.text.trimmingCharacters(in: .whitespaces))
            }
        }
        for (file, entry) in Self.pending {
            XCTAssertEqual(owed[file, default: 0], entry.lines,
                           "\(file) is owed to \(entry.task): lower or remove its entry in `pending`")
        }
    }

    func testTheScanSeesTheCalls() throws {
        XCTAssertGreaterThan(try lowCalls().count, 10, "the scan found the owed lines")
    }

    /// The scan reaches the AppKit fonts, which SwiftUI's `.font` scan could not see.
    func testTheScanReadsAppKitFontLiterals() {
        XCTAssertEqual(Self.lowSizes(in: "numberFont = FontChoice.codeNSFont(size: 10.5, weight: .regular)"), [10.5])
        XCTAssertEqual(Self.lowSizes(in: ".monospacedSystemFont(ofSize: 10, weight: .regular)"), [10])
        XCTAssertEqual(Self.lowSizes(in: ".font(.code(9, weight: .semibold))"), [9])
        XCTAssertEqual(Self.lowSizes(in: ".code(11) .ui(12.5) ofSize: 13"), [])
    }

    /// Nothing the grid or the editor draws is under 11: the grid's gutter is 11, and so are the
    /// editor's line numbers and the count in its corner (W10-T7b, D-17).
    func testTheGridAndTheEditorAreClean() throws {
        let calls = try lowCalls()
        for file in ["GridRowView.swift", "GridHeaderView.swift", "ResultGridTable.swift", "GridMetrics.swift",
                     "GridTableView.swift", "SQLEditor.swift", "Workspace.swift"] {
            XCTAssertTrue(calls.filter { $0.file == file }.isEmpty, "\(file) has no text under 11 pt")
        }
    }
}
