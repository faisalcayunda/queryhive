import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// `HelpHint`, the "?" that holds a sentence a screen would otherwise spell out: how big its popover
// is for sentences of different lengths, that Esc closes it and nothing behind it, and the key
// monitor that does that. The pictures of the screens that use it are `ChromeParityTests`.
@MainActor
final class HelpHintPopoverTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func setUp() {
        super.setUp()
        ThemeStore.shared.pin(reduceMotion: true, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: .midnight, accent: .ice, tone: .glow, mode: .dark,
                              systemIsDark: true, uiFont: "", codeFont: "")
    }

    override func tearDown() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
        super.tearDown()
    }

    /// A window holding one `HelpHint`, with a marker where it is.
    private func host(_ text: String, opensOnHover: Bool = false) -> NSWindow {
        let view = HelpHint(text: text, opensOnHover: opensOnHover)
            .background(FrameMarker(name: "hint"))
            .padding(40)
            .frame(width: 400, height: 200)
            .preferredColorScheme(.dark)
        let host = NSHostingView(rootView: view)
        host.frame = CGRect(x: 0, y: 0, width: 400, height: 200)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        window.orderFrontRegardless()
        windows.append(window)
        spin(0.4)
        return window
    }

    private func spin(_ seconds: Double = 0.3) {
        RunLoop.current.run(until: Date().addingTimeInterval(seconds))
    }

    @discardableResult
    private func wait(_ timeout: Double = 5, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        return condition()
    }

    private func descendants(in view: NSView) -> [NSView] {
        [view] + view.subviews.flatMap { descendants(in: $0) }
    }

    private func popovers(of window: NSWindow) -> [NSWindow] {
        let all = (window.childWindows ?? []) + NSApp.windows.filter { !($0 === window) }
        // A closed popover's window can outlive it, hidden.
        return all.filter { String(describing: Swift.type(of: $0)).contains("Popover") && $0.isVisible }
    }

    private func click(_ window: NSWindow) throws {
        let root = try XCTUnwrap(window.contentView)
        let marker = try XCTUnwrap(descendants(in: root).first { $0.identifier?.rawValue == "hint" })
        let rect = marker.convert(marker.bounds, to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(
                with: type, location: NSPoint(x: rect.midX, y: rect.midY), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: 1))
            window.sendEvent(event)
            spin(0.1)
        }
    }

    private func openPopover(_ text: String) throws -> (window: NSWindow, popover: NSWindow) {
        let window = host(text)
        try click(window)
        XCTAssertTrue(wait { !self.popovers(of: window).isEmpty }, "clicking the \"?\" opened nothing")
        spin(0.4)
        return (window, try XCTUnwrap(popovers(of: window).first))
    }

    private func escape(for window: NSWindow) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
            isARepeat: false, keyCode: 53))
    }

    // MARK: Size

    private let short = "Blank keeps it."
    private let medium = "The first time you connect, QueryHive shows the server's fingerprint and asks you to verify it."
    private let long = "Blank means no BasicAuth. Trino refuses BasicAuth over plain http, so a password forces https. "
        + "Prefer is the exception: it starts on HTTPS and falls back to clear only when the coordinator has no TLS."

    /// The popover is as big as its sentence and no bigger: a short sentence is a small popover, a long
    /// one wraps at the maximum width and is only as tall as the wrapped text needs, with no empty box.
    func testThePopoverFitsItsSentenceAtThreeLengths() throws {
        func wrapped(_ text: String, width: CGFloat) -> CGSize {
            let rect = NSAttributedString(string: text, attributes: [.font: NSFont.systemFont(ofSize: 11.5)])
                .boundingRect(with: CGSize(width: width, height: 10_000), options: [.usesLineFragmentOrigin])
            return CGSize(width: rect.width.rounded(.up), height: rect.height.rounded(.up))
        }
        var sizes: [CGSize] = []
        for text in [short, medium, long] {
            let (window, popover) = try openPopover(text)
            sizes.append(popover.frame.size)
            NSApp.sendEvent(try escape(for: window))
            XCTAssertTrue(wait { self.popovers(of: window).isEmpty })
        }
        let (s, m, l) = (sizes[0], sizes[1], sizes[2])
        XCTAssertLessThan(s.width, m.width, "short \(s), medium \(m)")
        XCTAssertLessThanOrEqual(m.width, l.width, "medium \(m), long \(l)")
        XCTAssertLessThan(s.height, m.height, "short \(s), medium \(m)")
        XCTAssertLessThan(m.height, l.height, "medium \(m), long \(l)")
        XCTAssertLessThan(l.height, 200, "the tallest is \(l)")
        // Wider than the text and its padding by no more than the popover's own chrome (the arrow and
        // the border, about 26 points), and never wider than the maximum.
        let chrome: CGFloat = 40
        for (text, size) in zip([short, medium, long], sizes) {
            let box = wrapped(text, width: HelpHint.popoverWidth)
            // The width is the sentence on one line, or the maximum when it does not fit on one.
            let oneLine = wrapped(text, width: 10_000).width
            let width = min(oneLine, HelpHint.popoverWidth)
            XCTAssertLessThanOrEqual(size.width, width + 2 * HelpHint.popoverPadding + chrome, "wider than \"\(text.prefix(20))…\" needs: \(size) for \(width)")
            XCTAssertLessThanOrEqual(size.height, box.height + 2 * HelpHint.popoverPadding + chrome, "taller than \"\(text.prefix(20))…\" needs: \(size) for \(box)")
        }
        XCTAssertLessThanOrEqual(l.width, HelpHint.popoverWidth + 2 * HelpHint.popoverPadding + chrome)
    }

    // MARK: Esc

    func testEscapeClosesTheOpenPopover() throws {
        let (window, _) = try openPopover(medium)
        XCTAssertEqual(EscapeWhileOpen.installed, 1, "no key monitor is installed while the popover is open")
        NSApp.sendEvent(try escape(for: window))
        XCTAssertTrue(wait { self.popovers(of: window).isEmpty }, "Esc did not close the popover")
    }
}
