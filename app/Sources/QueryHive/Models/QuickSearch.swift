import Foundation

/// One thing Open Quickly can offer, and what picking it does.
///
/// The action is the whole difference between a result that navigates and one that fills the
/// editor, so it is carried with the result rather than looked up again when it is picked.
struct QuickResult: Identifiable, Equatable {
    enum Action: Equatable {
        /// Select this node in the object tree, expanding what has to open for it to be visible.
        case revealNode(String)
        /// Put this statement into the tab in front.
        case loadSQL(String)
    }

    let id: String
    let action: Action
    let title: String
    let subtitle: String
    let symbol: String
}

/// Which of the three sources Open Quickly is looking at.
enum QuickScope: String, CaseIterable {
    case all, objects, saved, history

    var title: String {
        switch self {
        case .all: "All"
        case .objects: "Objects"
        case .saved: "Saved"
        case .history: "History"
        }
    }

    /// The next scope in the ring, or the previous one for Shift-Tab.
    func rotated(by delta: Int) -> QuickScope {
        let all = QuickScope.allCases
        let index = all.firstIndex(of: self)!
        return all[(index + delta % all.count + all.count) % all.count]
    }
}

/// What Open Quickly searches, and in what order it offers what it found.
///
/// Three sources, all of them already in memory: the object tree's loaded nodes, the saved queries
/// and the history. That is the honest scope of this command. Searching the *server* for a table
/// would mean opening a connection per keystroke, which is a different feature with a different
/// cost, and the tree is already the app's answer to "what is there".
///
/// A pure function over those three lists rather than a method on the model, so the ranking can be
/// checked without building a model, a window or a server.
enum QuickSearch {
    static func results(query: String,
                        nodes: [TreeNode],
                        savedQueries: [Event.SavedQuery],
                        history: [Event.HistoryEntry],
                        scope: QuickScope = .all,
                        limit: Int = 40) -> [QuickResult] {
        var candidates: [QuickResult] = []
        candidates.reserveCapacity(nodes.count + savedQueries.count + history.count)

        for node in nodes where scope == .all || scope == .objects {
            candidates.append(QuickResult(
                id: "node:\(node.id)",
                action: .revealNode(node.id),
                title: node.title,
                subtitle: node.subtitle ?? node.kind.noun,
                symbol: node.kind.symbol))
        }
        for query in savedQueries where scope == .all || scope == .saved {
            candidates.append(QuickResult(
                id: "saved:\(query.id)",
                action: .loadSQL(query.sql),
                title: query.name,
                subtitle: firstLine(query.sql),
                symbol: "bookmark"))
        }
        for entry in history where scope == .all || scope == .history {
            candidates.append(QuickResult(
                id: "history:\(entry.id)",
                action: .loadSQL(entry.sql),
                title: firstLine(entry.sql),
                subtitle: entry.outcome ?? "history",
                symbol: "clock"))
        }

        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return Array(candidates.prefix(limit)) }

        // A prefix match is a better answer than one found in the middle, and both are better than
        // a match in the subtitle. Ties go to the shorter title, which is the more specific name.
        let ranked = candidates.compactMap { candidate -> (result: QuickResult, rank: Int)? in
            let title = candidate.title.lowercased()
            if title.hasPrefix(needle) { return (candidate, 0) }
            if title.contains(needle) { return (candidate, 1) }
            if candidate.subtitle.lowercased().contains(needle) { return (candidate, 2) }
            return nil
        }
        return ranked
            .sorted { ($0.rank, $0.result.title.count) < ($1.rank, $1.result.title.count) }
            .prefix(limit)
            .map(\.result)
    }

    /// The first line of a statement that has anything on it, which is what a one-line row can
    /// show of it. Leading blank lines are skipped rather than shown as an empty title.
    static func firstLine(_ sql: String) -> String {
        let line = sql.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty } ?? ""
        return line.isEmpty ? "statement" : line
    }
}
