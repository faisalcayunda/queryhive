import AppKit
import SwiftUI

/// A parsed query plan as an outline: one row per node, the numbers in columns, the node that
/// carries the most of the work marked in words as well as colour.
///
/// Everything shown is server text, so every value goes through `Text(verbatim:)`. The view only
/// reads a `QueryPlan`; wiring it into a tab belongs to the caller.
struct PlanTreeView: View {
    let plan: QueryPlan
    /// The text the plan was parsed from, for "Copy plan".
    var raw: String = ""

    @State private var opened: Set<Int> = []

    var body: some View {
        List {
            OutlineGroup(plan.roots, children: \.optionalChildren) { node in
                PlanRow(
                    node: node, plan: plan, hot: node.id == plan.hottest,
                    open: opened.contains(node.id),
                    toggle: { if !opened.insert(node.id).inserted { opened.remove(node.id) } })
            }
        }
        .listStyle(.plain)
        .accessibilityLabel("Query plan")
        .contextMenu {
            Button("Copy plan") { copy(raw.isEmpty ? PlanText.describe(plan) : raw) }
        }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}

private struct PlanRow: View {
    let node: PlanNode
    let plan: QueryPlan
    let hot: Bool
    let open: Bool
    let toggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                if hot {
                    Image(systemName: "flame.fill").foregroundStyle(Tone.amber).accessibilityHidden(true)
                }
                Text(verbatim: node.title).font(.ui(12, weight: .semibold))
                if let subtitle = node.subtitle {
                    Text(verbatim: subtitle).font(.code(11)).foregroundStyle(Tone.secondary).lineLimit(1)
                }
                if hot {
                    Text("Hottest").font(.ui(10, weight: .bold)).foregroundStyle(Tone.amber)
                }
                Spacer(minLength: 8)
                ForEach(Array(PlanText.columns(node, plan).enumerated()), id: \.offset) { _, text in
                    Text(verbatim: text).font(.code(10.5)).foregroundStyle(Tone.secondary)
                }
                bar
                if !node.facts.isEmpty {
                    Button(action: toggle) {
                        Image(systemName: open ? "chevron.up" : "chevron.down").font(.system(size: 9))
                    }
                    .buttonStyle(.plain)
                    .help(open ? "Hide details" : "Show details")
                    .accessibilityLabel(open ? "Hide details" : "Show details")
                }
            }
            if open {
                ForEach(Array(node.facts.enumerated()), id: \.offset) { _, fact in
                    Text(verbatim: "\(fact.key): \(fact.value)")
                        .font(.code(10.5)).foregroundStyle(Tone.secondary).textSelection(.enabled)
                }
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text(verbatim: PlanText.spoken(node, plan, hot: hot)))
        .contextMenu {
            Button("Copy node") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(PlanText.describe(node), forType: .string)
            }
        }
    }

    private var bar: some View {
        let share = node.share ?? 0
        return ZStack(alignment: .leading) {
            Capsule().fill(Tone.ink.opacity(0.08))
            Capsule().fill(hot ? Tone.amber : Tone.accent).frame(width: 48 * min(max(share, 0), 1))
        }
        .frame(width: 48, height: 5)
        .accessibilityHidden(true)
    }
}

/// The words for a node: columns, spoken label, and plain-text copy. Pure, so it is testable.
enum PlanText {
    static func columns(_ node: PlanNode, _ plan: QueryPlan) -> [String] {
        var out: [String] = []
        if let rows = node.estimatedRows { out.append("est \(number(rows)) rows") }
        if let rows = node.actualRows {
            out.append("\(number(rows)) rows" + ((node.loops ?? 1) > 1 ? " x\(node.loops!)" : ""))
        }
        switch plan.hotMetric {
        case .selfTime: if let ms = node.selfMillis { out.append("\(number(ms)) ms") }
        case .estimatedCost: if let cost = node.cost { out.append("cost \(number(cost))") }
        case .estimatedCPU: if let cpu = node.cost { out.append("cpu \(number(cpu))") }
        }
        if let share = node.share { out.append("\(Int((share * 100).rounded()))%") }
        return out
    }

    static func spoken(_ node: PlanNode, _ plan: QueryPlan, hot: Bool) -> String {
        var parts = [node.title]
        if let subtitle = node.subtitle { parts.append(subtitle) }
        parts += columns(node, plan)
        if hot { parts.append("hottest node") }
        return parts.joined(separator: ", ")
    }

    static func describe(_ node: PlanNode) -> String {
        ([node.title + (node.subtitle.map { " " + $0 } ?? "")] + node.facts.map { "\($0.key): \($0.value)" })
            .joined(separator: "\n")
    }

    static func describe(_ plan: QueryPlan) -> String {
        func lines(_ node: PlanNode, _ depth: Int) -> [String] {
            [String(repeating: "  ", count: depth) + describe(node).replacingOccurrences(of: "\n", with: " | ")]
                + node.children.flatMap { lines($0, depth + 1) }
        }
        return plan.roots.flatMap { lines($0, 0) }.joined(separator: "\n")
    }

    /// Whole numbers without a fraction, the rest to two decimals.
    static func number(_ value: Double) -> String {
        value == value.rounded() && abs(value) < 1e15
            ? String(Int64(value)) : String(format: "%.2f", value)
    }
}
