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
            let analysis = try EditorAnalysis(text: sql)
            let full = NSRange(location: 0, length: analysis.length)
            let colors = Self.paintColors()
            let (regular, italic) = Self.fonts()
            // A pass that painted nothing would be fast for the wrong reason: the document is
            // under the ceiling, and the analysis must have coloured something.
            let first = try analysis.paint(window: full, budget: analysis.length)
            XCTAssertGreaterThan(first.runs.count, 10, "the analysis left no token colours")
            let samples = measureMs(iterations: 10, warmup: 1) {
                try? analysis.markDirty(full)
                if let paint = try? analysis.paint(window: full, budget: analysis.length) {
                    analysis.apply(paint, to: textView, colors: colors,
                                   regularFont: regular, italicFont: italic)
                }
            }
            report(short ? "bench-syntax-apply-10k-short" : "bench-syntax-apply-10k-long", axis: 5, samples: samples,
                   notes: "EditorAnalysis.paint plus temporary-attribute apply, 10,000 lines, \(sql.utf16.count) UTF-16 units")
        }
    }

    /// The palette instances the apply reads, built the way the coordinator builds them.
    private static func paintColors() -> [EditorColorClass: NSColor] {
        [.comment: SQLSyntax.colour(for: .comment),
         .string: SQLSyntax.colour(for: .string),
         .quotedIdentifier: SQLSyntax.colour(for: .quotedIdentifier),
         .number: SQLSyntax.colour(for: .number),
         .keyword: SQLSyntax.colour(for: .keyword),
         .literal: SQLSyntax.colour(for: .literal),
         .function: SQLSyntax.colour(for: .function),
         .punctuation: SQLSyntax.colour(for: .punctuation),
         .parameter: SQLSyntax.colour(for: .parameter)]
    }

    private static func fonts() -> (NSFont, NSFont) {
        (SQLSyntax.font(italic: false), SQLSyntax.font(italic: true))
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
        textView.textStorage?.delegate = coordinator
        coordinator.recolour()

        let middle = (sql as NSString).length / 2
        textView.setSelectedRange(NSRange(location: middle, length: 0))
        let samples = measureMs(iterations: 50, warmup: 3) {
            textView.insertText("a", replacementRange: textView.selectedRange())
            // One full turn: the paint for this keystroke has landed before the next starts.
            let wanted = coordinator.pendingRevisionForTesting
            let deadline = Date().addingTimeInterval(10)
            while coordinator.appliedRevision != wanted, Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.005))
            }
        }
        XCTAssertEqual(coordinator.appliedRevision, coordinator.pendingRevisionForTesting,
                       "the last keystroke's paint never landed")
        coordinator.flushToModel()
        XCTAssertGreaterThan(changes, 0, "typing never reached the model")
        report(short ? "bench-keystroke-10k" : "bench-keystroke-10k-long", axis: 5, samples: samples,
               notes: "insertText of one character mid-document plus its analysis repaint, \(sql.utf16.count) UTF-16 units")
    }
}
