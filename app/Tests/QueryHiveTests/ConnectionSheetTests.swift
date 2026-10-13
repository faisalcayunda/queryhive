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
        if let connection, !rig.model.connections.contains(where: { $0.id == connection.id }) {
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
    /// measured on their own, without that frame. What this really holds: the sidebar's own natural
    /// height (it has no fixed height, so this is a measurement) fits the sheet for every driver and
    /// section, and the header and footer are the fixed 52 and 56 they are designed to be (they
    /// carry their own height, so for them this pins a constant, not a layout) and leave a pane at
    /// least 400 points. It cannot say whether the scrolling pane or the sidebar would still be
    /// allowed to shrink without `.frame(minHeight: 0, maxHeight: .infinity)`: under xctest the
    /// sheet's outer frame hides that, and no seam shows it. That is the gate of the SDK 14 build
    /// and the UI probe (blueprint R-1), not this test.
    func testTheSidebarFitsAndTheHeaderAndFooterAreTheFixedHeightsTheySayTheyAre() {
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

    /// The count a section's badge carries in the picture (it is in the marker's name), or `nil`
    /// when the section has no badge.
    private func badgeCount(_ section: ConnectionFormSection, in window: NSWindow) -> Int? {
        guard let root = window.contentView else { return nil }
        let prefix = "connection-badge-\(section.rawValue)-"
        return descendants(NSView.self, in: root)
            .compactMap { $0.identifier?.rawValue }
            .first { $0.hasPrefix(prefix) }
            .flatMap { Int($0.dropFirst(prefix.count)) }
    }

    func testTheGeneralBadgeAndTheFooterFollowTheMissingFields() throws {
        let rig = rig()
        let sheet = try openEditor(nil, in: rig, newKind: .postgres)
        // A new PostgreSQL connection lacks a name, a host, a user and a database, all in General.
        XCTAssertTrue(wait { self.badgeCount(.general, in: sheet) != nil }, "General has no badge")
        XCTAssertEqual(badgeCount(.general, in: sheet), 4, "the badge in the picture counts the four missing fields")
        XCTAssertNil(badgeCount(.sshTunnel, in: sheet), "a section with nothing missing has none")
        XCTAssertNil(badgeCount(.limits, in: sheet))
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
            XCTAssertNil(badgeCount(section, in: sheet), section.title)
        }
        XCTAssertNil(marker("connection-footer-problem", in: sheet))
    }

    func testChoosingASectionInTheSidebarAnnouncesItsName() throws {
        let rig = rig()
        let sheet = try openEditor(postgres(), in: rig)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        Announcer.reset()
        try click("connection-section-limits", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.limits, in: sheet) })
        XCTAssertEqual(Announcer.last, "Limits")
        Announcer.reset()
        try click("connection-section-limits", in: sheet)
        XCTAssertNil(Announcer.last, "choosing the section already showing says nothing")
    }

    /// The required fields and the login are the first two cards of General, and they fit above the
    /// footer without scrolling for every driver: the scroller hides itself, so nothing would say
    /// that a required field was below the fold.
    func testTheLoginIsReachableWithoutScrollingInGeneralForEveryDriver() throws {
        let mysql = Connection(id: UUID(), name: "Reporting", color: .amber, kind: .mysql, host: "mysql.internal",
                               port: 3306, sslmode: "disable", user: "u", database: "r", schema: "", verify: true)
        for connection in [nil, postgres(), mysql] {
            let rig = rig()
            let kind = connection?.kind ?? .trino
            let sheet = try openEditor(connection ?? rig.model.connections.first, in: rig)
            XCTAssertTrue(wait { self.frame("connection-row-Password", in: sheet) != nil }, "\(kind): no Password row")
            let password = try XCTUnwrap(frame("connection-row-Password", in: sheet))
            let user = try XCTUnwrap(frame("connection-row-User", in: sheet))
            let footer = try XCTUnwrap(frame("connection-footer", in: sheet))
            // Window coordinates run upward: a row above the footer has its bottom above the footer's top.
            XCTAssertGreaterThanOrEqual(password.minY, footer.maxY, "\(kind): Password is below the fold")
            XCTAssertGreaterThanOrEqual(user.minY, footer.maxY, "\(kind): User is below the fold")
            for window in windows { if let attached = window.attachedSheet { window.endSheet(attached) } }
            spin(0.3)
        }
    }

    // MARK: Focus

    /// The text field whose field editor has the focus, if a field has it.
    private func focusedField(in window: NSWindow) -> NSTextField? {
        (window.firstResponder as? NSTextView)?.delegate as? NSTextField
    }

    func testOpeningASavedConnectionLeavesNoFieldFocusedOrSelected() throws {
        let rig = rig()
        let sheet = try openEditor(postgres(), in: rig)
        XCTAssertTrue(wait { self.frame("connection-footer", in: sheet) != nil })
        spin(1.0)
        XCTAssertNil(focusedField(in: sheet), "a field has the focus, and its text is selected for the first keystroke to replace")
        XCTAssertFalse(sheet.firstResponder is NSText)
    }

    func testANewConnectionOpensWithNameFocusedAndTheCaretAtTheEnd() throws {
        let rig = rig()
        let sheet = try openEditor(nil, in: rig)
        XCTAssertTrue(wait { self.frame("connection-type-postgres", in: sheet) != nil }, "no type picker")
        try click("connection-type-postgres", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        XCTAssertTrue(wait { self.focusedField(in: sheet) != nil }, "Name did not take the focus")
        let field = try XCTUnwrap(focusedField(in: sheet))
        XCTAssertTrue(field.isEditable)
        let editor = try XCTUnwrap(sheet.firstResponder as? NSTextView)
        XCTAssertEqual(editor.selectedRange, NSRange(location: (editor.string as NSString).length, length: 0),
                       "the caret is not at the end")
        editor.insertText("Warehouse")
        XCTAssertTrue(wait { self.textFields(in: sheet).contains { $0.stringValue == "Warehouse" } },
                      "what was typed went somewhere other than the first field")
    }

    private func textFields(in window: NSWindow) -> [NSTextField] {
        guard let root = window.contentView else { return [] }
        return descendants(NSTextField.self, in: root)
    }

    // MARK: Captions behind a "?"

    /// The key a person presses to leave: Esc.
    private func escapeEvent(in window: NSWindow) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
            isARepeat: false, keyCode: 53))
    }

    /// Clicks the "?" in the named row and returns the popover window it opens.
    private func openHint(inRow row: String, in sheet: NSWindow, file: StaticString = #filePath, line: UInt = #line) throws -> NSWindow {
        XCTAssertTrue(wait { self.frame("connection-row-\(row)", in: sheet) != nil }, "no \(row) row", file: file, line: line)
        let rowFrame = try XCTUnwrap(frame("connection-row-\(row)", in: sheet), file: file, line: line)
        let root = try XCTUnwrap(sheet.contentView, file: file, line: line)
        let hint = try XCTUnwrap(descendants(NSView.self, in: root).first {
            $0.identifier?.rawValue == "connection-hint" && rowFrame.contains($0.convert($0.bounds, to: nil))
        }, "no \"?\" in the \(row) row", file: file, line: line)
        let before = Set(popoverWindows().map(ObjectIdentifier.init))
        let target = hint.convert(hint.bounds, to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(
                with: type, location: NSPoint(x: target.midX, y: target.midY), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: sheet.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: 1), file: file, line: line)
            sheet.sendEvent(event)
            spin(0.1)
        }
        XCTAssertTrue(wait { popoverWindows().contains { !before.contains(ObjectIdentifier($0)) } },
                      "clicking the \"?\" in the \(row) row did not open its hint", file: file, line: line)
        spin(0.3)
        return try XCTUnwrap(popoverWindows().first { !before.contains(ObjectIdentifier($0)) }, file: file, line: line)
    }

    /// The popover is as big as its sentence and no bigger: a short caption is a small popover, a long
    /// one wraps at the maximum width and is as tall as the wrapped text needs, with no empty box.
    func testAHintPopoverFitsItsSentence() throws {
        func wrappedHeight(_ text: String, width: CGFloat) -> CGFloat {
            let font = NSFont.systemFont(ofSize: 11.5)
            return NSAttributedString(string: text, attributes: [.font: font])
                .boundingRect(with: CGSize(width: width, height: 10_000), options: [.usesLineFragmentOrigin]).height
        }
        let rig = rig()
        let trino = try XCTUnwrap(rig.model.connections.first)
        let sheet = try openEditor(trino, in: rig, section: .appearance)
        // Short: Appearance > Environment.
        XCTAssertTrue(wait { self.isShowing(.appearance, in: sheet) })
        let short = try openHint(inRow: "Environment", in: sheet).frame.size
        NSApp.sendEvent(try escapeEvent(in: sheet))
        XCTAssertTrue(wait { self.popoverWindows().isEmpty })
        // Medium: SSH Tunnel > Tunnel.
        try click("connection-section-sshTunnel", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.sshTunnel, in: sheet) })
        let medium = try openHint(inRow: "Tunnel", in: sheet).frame.size
        NSApp.sendEvent(try escapeEvent(in: sheet))
        XCTAssertTrue(wait { self.popoverWindows().isEmpty })
        // Long: General > Password, the longest caption in the sheet (a Trino connection's).
        try click("connection-section-general", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        let long = try openHint(inRow: "Password", in: sheet).frame.size
        NSApp.sendEvent(try escapeEvent(in: sheet))
        XCTAssertTrue(wait { self.popoverWindows().isEmpty })

        // Every caption in the sheet is longer than a line, so the width is the maximum for all; what
        // grows is the height, with the sentence. (A sentence that fits on one line, and the width
        // that follows it, is `HelpHintPopoverTests`.)
        XCTAssertLessThanOrEqual(short.height, medium.height, "short \(short), medium \(medium)")
        XCTAssertLessThan(medium.height, long.height, "medium \(medium), long \(long)")
        // The tallest stays small, and the widest stays at the maximum (plus the padding and the arrow).
        XCTAssertLessThan(long.height, 200, "the longest popover is \(long)")
        XCTAssertLessThanOrEqual(long.width, HelpHint.popoverWidth + 2 * HelpHint.popoverPadding + 30)
        // No taller than the wrapped text plus the padding (the slack is the popover's arrow).
        let passwordHint = "Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https. "
            + "Prefer is the exception: it starts on HTTPS and falls back to clear only when the coordinator has no TLS."
        let wrapped = wrappedHeight(passwordHint, width: HelpHint.popoverWidth)
        XCTAssertLessThanOrEqual(long.height, wrapped + 2 * HelpHint.popoverPadding + 30, "empty space around the text: \(long) for \(wrapped)")
        XCTAssertGreaterThan(long.height, wrapped * 0.6, "implausibly short for the text: \(long) for \(wrapped)")
    }

    func testEscapeClosesAnOpenHintAndNotTheSheet() throws {
        let rig = rig()
        let sheet = try openEditor(rig.model.connections.first, in: rig)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        _ = try openHint(inRow: "Password", in: sheet)
        NSApp.sendEvent(try escapeEvent(in: sheet))
        XCTAssertTrue(wait { self.popoverWindows().isEmpty }, "Esc did not close the hint")
        spin(0.5)
        XCTAssertNotNil(rig.window.attachedSheet, "Esc closed the whole sheet, and with it what was typed")
    }

    func testTheFooterSummaryHasTheRoomBetweenTheButtons() throws {
        let rig = rig()
        let sheet = try openEditor(nil, in: rig, section: .limits, newKind: .postgres)
        XCTAssertTrue(wait { self.frame("connection-footer-problem", in: sheet) != nil })
        let problem = try XCTUnwrap(frame("connection-footer-problem", in: sheet))
        // The summary is two lines at most, whichever pane is in view: the room between the footer's
        // buttons (the content column is 619 points wide) holds about 70 characters on two lines.
        XCTAssertLessThanOrEqual(problem.height, 34, "more than two lines: \(problem.height) pt")
        let summary = ConnectionFormIssues.summary(ConnectionFormIssues.fields(for: {
            var d = ConnectionDraft.new(); d.choose(.postgres); return d.formState }(), resolved: nil), visible: .limits)
        XCTAssertEqual(summary, "Name, Host, User and Database are required.", "and it is one short sentence")
    }

    func testTheFooterButtonsStayWhereTheyAreWhetherOrNotThereIsAStatus() throws {
        var cancels: [CGFloat] = []
        for connection in [postgres(), nil] as [Connection?] {
            let rig = rig()
            let sheet = try openEditor(connection, in: rig, newKind: connection == nil ? .postgres : nil)
            XCTAssertTrue(wait { self.frame("connection-footer-cancel", in: sheet) != nil })
            let footer = try XCTUnwrap(frame("connection-footer", in: sheet))
            let cancel = try XCTUnwrap(frame("connection-footer-cancel", in: sheet))
            XCTAssertGreaterThan(cancel.minX, footer.midX, "the buttons are not at the right edge: \(cancel) in \(footer)")
            cancels.append(cancel.minX.rounded())
            for window in windows { if let attached = window.attachedSheet { window.endSheet(attached) } }
            spin(0.3)
        }
        XCTAssertEqual(Set(cancels).count, 1, "the buttons moved between an empty footer and one with a summary: \(cancels)")
    }

    func testTheEncryptionControlIsTheSameWidthWhateverItsTitleSays() throws {
        var widths: Set<CGFloat> = []
        var hostWidths: Set<CGFloat> = []
        for tweak in [{ (_: inout Connection) in }, { $0.verify = false; $0.sslmode = "prefer" }, { $0.sslmode = "verify-full"; $0.verify = false }] as [(inout Connection) -> Void] {
            let rig = rig()
            let sheet = try openEditor(postgres(tweak), in: rig)
            XCTAssertTrue(wait { self.frame("connection-encryption", in: sheet) != nil })
            widths.insert(try XCTUnwrap(frame("connection-encryption", in: sheet)).width.rounded())
            hostWidths.insert(try XCTUnwrap(frame("connection-host-field", in: sheet)).width.rounded())
            for window in windows { if let attached = window.attachedSheet { window.endSheet(attached) } }
            spin(0.3)
        }
        XCTAssertEqual(widths, [ConnectionEncryptionMenu.width], "the control's width follows its title: \(widths)")
        XCTAssertEqual(hostWidths.count, 1, "the host field's width changes with the stored choice: \(hostWidths)")
    }

    func testMySQLHasNoCARowAndNoMoreCard() throws {
        let mysql = Connection(id: UUID(), name: "Reporting", color: .amber, kind: .mysql, host: "mysql.internal",
                               port: 3306, sslmode: "disable", user: "u", database: "r", schema: "", verify: true)
        let rig = rig()
        let sheet = try openEditor(mysql, in: rig)
        XCTAssertTrue(wait { self.frame("connection-row-Password", in: sheet) != nil })
        XCTAssertNil(frame("connection-row-CA certificate", in: sheet), "a MySQL form has no CA row")
        XCTAssertNil(frame("connection-row-Schema", in: sheet))
        // The other drivers keep theirs.
        let sibling = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .postgres, host: "pg.internal",
                                 port: 5432, sslmode: "prefer", user: "analyst", database: "warehouse", schema: "public", verify: true)
        let rig2 = self.rig()
        let sheet2 = try openEditor(sibling, in: rig2)
        XCTAssertTrue(wait { self.frame("connection-row-CA certificate", in: sheet2) != nil })
    }

    private func popoverWindows() -> [NSWindow] {
        var found: [NSWindow] = NSApp.windows
        for window in windows {
            found += window.childWindows ?? []
            found += window.attachedSheet?.childWindows ?? []
        }
        // A closed popover's window can outlive it, hidden.
        return found.filter { String(describing: Swift.type(of: $0)).contains("Popover") && $0.isVisible }
    }

    /// The sentence under "Connect through an SSH tunnel" used to take two lines of the pane. It is
    /// a "?" beside the row's label now, and opens on a click (a hover shows the tooltip `HelpHint`
    /// carries, which a test cannot see).
    func testALongCaptionIsBehindAQuestionMarkAndItsHintOpensOnClick() throws {
        let rig = rig()
        let sheet = try openEditor(postgres { $0.sshHost = "bastion.corp"; $0.sshUser = "deploy" }, in: rig, section: .sshTunnel)
        XCTAssertTrue(wait { self.frame("connection-row-Tunnel", in: sheet) != nil }, "no Tunnel row")
        let row = try XCTUnwrap(frame("connection-row-Tunnel", in: sheet))
        // A checkbox and a two-line caption were about 75 points; the checkbox alone is about 42.
        XCTAssertLessThan(row.height, 55, "the caption is still drawn under the checkbox")
        let root = try XCTUnwrap(sheet.contentView)
        let hints = descendants(NSView.self, in: root).filter { $0.identifier?.rawValue == "connection-hint" }
        let beside = try XCTUnwrap(hints.first { row.contains($0.convert($0.bounds, to: nil)) }, "no \"?\" in the Tunnel row")
        // Beside the label, which is the left column of the row.
        XCTAssertLessThan(beside.convert(beside.bounds, to: nil).maxX, row.minX + 12 + 128 + 1)

        let before = popoverWindows().count
        let target = beside.convert(beside.bounds, to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(
                with: type, location: NSPoint(x: target.midX, y: target.midY), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: sheet.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
            sheet.sendEvent(event)
            spin(0.1)
        }
        XCTAssertTrue(wait { self.popoverWindows().count > before }, "clicking the \"?\" did not open its hint")
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

    /// A Keychain that refuses every read, for the Test that cannot fetch a saved secret.
    private struct UnreadableKeychain: SecretStoring {
        struct Refused: LocalizedError { var errorDescription: String? { "denied" } }
        func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool { true }
        func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? { throw Refused() }
        func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {}
        func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws {}
    }

    func testAKeychainReadThatFailsIsATestResultLikeAnyOtherAndAnEditEndsIt() throws {
        let previous = AppModel.secretStore
        AppModel.secretStore = UnreadableKeychain()
        addTeardownBlock { AppModel.secretStore = previous }
        let rig = rig()
        let sheet = try openEditor(postgres(), in: rig)
        XCTAssertTrue(wait { self.frame("connection-footer", in: sheet) != nil })
        XCTAssertNil(marker("connection-footer-result", in: sheet))
        // Test with the password field blank asks the Keychain for the saved one, and is refused.
        try pressTest(in: sheet)
        XCTAssertTrue(wait { self.marker("connection-footer-result", in: sheet) != nil },
                      "the Keychain failure is not shown as a Test result")
        // The person edits: the result described a form that is gone, and must not stick.
        try type("x", intoFieldAt: "connection-host-field", in: sheet)
        XCTAssertTrue(wait { self.marker("connection-footer-result", in: sheet) == nil },
                      "a failed Keychain read stayed over a later edit")
        // Clearing a required field now shows what is missing, which the stale result used to hide.
        let host = try XCTUnwrap(textField(at: "connection-host-field", in: sheet))
        host.currentEditor()?.selectAll(nil)
        host.currentEditor()?.insertText("")
        XCTAssertTrue(wait { self.marker("connection-footer-problem", in: sheet) != nil },
                      "the summary of what is missing is hidden")
    }

    /// ⌘↩, the shortcut for Test Connection.
    private func pressTest(in sheet: NSWindow, file: StaticString = #filePath, line: UInt = #line) throws {
        let key = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.command],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: sheet.windowNumber, context: nil,
            characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: 36), file: file, line: line)
        XCTAssertTrue(sheet.performKeyEquivalent(with: key), "⌘↩ was not taken by the sheet", file: file, line: line)
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

    /// Types into a field through its field editor, as a person does: the field takes the focus and
    /// the text goes in with the editor, not into the model. Nothing resigns the field; leaving it
    /// is up to whatever the test does next (a click on the sidebar).
    private func type(_ text: String, intoFieldAt name: String, in sheet: NSWindow,
                      file: StaticString = #filePath, line: UInt = #line) throws {
        XCTAssertTrue(wait { self.textField(at: name, in: sheet) != nil }, "no field at \(name)", file: file, line: line)
        let field = try XCTUnwrap(textField(at: name, in: sheet), file: file, line: line)
        XCTAssertTrue(sheet.makeFirstResponder(field), file: file, line: line)
        field.currentEditor()?.selectAll(nil)
        field.currentEditor()?.insertText(text)
        spin(0.2)
    }

    /// Typed into Host, Port, the password, SSH port and the statement timeout, then a trip to
    /// another section and back by clicking the sidebar. A numeric field commits when it is left
    /// (`TextField(value:format:)`), and leaving a section removes it from the window, so this is
    /// the case where a number could come back as it was.
    func testTypedValuesSurviveASectionSwitch() throws {
        let rig = rig()
        let tunnelled = postgres { $0.sshHost = "bastion.corp"; $0.sshUser = "deploy"; $0.sshPort = 22 }
        let sheet = try openEditor(tunnelled, in: rig)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) }, "the form does not open on General")

        try type("db.example.org", intoFieldAt: "connection-host-field", in: sheet)
        try type("5433", intoFieldAt: "connection-port-field", in: sheet)
        let root = try XCTUnwrap(sheet.contentView)
        let password = try XCTUnwrap(descendants(NSSecureTextField.self, in: root).first, "no password field")
        XCTAssertTrue(sheet.makeFirstResponder(password))
        password.currentEditor()?.insertText("s3cret")
        spin(0.2)

        try click("connection-section-sshTunnel", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.sshTunnel, in: sheet) }, "the sidebar did not switch to SSH Tunnel")
        try type("2222", intoFieldAt: "connection-ssh-port-field", in: sheet)

        try click("connection-section-limits", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.limits, in: sheet) })
        let timeout = try XCTUnwrap(descendants(NSTextField.self, in: root).first { $0.isEditable },
                                    "no statement timeout field")
        XCTAssertTrue(sheet.makeFirstResponder(timeout))
        timeout.currentEditor()?.insertText("45")
        spin(0.2)

        try click("connection-section-general", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.general, in: sheet) })
        XCTAssertTrue(wait { self.textField(at: "connection-host-field", in: sheet) != nil })
        XCTAssertEqual(textField(at: "connection-host-field", in: sheet)?.stringValue, "db.example.org",
                       "what was typed in Host did not survive a trip to another section")
        XCTAssertEqual(textField(at: "connection-port-field", in: sheet)?.stringValue, "5433",
                       "the port reverted: a number typed into a field that was then removed was not committed")
        let passwordAgain = try XCTUnwrap(descendants(NSSecureTextField.self, in: root).first)
        XCTAssertEqual(passwordAgain.stringValue, "s3cret", "the typed password did not survive")

        try click("connection-section-sshTunnel", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.sshTunnel, in: sheet) })
        XCTAssertTrue(wait { self.textField(at: "connection-ssh-port-field", in: sheet) != nil })
        XCTAssertEqual(textField(at: "connection-ssh-port-field", in: sheet)?.stringValue, "2222",
                       "the SSH port reverted")

        try click("connection-section-limits", in: sheet)
        XCTAssertTrue(wait { self.isShowing(.limits, in: sheet) })
        let timeoutAgain = try XCTUnwrap(descendants(NSTextField.self, in: root).first { $0.isEditable })
        XCTAssertEqual(timeoutAgain.stringValue, "45", "the statement timeout did not survive")
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
