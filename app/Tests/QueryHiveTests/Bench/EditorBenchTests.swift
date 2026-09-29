import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// Bench 4-5: the syntax pass on a large document, and one keystroke through the coordinator.
final class EditorBenchTests: BenchCase {
    /// `SQLSyntax` stops colouring past 200,000 UTF-16 units (its `ceiling`), so a 10k-line
    /// document only exercises the scanner when its lines are short: `short` keeps it under that
    /// (about 15 units a line), the long form is the realistic one and lands over it.
    private func document(lines: Int, short: Bool) -> String {
        (0..<lines).map { i in
            if short {
                switch i % 4 {
                case 0: return "SELECT a, b FROM t;"
                case 1: return "-- note \(i % 100)"
                case 2: return "WHERE x > \(i % 999)"
                default: return "AND n = 'ab'"
                }
            }
            switch i % 4 {
            case 0: return "SELECT a\(i), b, count(*) AS n FROM public.t\(i % 50) WHERE x > \(i) AND name = 'foo\(i)'"
            case 1: return "  -- comment number \(i)"
            case 2: return "  GROUP BY 1, 2 ORDER BY n DESC LIMIT \(i % 100 + 1);"
            default: return "INSERT INTO t\(i % 50) (a, b) VALUES (\(i), NULL), (2, 'x'); /* block */"
            }
        }.joined(separator: "\n")
    }

    /// Same construction as `EditorFindAndFoldingTests.editor`.
    private func headlessEditor(_ sql: String) -> SQLTextView {
        let storage = NSTextStorage(string: sql)
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: 800, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let textView = SQLTextView(frame: NSRect(x: 0, y: 0, width: 800, height: 400), textContainer: container)
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        return textView
    }

    func testSyntaxApply10kLines() throws {
        for short in [true, false] {
            let sql = document(lines: 10_000, short: short)
            let textView = headlessEditor(sql)
            let samples = measureMs(iterations: 10, warmup: 1) { SQLSyntax.apply(to: textView) }
            // A pass that painted nothing would be fast for the wrong reason: only the short form
            // is under the ceiling, and it must have coloured something.
            let storage = try XCTUnwrap(textView.textStorage)
            var runs = 0
            storage.enumerateAttribute(.foregroundColor, in: NSRange(location: 0, length: min(4000, storage.length))) { _, _, _ in runs += 1 }
            if short { XCTAssertGreaterThan(runs, 10, "SQLSyntax.apply left no token colours") }
            report(short ? "bench-syntax-apply-10k" : "bench-syntax-apply-10k-over-ceiling", axis: 5, samples: samples,
                   notes: "SQLSyntax.apply, 10,000 lines, \(sql.utf16.count) UTF-16 units; ceiling is 200000 so \(short ? "colouring runs" : "colouring is skipped")")
        }
    }

    func testKeystroke10kLines() {
        for short in [true, false] { keystroke(short: short) }
    }

    private func keystroke(short: Bool) {
        let sql = document(lines: 10_000, short: short)
        let textView = headlessEditor(sql)
        var changes = 0
        let editor = SQLEditor(text: Binding(get: { textView.string }, set: { _ in changes += 1 }),
                               focused: .constant(false),
                               caret: .constant(0),
                               selection: .constant(NSRange(location: 0, length: 0)),
                               completion: EditorCompletion(),
                               candidates: { _, _ in [] },
                               layout: .standard, onRunStatement: nil)
        let coordinator = editor.makeCoordinator()
        coordinator.textView = textView
        textView.delegate = coordinator
        coordinator.recolour()

        let middle = (sql as NSString).length / 2
        textView.setSelectedRange(NSRange(location: middle, length: 0))
        let samples = measureMs(iterations: 50, warmup: 3) {
            textView.insertText("a", replacementRange: textView.selectedRange())
        }
        XCTAssertGreaterThan(changes, 0, "the coordinator's textDidChange never ran")
        report(short ? "bench-keystroke-10k" : "bench-keystroke-10k-over-ceiling", axis: 5, samples: samples,
               notes: "insertText of one character mid-document through SQLTextView + Coordinator.textDidChange, \(sql.utf16.count) UTF-16 units")
    }
}
