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
        "Workspace.swift": (1, "W10-T7 (the editor's line count, with the gutter numbers; V-10)"),
        // The editor's gutter numbers: `numberFont`'s two 10.5 pt (the default and the setter's
        // initial value). They move with the 16 `editor-*` scenes, which is V-10, not V-9.
        "SQLEditor.swift": (2, "W10-T7b (the gutter numbers, 11 pt; V-10)"),
    ]

    private static let sources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources/QueryHive")

    /// Every `.ui(` / `.code(` whose first argument is a number below 11, and every AppKit
    /// `codeNSFont(size:` / `ofSize:` literal below 11: (file, line, size, text).
    private func lowCalls() throws -> [(file: String, line: Int, size: Double, text: String)] {
        let pattern = try NSRegularExpression(
            pattern: #"(?:\.(?:ui|code)\(|codeNSFont\(size:|ofSize:)\s*([0-9]+(?:\.[0-9]+)?)"#)
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
        // The list of owed lines has shrunk to the few that remain (the editor's gutter, the sidebar's
        // labels); what this holds is that the scan still finds some, so it cannot pass by seeing none.
        XCTAssertGreaterThan(try lowCalls().count, 0, "the scan found the owed lines")
    }

    /// The scan reaches the AppKit fonts: the editor's gutter numbers are in it (they are owed to
    /// W10-T7b), and the grid's files, whose gutter is 11, are not.
    func testTheScanSeesTheAppKitFontsAndTheGridIsClean() throws {
        let calls = try lowCalls()
        XCTAssertEqual(calls.filter { $0.file == "SQLEditor.swift" }.count, 2)
        for file in ["GridRowView.swift", "GridHeaderView.swift", "ResultGridTable.swift", "GridMetrics.swift",
                     "GridTableView.swift", "ResultGrid.swift"] {
            XCTAssertTrue(calls.filter { $0.file == file }.isEmpty, "\(file) has no text under 11 pt")
        }
    }
}
