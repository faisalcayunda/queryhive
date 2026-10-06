import Foundation

/// One node of a query plan, in the shape both PostgreSQL and Trino plans reduce to.
struct PlanNode: Identifiable, Equatable {
    struct Fact: Equatable {
        var key: String
        var value: String
    }

    /// DFS order, stable for one plan.
    let id: Int
    var title: String
    /// Relation or index (PostgreSQL), table or criteria (Trino).
    var subtitle: String?
    var facts: [Fact] = []
    var estimatedRows: Double?
    /// Total rows: `Actual Rows` times `Actual Loops` (PostgreSQL ANALYZE only).
    var actualRows: Double?
    var loops: Int?
    /// Exclusive time: the node's total minus its children's (PostgreSQL ANALYZE only).
    var selfMillis: Double?
    /// The exclusive value the plan's `hotMetric` ranks by: estimated cost (PostgreSQL without
    /// ANALYZE) or `cpuCost` (Trino). Nil under `.selfTime`, where `selfMillis` is the value.
    var cost: Double?
    /// 0...1 against the sum of the exclusive values of every node, for the bar and the marker.
    var share: Double?
    var children: [PlanNode] = []

    /// `OutlineGroup` wants nil, not empty, for a leaf.
    var optionalChildren: [PlanNode]? { children.isEmpty ? nil : children }
}

struct QueryPlan: Equatable {
    enum Dialect { case postgres, trino }
    enum HotMetric { case selfTime, estimatedCost, estimatedCPU }

    var dialect: Dialect
    /// PostgreSQL: one. Trino: fragment 0, with every `RemoteSource` expanded.
    var roots: [PlanNode]
    var planningMillis: Double?
    var executionMillis: Double?
    /// Nil when no node reaches `hotShare`.
    var hottest: PlanNode.ID?
    var hotMetric: HotMetric
    var nodeCount: Int

    /// Plan JSON comes from a server, so every bound is here: depth, node count and text size.
    static let depthLimit = 256
    static let nodeLimit = 20_000
    static let byteLimit = 16 * 1024 * 1024
    static let hotShare = 0.2

    /// Nil means "not a plan this parser understands": the caller shows the raw text.
    static func parse(_ text: String) -> QueryPlan? {
        guard text.utf8.count <= byteLimit, let data = text.data(using: .utf8),
            let json = try? JSONSerialization.jsonObject(with: data)
        else { return nil }
        var builder = Builder()
        let plan: QueryPlan?
        if let array = json as? [Any], let top = array.first as? [String: Any], top["Plan"] is [String: Any] {
            plan = builder.postgres(top)
        } else if let top = json as? [String: Any] {
            plan = top["Plan"] is [String: Any] ? builder.postgres(top) : builder.trino(top)
        } else {
            plan = nil
        }
        return builder.failed ? nil : plan
    }
}

// MARK: - Building

private struct Builder {
    var failed = false
    var count = 0
    /// Exclusive values by DFS id, filled while ids are assigned.
    var values: [Int: Double] = [:]
    var tieBreak: [Int: Double] = [:]

    // MARK: PostgreSQL

    /// A node before ids and shares: the tree as read, with the exclusive value computed.
    private struct Raw {
        var node: PlanNode
        var value: Double?
        var tie: Double = 0
        var children: [Raw] = []
        /// The node's total time and total cost, for the parent's subtraction.
        var totalTime: Double?
        var totalCost: Double?
        var isInitPlan = false
    }

    mutating func postgres(_ top: [String: Any]) -> QueryPlan? {
        guard let plan = top["Plan"] as? [String: Any],
            var root = pgNode(plan, depth: 1, processes: nil)
        else { return nil }
        let timed = root.totalTime != nil
        if timed { applyTime(&root) } else { applyCost(&root) }
        var id = 0
        let finished = finish(root, id: &id)
        let sum = values.values.reduce(0, +)
        return QueryPlan(
            dialect: .postgres, roots: [setShares(finished, sum: sum, timed: timed)],
            planningMillis: number(top["Planning Time"]), executionMillis: number(top["Execution Time"]),
            hottest: hottestID(sum: sum), hotMetric: timed ? .selfTime : .estimatedCost, nodeCount: count)
    }

    private mutating func pgNode(_ dict: [String: Any], depth: Int, processes: Double?) -> Raw? {
        guard depth <= QueryPlan.depthLimit, count < QueryPlan.nodeLimit else {
            failed = true
            return nil
        }
        count += 1
        let type = string(dict["Node Type"]) ?? "Unknown"
        var subtitle: String?
        if let rel = string(dict["Relation Name"]) {
            let alias = string(dict["Alias"])
            subtitle = alias != nil && alias != rel ? "\(rel) as \(alias!)" : rel
        }
        if let index = string(dict["Index Name"]) {
            subtitle = subtitle.map { "\($0) using \(index)" } ?? index
        }
        let loopsRaw = number(dict["Actual Loops"])
        let loops = loopsRaw.map { max(1, $0) }
        let perLoop = number(dict["Actual Total Time"])
        let isGather = type == "Gather" || type == "Gather Merge"
        // Below a Gather, loops count the processes that ran side by side (aware or not), so
        // divide them out: summing concurrent durations is wrong, while a nested-loop inner side
        // inside a worker still multiplies by its own per-worker loops.
        let multiplier = processes.map { max(1, (loops ?? 1) / $0) } ?? (loops ?? 1)
        let totalTime = perLoop.map { $0 * multiplier }
        var node = PlanNode(id: 0, title: type, subtitle: subtitle)
        node.estimatedRows = number(dict["Plan Rows"])
        if let rows = number(dict["Actual Rows"]) {
            node.actualRows = rows * (loops ?? 1)
        }
        node.loops = loops.map { Int(min($0, Double(Int32.max))) }
        for key in ["Join Type", "Strategy", "Filter", "Index Cond", "Hash Cond", "Merge Cond", "Join Filter",
            "Recheck Cond", "Sort Key", "Group Key", "Rows Removed by Filter"]
        {
            if let value = factString(dict[key]) { node.facts.append(.init(key: key, value: value)) }
        }
        var childProcesses = processes
        if isGather {
            let launched = number(dict["Workers Launched"]) ?? number(dict["Workers Planned"]) ?? 0
            childProcesses = launched + 1
        }
        var raw = Raw(node: node, totalTime: totalTime, totalCost: number(dict["Total Cost"]))
        raw.isInitPlan = string(dict["Parent Relationship"]) == "InitPlan"
        if let kids = dict["Plans"] as? [Any] {
            for case let kid as [String: Any] in kids {
                guard let child = pgNode(kid, depth: depth + 1, processes: childProcesses) else { return nil }
                raw.children.append(child)
            }
        }
        return raw
    }

    /// Time: exclusive = total minus children, clamped; an `InitPlan` runs outside its parent's
    /// time (measured), a `SubPlan` inside it, so only the former is not subtracted.
    private func applyTime(_ raw: inout Raw) {
        for i in raw.children.indices { applyTime(&raw.children[i]) }
        let kids = raw.children.filter { !$0.isInitPlan }.compactMap(\.totalTime).reduce(0, +)
        let exclusive = max(0, (raw.totalTime ?? 0) - kids)
        raw.node.selfMillis = raw.totalTime == nil ? nil : exclusive
        raw.value = raw.totalTime == nil ? nil : exclusive
    }

    /// Cost: a `Limit` costs less than its child, so exclusive is clamped and `share` is taken
    /// against the sum of exclusives rather than the root's cost.
    private func applyCost(_ raw: inout Raw) {
        for i in raw.children.indices { applyCost(&raw.children[i]) }
        guard let total = raw.totalCost else { return }
        // Unlike time, PostgreSQL charges an InitPlan's cost into its parent.
        let kids = raw.children.compactMap(\.totalCost).reduce(0, +)
        let exclusive = max(0, total - kids)
        raw.node.cost = exclusive
        raw.value = exclusive
    }

    private mutating func finish(_ raw: Raw, id: inout Int) -> PlanNode {
        var node = raw.node
        node = PlanNode(
            id: id, title: node.title, subtitle: node.subtitle, facts: node.facts,
            estimatedRows: node.estimatedRows, actualRows: node.actualRows, loops: node.loops,
            selfMillis: node.selfMillis, cost: node.cost, share: nil, children: [])
        values[id] = raw.value ?? 0
        tieBreak[id] = raw.tie
        id += 1
        node.children = raw.children.map { finish($0, id: &id) }
        return node
    }

    private func setShares(_ node: PlanNode, sum: Double, timed: Bool) -> PlanNode {
        var out = node
        out.share = sum > 0 && (timed ? node.selfMillis : node.cost) != nil ? (values[node.id] ?? 0) / sum : nil
        out.children = node.children.map { setShares($0, sum: sum, timed: timed) }
        return out
    }

    private func hottestID(sum: Double) -> Int? {
        guard sum > 0 else { return nil }
        let best = values.max { a, b in
            a.value != b.value ? a.value < b.value : (tieBreak[a.key] ?? 0) < (tieBreak[b.key] ?? 0)
        }
        guard let best, best.value / sum >= QueryPlan.hotShare else { return nil }
        return best.key
    }

    // MARK: Trino

    mutating func trino(_ fragments: [String: Any]) -> QueryPlan? {
        guard let zero = fragments["0"] as? [String: Any] else { return nil }
        var visited: Set<String> = ["0"]
        guard let root = trinoNode(zero, fragments: fragments, visited: &visited, depth: 1) else { return nil }
        var id = 0
        let finished = finish(root, id: &id)
        let sum = values.values.reduce(0, +)
        let shaped = setShares(finished, sum: sum, timed: false)
        return QueryPlan(
            dialect: .trino, roots: [shaped], planningMillis: nil, executionMillis: nil,
            hottest: hottestID(sum: sum), hotMetric: .estimatedCPU, nodeCount: count)
    }

    private mutating func trinoNode(
        _ dict: [String: Any], fragments: [String: Any], visited: inout Set<String>, depth: Int
    ) -> Raw? {
        guard depth <= QueryPlan.depthLimit, count < QueryPlan.nodeLimit else {
            failed = true
            return nil
        }
        // A fragment value that is not a node (an IO plan, say) is not a plan we read.
        guard let name = string(dict["name"]) else {
            failed = true
            return nil
        }
        count += 1
        let descriptor = dict["descriptor"] as? [String: Any] ?? [:]
        var node = PlanNode(id: 0, title: name)
        node.subtitle = string(descriptor["table"]) ?? string(descriptor["criteria"])
        for key in descriptor.keys.sorted() {
            if let value = string(descriptor[key]) { node.facts.append(.init(key: key, value: value)) }
        }
        for case let line as String in (dict["details"] as? [Any]) ?? [] {
            node.facts.append(.init(key: "details", value: line))
        }
        let estimate = (dict["estimates"] as? [Any])?.first as? [String: Any]
        node.estimatedRows = number(estimate?["outputRowCount"])
        let cpu = number(estimate?["cpuCost"])
        node.cost = cpu
        var raw = Raw(node: node, value: cpu, tie: node.estimatedRows ?? 0)
        for case let kid as [String: Any] in (dict["children"] as? [Any]) ?? [] {
            guard let child = trinoNode(kid, fragments: fragments, visited: &visited, depth: depth + 1)
            else { return nil }
            raw.children.append(child)
        }
        if let ids = string(descriptor["sourceFragmentIds"]) {
            for fragment in fragmentIDs(ids) where !visited.contains(fragment) {
                visited.insert(fragment)
                guard let sub = fragments[fragment] as? [String: Any] else { continue }
                guard let child = trinoNode(sub, fragments: fragments, visited: &visited, depth: depth + 1)
                else { return nil }
                raw.children.append(child)
            }
        }
        return raw
    }

    /// `"[1, 2]"` to `["1", "2"]`.
    private func fragmentIDs(_ text: String) -> [String] {
        text.split(whereSeparator: { !$0.isNumber }).map(String.init)
    }

    // MARK: Values

    /// Finite numbers only: `"NaN"`, infinities and non-numbers are "not there".
    private func number(_ value: Any?) -> Double? {
        let parsed: Double?
        switch value {
        case let n as NSNumber where CFGetTypeID(n) != CFBooleanGetTypeID(): parsed = n.doubleValue
        case let s as String: parsed = Double(s.trimmingCharacters(in: .whitespaces))
        default: parsed = nil
        }
        guard let parsed, parsed.isFinite else { return nil }
        return parsed
    }

    private func string(_ value: Any?) -> String? {
        switch value {
        case let s as String: return s
        case let n as NSNumber: return CFGetTypeID(n) == CFBooleanGetTypeID() ? (n.boolValue ? "true" : "false") : n.stringValue
        default: return nil
        }
    }

    private func factString(_ value: Any?) -> String? {
        if let array = value as? [Any] { return array.compactMap(string).joined(separator: ", ") }
        return string(value)
    }
}
