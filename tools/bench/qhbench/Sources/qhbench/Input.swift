import Foundation
import CoreGraphics
import ApplicationServices
import AppKit

/// Synthetic input through CGEvent, posted to the HID tap so the frontmost app receives it the
/// way a keyboard or trackpad would deliver it. Needs Accessibility.
enum Input {
    /// The one process allowed to receive events. Every post re-checks that it is still frontmost,
    /// so a focus change mid-run makes the harness stop instead of typing into another app.
    nonisolated(unsafe) static var guardPid: pid_t = 0

    static func frontmostOK() -> Bool {
        RunLoop.current.run(until: Date())  // let NSWorkspace see focus changes in a CLI process
        return guardPid != 0 && NSWorkspace.shared.frontmostApplication?.processIdentifier == guardPid
    }

    private static let keyCodes: [String: CGKeyCode] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11,
        "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21,
        "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35,
        "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, ".": 47, "return": 36, "tab": 48, "space": 49,
        "escape": 53, "left": 123, "right": 124, "down": 125, "up": 126,
    ]

    static func flags(_ mods: [String]) -> CGEventFlags {
        var f: CGEventFlags = []
        for m in mods {
            switch m.lowercased() {
            case "cmd", "command": f.insert(.maskCommand)
            case "shift": f.insert(.maskShift)
            case "opt", "option", "alt": f.insert(.maskAlternate)
            case "ctrl", "control": f.insert(.maskControl)
            default: break
            }
        }
        return f
    }

    /// Returns the mach-clock time (ns) right before the key-down was posted.
    @discardableResult
    static func chord(_ c: KeyChord) -> UInt64? {
        guard frontmostOK() else { return nil }
        guard let code = keyCodes[c.key.lowercased()] else { return nil }
        let f = flags(c.modifiers)
        let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
        let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
        down?.flags = f
        up?.flags = f
        let t = Clock.nowNs()
        down?.post(tap: .cghidEventTap)
        up?.post(tap: .cghidEventTap)
        return t
    }

    /// One character typed as a unicode key event. Returns the time just before the key-down.
    @discardableResult
    static func type(_ ch: Character) -> UInt64? {
        guard frontmostOK() else { return nil }
        let units = Array(String(ch).utf16)
        let down = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true)
        let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false)
        down?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        up?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        let t = Clock.nowNs()
        down?.post(tap: .cghidEventTap)
        up?.post(tap: .cghidEventTap)
        return t
    }

    static func typeString(_ s: String, gapMs: Double = 2) -> Bool {
        for ch in s {
            guard type(ch) != nil else { return false }
            sleepMs(gapMs)
        }
        return true
    }

    /// Pixel-unit scroll wheel event at a fixed screen point. Positive `dy` scrolls content down.
    @discardableResult
    static func scroll(dx: Int32, dy: Int32, at point: CGPoint) -> Bool {
        guard frontmostOK() else { return false }
        let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                        wheel1: -dy, wheel2: -dx, wheel3: 0)
        e?.location = point
        e?.post(tap: .cghidEventTap)
        return true
    }

    @discardableResult
    static func moveMouse(to p: CGPoint) -> Bool {
        guard frontmostOK() else { return false }
        CGWarpMouseCursorPosition(p)
        return true
    }
}
