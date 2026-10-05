import AppKit

/// What the grid's two tooltip owners — the body and the header band — have to agree on.
///
/// Neither question has an AppKit answer, so they are answered once here instead of being guessed
/// twice in two files (blueprint §8.5).
enum GridToolTip {
    /// The most of a value a tooltip will show, in UTF-16 units.
    ///
    /// A `jsonb` column can hold a megabyte. The old SwiftUI `.help` passed all of it to AppKit,
    /// which lays the whole string out before deciding where to draw it; §8.5 caps it and the cap
    /// is a *small change* recorded in the blueprint, because "the tooltip is the full value" and
    /// "the tooltip is the full value up to 8192 units" are the same sentence at any size a person
    /// can read.
    static let limit = 8_192

    /// `text` cut to `limit` UTF-16 units, with `…` in the last one.
    ///
    /// Cut by grapheme and not by UTF-16 offset: slicing a surrogate pair or a combining sequence
    /// in half produces a string AppKit draws as a replacement character, which is worse than a
    /// shorter one. The ellipsis takes the final unit, so the result never exceeds `limit`.
    static func cap(_ text: String) -> String {
        guard text.utf16.count > limit else { return text }
        var result = ""
        var units = 0
        for character in text {
            let size = String(character).utf16.count
            if units + size > limit - 1 { break }
            result.append(character)
            units += size
        }
        return result + "…"
    }

    /// The pointer's location in `view`'s coordinates, from the point AppKit hands the owner.
    ///
    /// `view(_:stringForToolTip:point:userData:)` documents neither the point's coordinate system
    /// nor a conversion helper, and both readings are plausible: the rectangle is *registered* in
    /// view coordinates, while the tracking that fires it runs in window ones. So the registered
    /// rectangle settles it — whichever reading lands inside the region that is actually on screen
    /// is where the pointer is — and a tie (a table whose origin sits near the window's) resolves
    /// to the view reading, because that is the one the rectangle was registered in.
    static func local(point: NSPoint, in view: NSView, registered: NSRect?) -> NSPoint {
        guard let registered else { return point }
        let asWindowIfLocal = view.convert(point, to: nil)
        if registered.contains(point) && !registered.contains(asWindowIfLocal) {
            return view.convert(point, from: nil)
        }
        return point
    }
}
