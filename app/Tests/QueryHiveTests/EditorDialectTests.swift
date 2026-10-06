import AppKit
import QueryHiveFFI
import SwiftUI
import XCTest

@testable import QueryHive

/// Blueprint w10 §8.6: the editor reads a tab's SQL under its connection's dialect, so the statement
/// the editor draws is the statement Run sends (L-18, and the dialect part of L-12).
@MainActor
final class EditorDialectTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    /// `'\''` is one string in MySQL, so the `DELETE` after it is code; generic reads `''` as a
    /// doubled quote and never sees a separator.
    private let backslashScript = "SELECT '\\''; DELETE FROM t; -- '"

    func testEachKindMapsToItsDialect() {
        XCTAssertEqual(ConnectionKind.trino.editorDialect, .trino)
        XCTAssertEqual(ConnectionKind.postgres.editorDialect, .postgres)
        XCTAssertEqual(ConnectionKind.mysql.editorDialect, .mysql)
    }

    func testStatementsAreCutUnderTheGivenDialect() {
        XCTAssertEqual(sqlStatements(in: backslashScript, dialect: .mysql).map(\.text),
                       ["SELECT '\\''", "DELETE FROM t"])
        XCTAssertEqual(sqlStatements(in: backslashScript, dialect: .generic).count, 1)
        XCTAssertEqual(sqlStatements(in: backslashScript).count, 1, "the default stays generic")
        XCTAssertEqual(sqlStatement(in: backslashScript, atUTF16Offset: 20, dialect: .mysql), "DELETE FROM t")
        XCTAssertEqual(try editorStatementRanges(backslashScript, dialect: .mysql).count, 2)
    }

    func testTheEditorsAnalysisIsBuiltUnderTheTabsDialect() throws {
        let generic = EditorRig(backslashScript)
        try generic.settle()
        XCTAssertEqual(generic.coordinator.rotor?.items.count, 1)
        let mysql = EditorRig(backslashScript, dialect: .mysql)
        try mysql.settle()
        XCTAssertEqual(mysql.coordinator.rotor?.items.count, 2)
    }

    func testAChangedDialectBuildsTheAnalysisAgain() throws {
        let rig = EditorRig(backslashScript)
        try rig.settle()
        XCTAssertEqual(rig.coordinator.rotor?.items.count, 1)
        rig.switchDialect(to: .mysql)
        try rig.settle()
        XCTAssertEqual(rig.coordinator.rotor?.items.count, 2, "the old analysis still read it as generic")
    }

    func testTheSameDialectKeepsTheAnalysis() throws {
        let rig = EditorRig("select 1")
        try rig.settle()
        rig.view.setSelectedRange(NSRange(location: 8, length: 0))
        rig.view.insertText(" ", replacementRange: NSRange(location: NSNotFound, length: 0))
        let before = rig.coordinator.pendingRevisionForTesting
        XCTAssertGreaterThan(before, 1)
        rig.switchDialect(to: .generic)
        XCTAssertEqual(rig.coordinator.pendingRevisionForTesting, before, "a rebuild would restart the revision")
        rig.switchDialect(to: .postgres)
        XCTAssertEqual(rig.coordinator.pendingRevisionForTesting, 1)
    }

    func testTheEditorPaneHandsItsTabTheConnectionsDialect() throws {
        let model = AppModel()
        let connection = Connection(id: UUID(), name: "My", color: .violet, kind: .mysql, host: "127.0.0.1",
                                    port: 3306, sslmode: "prefer", user: "qh", database: "qh", schema: "",
                                    verify: false)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = backslashScript
        model.tabs = [tab]
        model.selectedTabID = tab.id
        XCTAssertEqual(tab.dialect, .generic)
        let host = NSHostingView(rootView: AnyView(EditorPane(tab: tab).environment(model)
            .frame(width: 600, height: 240)))
        host.frame = CGRect(x: 0, y: 0, width: 600, height: 240)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        let deadline = Date().addingTimeInterval(5)
        while tab.dialect != .mysql, Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            host.layoutSubtreeIfNeeded()
        }
        XCTAssertEqual(tab.dialect, .mysql)
        // The tab now cuts statements the way the editor draws them.
        tab.selection = NSRange(location: 20, length: 0)
        XCTAssertEqual(tab.sql(for: .statement), "DELETE FROM t")
        // Another connection, another dialect.
        let other = Connection(id: UUID(), name: "Pg", color: .violet, kind: .postgres, host: "127.0.0.1",
                               port: 5432, sslmode: "prefer", user: "qh", database: "qh", schema: "public",
                               verify: false)
        model.connections.append(other)
        tab.connectionID = other.id
        let again = Date().addingTimeInterval(5)
        while tab.dialect != .postgres, Date() < again {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            host.layoutSubtreeIfNeeded()
        }
        XCTAssertEqual(tab.dialect, .postgres)
    }
}
