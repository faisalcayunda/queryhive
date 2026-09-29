import Foundation

/// A binary cell's bytes as a hex dump, for the viewer's Hex mode.
///
/// The engine hands the app bytes as lowercase hex with no prefix
/// (`crates/qh-core/src/render.rs::hex_encode`), so a `bytea`/`BLOB`/`varbinary` cell's text *is* a
/// hex string in this app. Decoding it back is therefore the common case rather than a heuristic;
/// a `\x` or `0x` prefix is accepted too because some drivers spell it that way, and anything that
/// is not clean, even-length hex is taken as UTF-8 text, which is what a driver that already
/// decoded the bytes would send.
enum HexDump {
    /// The most bytes the dump shows, in bytes. TablePro's 10 KB.
    static let limit = 10_000

    /// The bytes behind a cell's text.
    static func bytes(from text: String) -> Data {
        decodedHex(text) ?? Data(text.utf8)
    }

    /// The hex decoding, or `nil` when the text is not a hex string.
    static func decodedHex(_ text: String) -> Data? {
        var body = Substring(text)
        if body.hasPrefix("\\x") || body.hasPrefix("\\X") {
            body = body.dropFirst(2)
        } else if body.hasPrefix("0x") || body.hasPrefix("0X") {
            body = body.dropFirst(2)
        }
        guard body.count % 2 == 0 else { return nil }
        var data = Data(capacity: body.count / 2)
        var high: UInt8?
        for scalar in body.unicodeScalars {
            guard let nibble = hex(scalar) else { return nil }
            if let pending = high {
                data.append(pending << 4 | nibble)
                high = nil
            } else {
                high = nibble
            }
        }
        return high == nil ? data : nil
    }

    /// The classic `offset  hex bytes  |ascii|` layout, capped at `limit` bytes.
    ///
    /// Cap and layout are both stated rather than silent: a dump that quietly stopped mid-row would
    /// look like a value that ended, and the marker says the value did not.
    static func dump(_ data: Data, limit: Int = HexDump.limit, bytesPerLine: Int = 16) -> String {
        guard !data.isEmpty else { return "(no bytes)" }
        let shown = min(data.count, limit)
        var lines: [String] = []
        var offset = 0
        while offset < shown {
            let end = min(offset + bytesPerLine, shown)
            let slice = data[offset..<end]
            var hexPart = ""
            var asciiPart = ""
            for position in 0..<bytesPerLine {
                if position == 8 { hexPart += " " }
                if position < slice.count {
                    let byte = slice[slice.startIndex + position]
                    hexPart += String(format: "%02x ", byte)
                    asciiPart += (byte >= 0x20 && byte < 0x7F) ? String(Unicode.Scalar(byte)) : "."
                } else {
                    hexPart += "   "
                    asciiPart += " "
                }
            }
            lines.append(String(format: "%08x  %@ |%@|", offset, hexPart, asciiPart))
            offset = end
        }
        if data.count > shown {
            lines.append("… \(data.count - shown) more bytes; Copy keeps the whole value.")
        }
        return lines.joined(separator: "\n")
    }

    private static func hex(_ scalar: Unicode.Scalar) -> UInt8? {
        switch scalar {
        case "0"..."9": return UInt8(scalar.value - 0x30)
        case "a"..."f": return UInt8(scalar.value - 0x61 + 10)
        case "A"..."F": return UInt8(scalar.value - 0x41 + 10)
        default: return nil
        }
    }
}
