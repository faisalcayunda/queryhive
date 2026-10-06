import Foundation
import XCTest

/// Nothing a person must read is smaller than 11 pt (W9-T9, blueprint w9 §12). A scan of the
/// sources for `.ui(` and `.code(` literals under 11.
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
        "Workspace.swift": (1, "W10-T7 (the editor's line count, with the gutter numbers; V-10)"),
    ]

    private static let sources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources/QueryHive")

    /// Every `.ui(` / `.code(` whose first argument is a number below 11: (file, line, size, text).
    private func lowCalls() throws -> [(file: String, line: Int, size: Double, text: String)] {
        let pattern = try NSRegularExpression(pattern: #"\.(?:ui|code)\(\s*([0-9]+(?:\.[0-9]+)?)"#)
        let files = try XCTUnwrap(FileManager.default.enumerator(at: Self.sources,
                                                                 includingPropertiesForKeys: nil))
        var found: [(String, Int, Double, String)] = []
        for case let url as URL in files where url.pathExtension == "swift" {
            let lines = try String(contentsOf: url, encoding: .utf8).components(separatedBy: "\n")
            for (index, text) in lines.enumerated() {
                let range = NSRange(text.startIndex..., in: text)
                for match in pattern.matches(in: text, range: range) {
                    guard let number = Range(match.range(at: 1), in: text),
                          let size = Double(text[number]), size < 11 else { continue }
                    found.append((url.lastPathComponent, index + 1, size, text))
                }
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
}
