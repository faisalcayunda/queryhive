import Foundation
import Observation

/// Where a table destination writes, resolved from the tab's `targetConnectionID` (W13-T20).
///
/// The tab's own connection keeps the server-side `CREATE TABLE … AS` it always ran. Anything else
/// is a copy across connections, and every way that copy can be unresolvable has its own case, so
/// none of them can quietly become "the tab's own connection".
enum TableDestination: Equatable {
    /// The tab's own connection: no `DEST_` setting is sent.
    case same(Connection)
    /// Another saved connection, with the experimental switch on.
    case other(Connection)
    /// The tab names another connection and the switch is off. Blocked, never the tab's own.
    case switchedOff
    /// The tab names a connection that is not saved any more (deleted, or a restored value that was
    /// not a UUID). Blocked until one is chosen again.
    case missing

    /// The connection whose driver shapes the target name, when there is one.
    var connection: Connection? {
        switch self {
        case .same(let connection), .other(let connection): connection
        case .switchedOff, .missing: nil
        }
    }

    /// Why a run cannot start, for the two cases that name nothing runnable.
    var blockedReason: String? {
        switch self {
        case .same, .other: nil
        case .switchedOff: "Saving to another connection is off in Settings"
        case .missing: "Choose the target connection again"
        }
    }

    /// Why a copy to `target` in `mode` cannot run, decided without the network: the cases the
    /// engine refuses before connecting, said here so the wizard can grey them out (blueprint §8.1).
    /// The engine decides again whatever this says.
    static func refusal(target: Connection, mode: WriteMode) -> String? {
        if target.kind == .trino {
            return "Trino can't be written to from another connection"
        }
        if mode == .append {
            return "Append isn't offered across connections"
        }
        return safeModeRefusal(target: target, mode: mode)
    }

    /// The target's Safe Mode answer for one mode, in the sentence the wizard shows beside it.
    static func safeModeRefusal(target: Connection, mode: WriteMode) -> String? {
        switch target.safeMode {
        case .full:
            return nil
        case .readOnly:
            return "\(target.name) is Read only: nothing is written there"
        case .noDDL:
            return "\(target.name) is No DDL: it refuses CREATE TABLE"
        case .confirm:
            // Create asks once on the transfer sheet; replace drops a table, which Confirm refuses
            // (OQ-9).
            return mode == .create ? nil : "\(target.name) is Confirm: it refuses replacing a table"
        }
    }
}

/// One copy across connections, from the sheet that asks about it to its summary.
///
/// The tab keeps its own record (stage, log, rows), exactly as a table run on one connection does;
/// this is what the transfer sheet draws, so the question, the progress, Stop and the outcome are
/// one window that names both connections throughout.
@Observable
final class TableTransfer: Identifiable {
    enum Phase: Equatable {
        /// Waiting for the person: nothing has run.
        case confirming
        case running
        case finished
        /// The engine answered Stop with `done{cancelled: true}`: the table was not put in place.
        /// Whether anything was left on the target (a scratch table, say) is in `warnings`;
        /// `nothingWritten` says "nothing" only when there are none.
        case stopped
        case failed
        /// Stopped, and the engine never said how far it got (`TransferOutcome.unknown`).
        case stopUnconfirmed
    }

    let id = UUID()
    let tabID: UUID
    let request: RunConfirmation.TransferRequest
    /// The statement the source runs, fixed when the sheet opened, so what runs is what was asked.
    let sql: String
    /// The two connections as they were when the sheet opened, whole: approval checks they still
    /// are, field for field, not only what the sheet happened to show.
    let source: Connection
    let target: Connection

    var phase: Phase = .confirming
    /// The tick the prod banner asks for (`TransferRequest.needsAcknowledgement`).
    var acknowledged = false
    var step: String?
    var rowsWritten = 0
    var rowsRead = 0
    /// The table the engine reported writing, in the target's own spelling.
    var table: String?
    var verified = false
    var warnings: [String] = []
    /// Columns the target got as text, one sentence each.
    var fallbacks: [String] = []
    var failure: String?

    init(tabID: UUID, request: RunConfirmation.TransferRequest, sql: String, source: Connection,
         target: Connection) {
        self.tabID = tabID
        self.request = request
        self.sql = sql
        self.source = source
        self.target = target
    }

    /// Whether the approve button may be pressed.
    var mayApprove: Bool { phase == .confirming && (!request.needsAcknowledgement || acknowledged) }

    /// "Nothing was written to <target>." for a copy that ended without its table, and **only**
    /// when the engine reported nothing it left there. A failure or a stop that left a scratch
    /// table, a transaction it could not roll back, or an old table kept under another name says so
    /// in its warnings, and those are the outcome then (review B-1).
    var nothingWritten: String? {
        warnings.isEmpty ? "Nothing was written to \(request.target.name)." : nil
    }

    /// What introduces the engine's warnings when they are the outcome. Neutral: a warning can be
    /// about the source (a stop the server did not confirm) or a column, not only the target.
    static let warningsLeadIn = "The engine reported:"

    /// What a stop the engine never answered says: the rows may or may not be on the target.
    var stopUnconfirmedLine: String {
        "Stopped after rows were read; check \(request.target.name) for the table."
    }

    /// The one-line outcome the sheet and the log show for a finished copy.
    var summary: String {
        let table = self.table ?? request.table
        return "\(pluralized(rowsWritten, "row")) copied from \(request.source.name) to "
            + "\(request.target.name) · \(table)\(verified ? " (verified)" : "")"
    }
}

/// What a copy across connections ended as, from its exit status, its `done` event and whether
/// Stop was pressed. A function of its own because the case it gets right is easy to get wrong:
/// Stop pressed after the rows were verified lets the swap finish, and the engine then reports the
/// table as written (`cancelled: false`), so "Stop was pressed" alone must not say nothing was.
enum TransferOutcome: Equatable {
    case finished
    case stopped
    case failed
    /// Stop was pressed and the run ended cleanly without a `done`: nobody said whether the table
    /// went in, so the app does not say either way (review B-2).
    case unknown

    ///
    /// `.stopped` is the engine's own word for it, `done{cancelled: true}`, and nothing else: a Stop
    /// followed by an error exit is a failure, whose warnings say what it left behind.
    static func of(status: Int32, done: Event?, stopRequested: Bool) -> TransferOutcome {
        if let done, done.cancelled != true, done.table != nil, status == 0 { return .finished }
        if done?.cancelled == true { return .stopped }
        if done == nil, status == 0, stopRequested { return .unknown }
        return .failed
    }
}
