import Foundation

/// A JSON value laid out as a tree, with the cap that keeps the tree a view rather than a stall.
///
/// This is the collapsible half of the cell reader, beside the text the viewer already showed.
/// `GridValue.parseLimit` is still the outer limit — over it nothing is parsed at all — and this is
/// the inner one: what to do once the value *has* parsed. A tree with a hundred thousand rows is
/// its own hang, so past `nodeLimit` the members that did not fit are replaced by one marker that
/// says how many were left out, rather than a tree that stops mid-branch with no explanation.
///
/// It parses the value's own text rather than going through `JSONSerialization` for one reason:
/// that API hands back an `NSDictionary`, which does not keep the key order the server sent, and a
/// tree that silently reorders keys would disagree with the value it claims to show. The parser
/// here is small because JSON is small; anything it does not understand makes `build` fall back to
/// `notJSON`, and the text mode is always there to take over.
enum JSONTree {
    /// The most nodes the tree will build, root included. TablePro's number.
    static let nodeLimit = 5_000

    /// The longest scalar a row shows, in UTF-16 units.
    ///
    /// The whole scalar is still kept in the node — search has to see all of it, and so does the
    /// text mode — this only stops one 100.000-character string from becoming a 100.000-character
    /// row that the tree has to lay out.
    static let scalarDisplayLimit = 500

    /// What `build` found.
    enum Outcome: Equatable {
        /// Over `GridValue.parseLimit`: no object is built, and the viewer has to stay on Text.
        case tooLarge
        /// Not JSON — a PostgreSQL array literal, say (`docs/golden-deltas.md` D-9).
        case notJSON
        case tree(Node)

        var isTree: Bool {
            if case .tree = self { return true }
            return false
        }
    }

    /// One row of the tree.
    struct Node: Identifiable, Equatable {
        enum Kind: Equatable {
            case object, array, string, number, bool, null, truncated
        }

        /// Assigned in build order. Stable for one build, which is all a view needs.
        let id: Int
        let kind: Kind
        /// The member's key, or `[i]` for an array element. Empty for the root and for the marker.
        let label: String
        /// A scalar's whole text, or a container's summary. Empty for the marker.
        let value: String
        var children: [Node]

        var isContainer: Bool { kind == .object || kind == .array }
        /// The node the cap inserted where members were left out.
        var isMarker: Bool { kind == .truncated }

        /// Every real node at or under this one; markers are not real nodes and are excluded unless
        /// asked for. Used by the tests to hold `nodeLimit` to what its name says.
        func count(includingMarkers: Bool = false) -> Int {
            let selfCount = (includingMarkers || !isMarker) ? 1 : 0
            return selfCount + children.reduce(0) { $0 + $1.count(includingMarkers: includingMarkers) }
        }
    }

    /// What a search found: which nodes are hits, and which containers have to be open for those
    /// hits to be visible.
    struct Search: Equatable {
        var hits: Set<Int> = []
        var expand: Set<Int> = []
    }

    /// Build the tree for a value, or say why it cannot be built.
    static func build(from text: String) -> Outcome {
        // Before the parse, not after: the point of the limit is not to build the object at all.
        guard (text as NSString).length <= GridValue.parseLimit else { return .tooLarge }
        var scanner = JSONScanner(text)
        guard let raw = scanner.parse() else { return .notJSON }
        var builder = JSONTreeBuilder()
        return .tree(builder.build(raw, label: ""))
    }

    /// The text one row shows for a node: a quoted, escaped string, a shortened long value, or the
    /// container's own summary. The value stays whole in the node; this is only what is drawn.
    static func display(_ node: Node) -> String {
        if node.isContainer { return node.value }
        if node.kind == .string { return quoted(node.value) }
        return shortened(node.value)
    }

    /// A string with its control characters spelled out, so one row is always one line.
    static func quoted(_ text: String) -> String {
        var out = String.UnicodeScalarView()
        out.append("\"")
        for scalar in text.unicodeScalars {
            switch scalar {
            case "\"": out.append(contentsOf: "\\\"".unicodeScalars)
            case "\\": out.append(contentsOf: "\\\\".unicodeScalars)
            case "\n": out.append(contentsOf: "\\n".unicodeScalars)
            case "\r": out.append(contentsOf: "\\r".unicodeScalars)
            case "\t": out.append(contentsOf: "\\t".unicodeScalars)
            default:
                if scalar.value < 0x20 {
                    out.append(contentsOf: String(format: "\\u%04x", scalar.value).unicodeScalars)
                } else {
                    out.append(scalar)
                }
            }
        }
        out.append("\"")
        return shortened(String(out))
    }

    static func shortened(_ text: String) -> String {
        guard (text as NSString).length > scalarDisplayLimit else { return text }
        return (text as NSString).substring(to: scalarDisplayLimit) + "…"
    }

    /// Case-insensitive search over keys and scalar values, plus the ancestors a hit needs open.
    ///
    /// `nil` for an empty query, which is a different state from "nothing matched": the view draws
    /// no count and no highlight for the first and a coral "No matches" for the second.
    static func search(in root: Node, query: String) -> Search? {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return nil }
        var result = Search()
        collect(root, ancestors: [], needle: needle, into: &result)
        return result
    }

    private static func collect(_ node: Node, ancestors: [Int], needle: String, into result: inout Search) {
        guard !node.isMarker else { return }
        // A container's own value is a summary like "3 keys", which a search for "3" should not
        // turn into noise; only its label and its children can match.
        let hit = node.label.localizedCaseInsensitiveContains(needle)
            || (!node.isContainer && node.value.localizedCaseInsensitiveContains(needle))
        if hit {
            result.hits.insert(node.id)
            result.expand.formUnion(ancestors)
            if node.isContainer { result.expand.insert(node.id) }
        }
        for child in node.children {
            collect(child, ancestors: ancestors + [node.id], needle: needle, into: &result)
        }
    }
}

// MARK: - Parsing

/// The parsed value, before the node budget is applied.
///
/// Order-preserving on purpose: an object is a list of members, not a dictionary, because the key
/// order is part of what the tree is showing.
private indirect enum JSONRaw: Equatable {
    case object([JSONMember])
    case array([JSONRaw])
    case string(String)
    case number(String)
    case bool(Bool)
    case null
}

private struct JSONMember: Equatable {
    let key: String
    let value: JSONRaw
}

/// A recursive-descent parser over the JSON grammar, strict enough that a value it accepts is one
/// `JSONSerialization` would also accept and loose enough to keep number and string spelling.
private struct JSONScanner {
    private let scalars: [Unicode.Scalar]
    private var index = 0

    init(_ text: String) {
        scalars = Array(text.unicodeScalars)
    }

    mutating func parse() -> JSONRaw? {
        skipWhitespace()
        guard let value = parseValue() else { return nil }
        skipWhitespace()
        // Trailing content means this is not one JSON value; the text mode is the honest answer.
        guard index == scalars.count else { return nil }
        return value
    }

    private mutating func parseValue() -> JSONRaw? {
        guard let scalar = peek() else { return nil }
        switch scalar {
        case "{": return parseObject()
        case "[": return parseArray()
        case "\"": return parseString().map(JSONRaw.string)
        case "t", "f": return parseBool()
        case "n": return parseNull()
        default:
            guard scalar == "-" || isDigit(scalar) else { return nil }
            return parseNumber().map(JSONRaw.number)
        }
    }

    private mutating func parseObject() -> JSONRaw? {
        guard match("{") else { return nil }
        skipWhitespace()
        if match("}") { return .object([]) }
        var members: [JSONMember] = []
        while true {
            skipWhitespace()
            guard let key = parseString() else { return nil }
            skipWhitespace()
            guard match(":") else { return nil }
            skipWhitespace()
            guard let value = parseValue() else { return nil }
            members.append(JSONMember(key: key, value: value))
            skipWhitespace()
            if match(",") { continue }
            guard match("}") else { return nil }
            return .object(members)
        }
    }

    private mutating func parseArray() -> JSONRaw? {
        guard match("[") else { return nil }
        skipWhitespace()
        if match("]") { return .array([]) }
        var items: [JSONRaw] = []
        while true {
            skipWhitespace()
            guard let value = parseValue() else { return nil }
            items.append(value)
            skipWhitespace()
            if match(",") { continue }
            guard match("]") else { return nil }
            return .array(items)
        }
    }

    private mutating func parseString() -> String? {
        guard match("\"") else { return nil }
        var out = String.UnicodeScalarView()
        while let scalar = peek() {
            index += 1
            if scalar == "\"" { return String(out) }
            if scalar == "\\" {
                guard let escaped = parseEscape() else { return nil }
                out.append(escaped)
            } else if scalar.value < 0x20 {
                return nil   // a raw control character is not a legal JSON string
            } else {
                out.append(scalar)
            }
        }
        return nil
    }

    private mutating func parseEscape() -> Unicode.Scalar? {
        guard let scalar = peek() else { return nil }
        index += 1
        switch scalar {
        case "\"": return "\""
        case "\\": return "\\"
        case "/": return "/"
        case "b": return Unicode.Scalar(0x08)
        case "f": return Unicode.Scalar(0x0C)
        case "n": return "\n"
        case "r": return "\r"
        case "t": return "\t"
        case "u": return parseUnicodeEscape()
        default: return nil
        }
    }

    private mutating func parseUnicodeEscape() -> Unicode.Scalar? {
        guard let high = parseHex4() else { return nil }
        if high >= 0xD800, high <= 0xDBFF {
            // A surrogate pair: the next four characters must be `\u` and the low half, or the
            // string is malformed and the whole parse falls back to text.
            guard match("\\"), match("u"), let low = parseHex4(),
                  low >= 0xDC00, low <= 0xDFFF else { return nil }
            return Unicode.Scalar(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00))
        }
        guard !(high >= 0xDC00 && high <= 0xDFFF) else { return nil }
        return Unicode.Scalar(high)
    }

    private mutating func parseHex4() -> UInt32? {
        var value: UInt32 = 0
        for _ in 0..<4 {
            guard let scalar = peek(), let digit = hexDigit(scalar) else { return nil }
            index += 1
            value = value << 4 | digit
        }
        return value
    }

    private mutating func parseNumber() -> String? {
        let start = index
        _ = match("-")
        if match("0") {
            // A leading zero may not be followed by another digit, which the loop below would allow.
        } else {
            guard let first = peek(), isDigit(first) else { return nil }
            while let scalar = peek(), isDigit(scalar) { index += 1 }
        }
        if match(".") {
            guard let scalar = peek(), isDigit(scalar) else { return nil }
            while let scalar = peek(), isDigit(scalar) { index += 1 }
        }
        var exponent = false
        if match("e") { exponent = true }
        else if match("E") { exponent = true }
        if exponent {
            if !match("+") { _ = match("-") }
            guard let scalar = peek(), isDigit(scalar) else { return nil }
            while let scalar = peek(), isDigit(scalar) { index += 1 }
        }
        return String(String.UnicodeScalarView(scalars[start..<index]))
    }

    private mutating func parseBool() -> JSONRaw? {
        if matchWord("true") { return .bool(true) }
        if matchWord("false") { return .bool(false) }
        return nil
    }

    private mutating func parseNull() -> JSONRaw? {
        matchWord("null") ? .null : nil
    }

    private mutating func matchWord(_ word: String) -> Bool {
        let characters = Array(word.unicodeScalars)
        guard index + characters.count <= scalars.count,
              Array(scalars[index..<index + characters.count]) == characters else { return false }
        index += characters.count
        return true
    }

    private func peek() -> Unicode.Scalar? { index < scalars.count ? scalars[index] : nil }

    private mutating func match(_ scalar: Unicode.Scalar) -> Bool {
        guard index < scalars.count, scalars[index] == scalar else { return false }
        index += 1
        return true
    }

    private mutating func skipWhitespace() {
        while let scalar = peek(),
              scalar == " " || scalar == "\n" || scalar == "\r" || scalar == "\t" {
            index += 1
        }
    }

    private func isDigit(_ scalar: Unicode.Scalar) -> Bool { scalar >= "0" && scalar <= "9" }

    private func hexDigit(_ scalar: Unicode.Scalar) -> UInt32? {
        switch scalar {
        case "0"..."9": return scalar.value - 0x30
        case "a"..."f": return scalar.value - 0x61 + 10
        case "A"..."F": return scalar.value - 0x41 + 10
        default: return nil
        }
    }
}

// MARK: - Node budget

/// Turns the parsed value into display nodes while spending `JSONTree.nodeLimit`.
///
/// When the budget runs out inside a container, the rest of that container's members are counted
/// but not built, and one marker child says how many were skipped. Members of deeper containers is
/// then never reached, which is the point: the tree stops growing, the value does not.
private struct JSONTreeBuilder {
    private var budget = JSONTree.nodeLimit
    private var nextID = 0

    mutating func build(_ raw: JSONRaw, label: String) -> JSONTree.Node {
        let id = nextID
        nextID += 1
        budget -= 1

        switch raw {
        case .object(let members):
            var children: [JSONTree.Node] = []
            var skipped = 0
            for (offset, member) in members.enumerated() {
                guard budget > 0 else { skipped = members.count - offset; break }
                children.append(build(member.value, label: member.key))
            }
            children.append(contentsOf: marker(skipped: skipped))
            return JSONTree.Node(id: id, kind: .object, label: label,
                                 value: "\(members.count) \(members.count == 1 ? "key" : "keys")",
                                 children: children)
        case .array(let items):
            var children: [JSONTree.Node] = []
            var skipped = 0
            for (offset, item) in items.enumerated() {
                guard budget > 0 else { skipped = items.count - offset; break }
                children.append(build(item, label: "[\(offset)]"))
            }
            children.append(contentsOf: marker(skipped: skipped))
            return JSONTree.Node(id: id, kind: .array, label: label,
                                 value: "\(items.count) \(items.count == 1 ? "item" : "items")",
                                 children: children)
        case .string(let text):
            return JSONTree.Node(id: id, kind: .string, label: label, value: text, children: [])
        case .number(let text):
            return JSONTree.Node(id: id, kind: .number, label: label, value: text, children: [])
        case .bool(let flag):
            return JSONTree.Node(id: id, kind: .bool, label: label,
                                 value: flag ? "true" : "false", children: [])
        case .null:
            return JSONTree.Node(id: id, kind: .null, label: label, value: "null", children: [])
        }
    }

    private mutating func marker(skipped: Int) -> [JSONTree.Node] {
        guard skipped > 0 else { return [] }
        let node = JSONTree.Node(id: nextID, kind: .truncated, label: "",
                                 value: "… \(skipped) more not shown", children: [])
        nextID += 1
        return [node]
    }
}
