import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// The connection sheet's structure (W13-T19, blueprint w13-connection-form §8.2): what a picture
// cannot say. The pictures are `ChromeParityTests`; these tests hold that the sheet's own parts ask
// for no more than its frame gives, that it fits under a window of the shell's minimum height, that
// what is typed survives a change of section, that a host-key prompt attaches from any section, and
// that Test with something missing opens the section that holds it.
//
// The rig is `ShellTests`' (a real window, the run loop spun until a condition holds, a walk of the
// view tree and the `FrameMarker` views that say where SwiftUI put things). Its helpers are private
// to that file, which other work is editing, so the few that are needed are copied here.
//
// xctest runs with the SDK it was built with, so this does not run the sheet the way a binary built
// for macOS 14 does; the gate for that is `./app/build.sh` and the UI probe (blueprint R-1).
@MainActor
final class ConnectionSheetTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        ThemeStore.shared.pin(reduceMotion: true, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: .midnight, accent: .ice, tone: .glow, mode: .dark,
                              systemIsDark: true, uiFont: "", codeFont: "")
    }

    override func tearDown() {
        ConnectionEditorSheet.initialSectionForSnapshot = nil
        ConnectionEditorSheet.newKindForSnapshot = nil
        HostKeyCenter.shared.dismiss()
        for window in windows {
            if let sheet = window.attachedSheet { window.endSheet(sheet) }
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
        ThemeStore.shared.unpinSurface()
        ThemeStore.shared.pin(theme: .midnight, accent: .ice, tone: .glow, mode: .dark,
                              systemIsDark: true, uiFont: "", codeFont: "")
        super.tearDown()
    }

    // MARK: Rig

    private struct Rig {
        let model: AppModel
        let window: NSWindow
        let host: NSHostingView<AnyView>
    }

    /// The shell in a window of `size`, as `ShellTests` draws it. The sheet is opened by the test.
    private func rig(size: CGSize = CGSize(width: Shell.minWidth, height: 700)) -> Rig {
        let model = Snapshot.seeded(scene: "done")
        // A fixture must not ask a server that is not there.
        for connection in model.connections {
            model.catalogOptions[connection.id] = []
            model.schemaOptions["\(connection.id.uuidString)|"] = []
        }
        let host = NSHostingView(rootView: AnyView(RootView().environment(model)
            .frame(width: size.width, height: size.height).preferredColorScheme(.dark)))
        host.sceneBridgingOptions = [.toolbars, .title]
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame,
                              styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        window.orderFrontRegardless()
        windows.append(window)
        return Rig(model: model, window: window, host: host)
    }

    /// A saved PostgreSQL connection that is complete except for what the test blanks.
    private func postgres(_ tweak: (inout Connection) -> Void = { _ in }) -> Connection {
        var c = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .postgres,
                           host: "pg.internal", port: 5432, sslmode: "prefer", user: "analyst",
                           database: "warehouse", schema: "public", verify: true)
        tweak(&c)
        return c
    }

    /// Opens the editor for `connection` over the shell and returns the sheet's window.
    @discardableResult
    private func openEditor(_ connection: Connection?, in rig: Rig, section: ConnectionFormSection? = nil,
                            newKind: ConnectionKind? = nil) throws -> NSWindow {
        ConnectionEditorSheet.initialSectionForSnapshot = section
        ConnectionEditorSheet.newKindForSnapshot = newKind
        if let connection {
            rig.model.connections.append(connection)
            rig.model.rebuildTree()
        }
        rig.model.presentConnectionEditor(connection?.id)
        XCTAssertTrue(wait { rig.window.attachedSheet != nil }, "the connection sheet did not attach")
        let sheet = try XCTUnwrap(rig.window.attachedSheet)
        spin()
        return sheet
    }

    private func spin(_ seconds: Double = 0.4) {
        RunLoop.current.run(until: Date().addingTimeInterval(seconds))
    }

    /// Runs the loop until `condition` holds or `timeout` passes; the answer is whether it held.
    @discardableResult
    private func wait(_ timeout: Double = 5, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        return condition()
    }

    private func descendants<T: NSView>(_ type: T.Type, in view: NSView) -> [T] {
        var found: [T] = []
        if let match = view as? T { found.append(match) }
        for subview in view.subviews { found += descendants(type, in: subview) }
        return found
    }

    private func marker(_ name: String, in window: NSWindow) -> NSView? {
        guard let root = window.contentView else { return nil }
        return descendants(NSView.self, in: root).first { $0.identifier?.rawValue == name }
    }

    /// Where a marker landed, in its window's coordinates (origin at the bottom left).
    private func frame(_ name: String, in window: NSWindow) -> CGRect? {
        marker(name, in: window).map { $0.convert($0.bounds, to: nil) }
    }

    /// A left click at the middle of the named marker.
    private func click(_ name: String, in window: NSWindow, file: StaticString = #filePath, line: UInt = #line) throws {
        let rect = try XCTUnwrap(frame(name, in: window), "no view named \(name)", file: file, line: line)
        let point = NSPoint(x: rect.midX, y: rect.midY)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(
                with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
            window.sendEvent(event)
            spin(0.1)
        }
        spin(0.2)
    }

    /// The text field that sits where the named marker is.
    private func textField(at name: String, in window: NSWindow) -> NSTextField? {
        guard let rect = frame(name, in: window), let root = window.contentView else { return nil }
        return descendants(NSTextField.self, in: root).first { field in
            let box = field.convert(field.bounds, to: nil)
            return box.intersects(rect) && field.isEditable
        }
    }

    private func isShowing(_ section: ConnectionFormSection, in window: NSWindow) -> Bool {
        marker("connection-pane-\(section.rawValue)", in: window) != nil
    }

    // MARK: Parts that do not scroll

    /// What a sheet that is framed to 820 x 620 asks for cannot be read from the sheet: the frame
    /// answers 820 x 620 whatever is inside (blueprint B-3). So the parts that do not scroll are
    /// measured on their own, without the frame, and must leave room for a pane in what is left.
    func testTheSheetNeverAsksForMoreThanItsFrame() {
        func height<V: View>(_ view: V, width: CGFloat) -> CGFloat {
            NSHostingView(rootView: view.frame(width: width)).fittingSize.height
        }
        let header = height(ConnectionFormHeader(title: "A connection with a rather long name indeed", isDirty: true), width: 619)
        let footerStatuses: [ConnectionFormFooter.Status] = [
            .none, .success("Connected · 3 catalogs"),
            .failure("Connection refused: " + String(repeating: "x ", count: 200)),
            .problem("SSH Tunnel: SSH host and SSH user are required. Also see General and Limits."),
        ]
        var footer: CGFloat = 0
        for status in footerStatuses {
            for testing in [false, true] {
                footer = max(footer, height(ConnectionFormFooter(isTesting: testing, status: status, canSave: true,
                                                                  onTest: {}, onCancel: {}, onSave: {}), width: 619))
            }
        }
        XCTAssertEqual(header, 52)
        XCTAssertEqual(footer, 56)
        for kind in ConnectionKind.allCases {
            for section in ConnectionFormSection.allCases {
                var selection = section
                let sidebar = height(ConnectionFormSidebar(kind: kind, isSaved: true,
                                                           selection: Binding(get: { selection }, set: { selection = $0 }),
                                                           issueCounts: [.general: 2, .sshTunnel: 1], onDelete: {}),
                                     width: 200)
                XCTAssertLessThanOrEqual(sidebar, 620, "the sidebar of \(kind) in \(section)")
                // Header, footer and the two hairlines leave the pane at least 400 points.
                XCTAssertLessThanOrEqual(header + footer + 2, 620 - 400, "\(kind) \(section)")
            }
        }
    }

    // MARK: Encryption beside the host

    func testTheEncryptionControlSitsOnTheHostRow() throws {
        let mysql = Connection(id: UUID(), name: "Reporting", color: .amber, kind: .mysql, host: "mysql.internal",
                               port: 3306, sslmode: "disable", user: "u", database: "r", schema: "", verify: true)
        for (index, connection) in [nil, postgres(), mysql].enumerated() {
            let rig = rig()
            let kind = connection?.kind ?? .trino
            let sheet = try openEditor(connection ?? rig.model.connections.first, in: rig)
            XCTAssertTrue(wait { self.frame("connection-encryption", in: sheet) != nil && self.frame("connection-host-field", in: sheet) != nil },
                          "\(kind): no encryption control or no host field")
            let control = try XCTUnwrap(frame("connection-encryption", in: sheet))
            let field = try XCTUnwrap(frame("connection-host-field", in: sheet))
            XCTAssertLessThanOrEqual(abs(control.midY - field.midY), 2, "\(kind) (\(index)): not on the host row")
            XCTAssertGreaterThan(control.minX, field.maxX, "\(kind): not to the right of the host")
            let content = try XCTUnwrap(sheet.contentView).bounds
            XCTAssertTrue(content.contains(control) && content.contains(field), "\(kind): outside the sheet")
            for window in windows { if let attached = window.attachedSheet { window.endSheet(attached) } }
            spin(0.3)
        }
    }

    // MARK: Badges and the footer

    func testTheGeneralBadgeFollowsTheMissingFields() throws {
        let rig = rig()
        let sheet = try openEditor(nil, in: rig, newKind: .postgres)
        // A new PostgreSQL connection lacks a name, a host, a user and a database, all in General.
        XCTAssertTrue(wait { self.marker("connection-badge-general", in: sheet) != nil }, "General has no badge")
        XCTAssertNil(marker("connection-badge-sshTunnel", in: sheet), "a section with nothing missing has none")
        XCTAssertNil(marker("connection-badge-limits", in: sheet))
        XCTAssertNotNil(marker("connection-footer-problem", in: sheet), "the footer does not say what is missing")
        // What the badge says aloud, and what the footer says: pure, so they are held directly.
        XCTAssertEqual(ConnectionFormSidebar.attentionValue(4), "4 fields need attention")
        XCTAssertEqual(ConnectionFormSidebar.attentionValue(1), "1 field needs attention")
        XCTAssertEqual(ConnectionFormSidebar.attentionValue(0), "")
        var postgres = ConnectionDraft.new()
        postgres.choose(.postgres)
        let missing = ConnectionFormIssues.fields(for: postgres.formState, resolved: nil)
        XCTAssertEqual(ConnectionFormIssues.summary(missing, visible: .general), "Name, Host, User and Database are required.")
    }

    func testASavedCompleteConnectionHasNoBadgeAndNoSummary() throws {
        let rig = rig()
        let sheet = try openEditor(postgres(), in: rig)
        XCTAssertTrue(wait { self.frame("connection-footer", in: sheet) != nil })
        for section in ConnectionFormSection.allCases {
            XCTAssertNil(marker("connection-badge-\(section.rawValue)", in: sheet), section.title)
        }
        XCTAssertNil(marker("connection-footer-problem", in: sheet))
    }

    // MARK: The Test result

    func testATestResultHandedInSurvivesTheLoadAndGoesWithTheFirstEdit() throws {
        let rig = rig()
        let connection = try XCTUnwrap(rig.model.connections.first)
        ConnectionEditorSheet.initialSectionForSnapshot = nil
        rig.model.presentConnectionEditor(connection.id, previewTestCount: 56)
        XCTAssertTrue(wait { rig.window.attachedSheet != nil })
        let sheet = try XCTUnwrap(rig.window.attachedSheet)
        // `load()` runs as the sheet appears and must not take the result with it.
        XCTAssertTrue(wait { self.marker("connection-footer-result", in: sheet) != nil },
                      "the result the scene hands in was lost when the sheet loaded")
        spin(0.6)
        XCTAssertNotNil(marker("connection-footer-result", in: sheet), "and it must still be there a moment later")
        // An edit by the person ends it: it described the form as it was.
        XCTAssertTrue(wait { self.textField(at: "connection-host-field", in: sheet) != nil })
        let host = try XCTUnwrap(textField(at: "connection-host-field", in: sheet))
        XCTAssertTrue(sheet.makeFirstResponder(host))
        host.currentEditor()?.insertText("x")
        XCTAssertTrue(wait { self.marker("connection-footer-result", in: sheet) == nil }, "the result outlived an edit")
    }

    // MARK: Over the shell

    func testTheSheetFitsAWindowAtTheShellMinimum() throws {
        let rig = rig()
        let sheet = try openEditor(rig.model.connections.first, in: rig)
        // A presented sheet's own minimum, which is where a layout minimum that is taller than the
        // frame would show. (This runs on the SDK xctest was built with; the build for macOS 14 is
        // checked by the UI probe.)
        XCTAssertLessThanOrEqual(sheet.contentMinSize.height, 620)
        XCTAssertLessThanOrEqual(sheet.frame.height, rig.window.frame.height, "the sheet is taller than the window it hangs from")
        let footer = try XCTUnwrap(frame("connection-footer", in: sheet), "no footer")
        let content = try XCTUnwrap(sheet.contentView).bounds
        XCTAssertTrue(content.contains(footer), "the footer is not whole inside the sheet: \(footer) in \(content)")
        XCTAssertGreaterThan(footer.height, 40)
    }

    func testTypedValuesSurviveASectionSwitch() throws {
        let rig = rig()
        let sheet = try openEditor(nil, in: rig, newKind: .postgres)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) }, "the form does not open on General")
        XCTAssertTrue(wait { self.textField(at: "connection-host-field", in: sheet) != nil })
        let host = try XCTUnwrap(textField(at: "connection-host-field", in: sheet))
        XCTAssertTrue(sheet.makeFirstResponder(host))
        host.currentEditor()?.insertText("db.example.org")
        spin()
        XCTAssertEqual(host.stringValue, "db.example.org")

        try click("connection-section-sshTunnel", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.sshTunnel, in: sheet) }, "the sidebar did not switch to SSH Tunnel")
        try click("connection-section-general", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        XCTAssertTrue(wait { self.textField(at: "connection-host-field", in: sheet) != nil })
        XCTAssertEqual(textField(at: "connection-host-field", in: sheet)?.stringValue, "db.example.org",
                       "what was typed in Host did not survive a trip to another section")
    }

    func testAHostKeyPromptAttachesFromAnySection() throws {
        let rig = rig()
        let sheet = try openEditor(rig.model.connections.first, in: rig, section: .limits)
        XCTAssertTrue(wait { self.isShowing(.limits, in: sheet) })
        XCTAssertNil(sheet.attachedSheet)
        HostKeyCenter.shared.report(
            Event.HostKeyDetail(state: "unknown", host: "bastion.corp", port: 22, alias: nil, keyType: "ssh-ed25519",
                                fingerprint: "SHA256:uNiQuEfInGeRpRiNtFoRtHeSnApShOtSsCeNeAbCdE",
                                appKnownHosts: "known_hosts", caCovered: false, recorded: nil, pinned: nil),
            env: [:])
        XCTAssertTrue(wait { sheet.attachedSheet != nil }, "the host-key sheet did not attach to the connection sheet")
    }

    func testTestWithSomethingMissingOpensTheSectionThatHoldsIt() throws {
        let rig = rig()
        // Complete in General; the tunnel is on and its user is missing.
        let broken = postgres { $0.sshHost = "bastion.corp"; $0.sshUser = "" }
        let sheet = try openEditor(broken, in: rig)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        Announcer.reset()
        // ⌘↩ is Test Connection, and it runs the same check Save does before it touches anything.
        let key = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.command],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: sheet.windowNumber, context: nil,
            characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36))
        XCTAssertTrue(sheet.performKeyEquivalent(with: key), "⌘↩ was not taken by the sheet")
        XCTAssertTrue(wait { self.isShowing(.sshTunnel, in: sheet) }, "the sheet stayed on General")
        XCTAssertEqual(Announcer.last, "SSH user is required.", "the summary is announced from the section it opened")
    }
}
