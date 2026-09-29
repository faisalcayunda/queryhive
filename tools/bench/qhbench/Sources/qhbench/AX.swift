import Foundation
import ApplicationServices
import AppKit

/// Locating the app's window, editor and grid through the accessibility tree.
enum AX {
    static func attr(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
    }

    static func rect(_ e: AXUIElement) -> CGRect? {
        guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute) else { return nil }
        var pt = CGPoint.zero
        var sz = CGSize.zero
        guard AXValueGetValue(p as! AXValue, .cgPoint, &pt), AXValueGetValue(s as! AXValue, .cgSize, &sz) else { return nil }
        return CGRect(origin: pt, size: sz)
    }

    static func window(pid: pid_t) -> AXUIElement? {
        let app = AXUIElementCreateApplication(pid)
        if let w = attr(app, kAXFocusedWindowAttribute) ?? attr(app, kAXMainWindowAttribute) {
            return (w as! AXUIElement)
        }
        if let ws = attr(app, kAXWindowsAttribute) as? [AXUIElement] { return ws.first }
        return nil
    }

    /// The largest element whose role (or identifier) matches the spec, skipping `excluding`.
    static func find(in root: AXUIElement, spec: RegionSpec, excluding: CGRect? = nil, accept: (CGRect) -> Bool = { _ in true }) -> (AXUIElement, CGRect)? {
        var best: (AXUIElement, CGRect)?
        var visited = 0
        func walk(_ e: AXUIElement, depth: Int) {
            guard depth < 14, visited < 6000 else { return }
            visited += 1
            let role = attr(e, kAXRoleAttribute) as? String ?? ""
            let ident = attr(e, kAXIdentifierAttribute) as? String ?? ""
            let idMatch = spec.identifiers?.contains(ident) ?? false
            if spec.roles.contains(role) || idMatch, let r = rect(e), r.width > 40, r.height > 20, r != excluding, accept(r) {
                if best == nil || r.width * r.height > best!.1.width * best!.1.height { best = (e, r) }
            }
            if let kids = attr(e, kAXChildrenAttribute) as? [AXUIElement] {
                for k in kids { walk(k, depth: depth + 1) }
            }
        }
        walk(root, depth: 0)
        return best
    }

    /// Caret bounds via AXSelectedTextRange + AXBoundsForRange, in global screen points.
    static func caretRect(_ editor: AXUIElement) -> CGRect? {
        guard let sel = attr(editor, kAXSelectedTextRangeAttribute) else { return nil }
        var out: CFTypeRef?
        let st = AXUIElementCopyParameterizedAttributeValue(
            editor, kAXBoundsForRangeParameterizedAttribute as CFString, sel, &out)
        guard st == .success, let v = out else { return nil }
        var r = CGRect.zero
        return AXValueGetValue(v as! AXValue, .cgRect, &r) ? r : nil
    }

    static func focus(_ e: AXUIElement) {
        AXUIElementSetAttributeValue(e, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }

    /// Put the caret in the middle of the document. Returns the character count.
    @discardableResult
    static func placeCaretInMiddle(_ editor: AXUIElement) -> Int? {
        focus(editor)
        guard let n = attr(editor, kAXNumberOfCharactersAttribute) as? Int else { return nil }
        var range = CFRange(location: n / 2, length: 0)
        guard let v = AXValueCreate(.cfRange, &range) else { return nil }
        AXUIElementSetAttributeValue(editor, kAXSelectedTextRangeAttribute as CFString, v)
        return n
    }
}

/// The resolved regions of the target for one run, in global screen points (top-left origin).
struct Regions {
    let pid: pid_t
    let windowFrame: CGRect
    let editor: (AXUIElement, CGRect)?
    let grid: CGRect
    let gridFromFallback: Bool
}

func resolveRegions(profile: Profile, app: NSRunningApplication) -> Regions? {
    guard let win = AX.window(pid: app.processIdentifier), let frame = AX.rect(win) else { return nil }
    let editor = AX.find(in: win, spec: profile.editor)
    // A grid is wide: a sidebar outline is narrow, so anything under half the window width is not it.
    let wide: (CGRect) -> Bool = { $0.width >= frame.width * 0.5 }
    if let g = AX.find(in: win, spec: profile.grid, excluding: editor?.1, accept: wide) {
        return Regions(pid: app.processIdentifier, windowFrame: frame, editor: editor, grid: g.1, gridFromFallback: false)
    }
    let f = profile.grid.fallbackWindowFraction ?? [0, 0.5, 1, 0.5]
    let r = CGRect(x: frame.minX + frame.width * f[0], y: frame.minY + frame.height * f[1],
                   width: frame.width * f[2], height: frame.height * f[3])
    return Regions(pid: app.processIdentifier, windowFrame: frame, editor: editor, grid: r, gridFromFallback: true)
}
