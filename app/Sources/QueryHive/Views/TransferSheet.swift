import SwiftUI

/// The one window a copy across connections lives in (W13-T20, blueprint §7.3, §7.4 and §8.4).
///
/// It asks first, every time (B-6): both connections by name, driver, address, database, label and
/// TLS, the table and the mode, and a tick when a prod source copies to something that is not prod.
/// Approved, the same sheet shows the progress with Stop, then the outcome naming both connections,
/// with the columns that were widened or written as text. Closing it while the copy runs leaves the
/// copy running; its tab keeps reporting, and Stop is there too.
///
/// A fixed size (§8.5): the part that can grow, the warnings, scrolls inside it.
struct TransferSheet: View {
    @Environment(AppModel.self) private var model
    @Bindable var transfer: TableTransfer

    @FocusState private var cancelFocused: Bool

    static let size = CGSize(width: 620, height: 500)

    private var request: RunConfirmation.TransferRequest { transfer.request }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            HStack(alignment: .top, spacing: 10) {
                SideCard(role: "From", side: request.source)
                Image(systemName: "arrow.right")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                    .padding(.top, 34)
                    .accessibilityHidden(true)
                SideCard(role: "To", side: request.target)
            }
            // Both cards as tall as the taller one, so the two ends read as a pair.
            .fixedSize(horizontal: false, vertical: true)
            Text("\(request.mode.statement) \(request.table)")
                .font(.code(12, weight: .semibold))
                .foregroundStyle(request.destructive ? Tone.coral : Tone.ink)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    phaseBody
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(minHeight: 0, maxHeight: .infinity)
            .scrollBounceBehavior(.basedOnSize)

            buttons
        }
        .padding(18)
        .frame(width: Self.size.width, height: Self.size.height)
        .background(Tone.canvas, ignoresSafeAreaEdges: [])
        // A host key the target's bastion refused is asked about over this sheet: a second sheet
        // cannot open over it from the window.
        .hostKeySheet(active: true)
        .onAppear { cancelFocused = transfer.phase == .confirming }
    }

    // MARK: Parts

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: headerSymbol)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(headerTint)
                .accessibilityHidden(true)
            Text(headerTitle)
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
        }
    }

    private var headerTitle: String {
        switch transfer.phase {
        case .confirming: request.title
        case .running: "Copying to \(request.target.name)…"
        case .finished: "Copied to \(request.target.name)"
        case .stopped, .stopUnconfirmed: "Stopped"
        case .failed: "The copy did not finish"
        }
    }

    private var headerSymbol: String {
        switch transfer.phase {
        case .confirming: request.destructive ? "exclamationmark.triangle.fill" : "arrow.left.arrow.right"
        case .running: "arrow.left.arrow.right"
        case .finished: "checkmark.circle.fill"
        case .stopped, .stopUnconfirmed: "stop.circle"
        case .failed: "xmark.octagon.fill"
        }
    }

    private var headerTint: Color {
        switch transfer.phase {
        case .confirming: request.destructive ? Tone.coral : Tone.accent
        case .running: Tone.accent
        case .finished: Tone.mint
        case .stopped, .stopUnconfirmed: Tone.amber
        case .failed: Tone.coral
        }
    }

    @ViewBuilder
    private var phaseBody: some View {
        switch transfer.phase {
        case .confirming:
            Text(request.note)
                .font(.ui(11.5))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if request.needsAcknowledgement {
                TransferBanner(symbol: "exclamationmark.triangle.fill", tint: Tone.coral,
                               text: "\(request.source.name) is labelled Prod and \(request.target.name) is "
                                   + (request.target.environment.map { "labelled \($0.title)." } ?? "not labelled."))
                InlineCheckbox(label: request.acknowledgement, isOn: $transfer.acknowledged)
            }
        case .running:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(progressLine)
                    .font(.ui(12))
                    .foregroundStyle(Tone.ink)
                    .monospacedDigit()
            }
            if let step = stepLine {
                Text(step).font(.ui(11)).foregroundStyle(Tone.secondary)
            }
            columnNotes
        case .finished:
            Label(transfer.summary, systemImage: "checkmark")
                .font(.ui(12))
                .foregroundStyle(Tone.ink)
                .fixedSize(horizontal: false, vertical: true)
            columnNotes
        case .stopped:
            Text(transfer.nothingWritten ?? leftBehind)
                .font(.ui(12))
                .foregroundStyle(Tone.ink)
                .fixedSize(horizontal: false, vertical: true)
            if transfer.rowsRead > 0 {
                Text("\(pluralized(transfer.rowsRead, "row")) had been read from \(request.source.name).")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
            }
            warnings
        case .stopUnconfirmed:
            Text(transfer.stopUnconfirmedLine)
                .font(.ui(12))
                .foregroundStyle(Tone.ink)
                .fixedSize(horizontal: false, vertical: true)
            warnings
        case .failed:
            Text(transfer.failure ?? "The copy failed.")
                .font(.ui(12))
                .foregroundStyle(Tone.coral)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Text(transfer.nothingWritten.map { "\($0) The run's log has the details." } ?? leftBehind)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            warnings
        }
    }

    /// What the sheet says instead of "nothing was written" when the engine reported what it left.
    private var leftBehind: String {
        "The engine reported what it left on \(request.target.name):"
    }

    private var progressLine: String {
        "\(pluralized(transfer.rowsWritten, "row")) written · \(pluralized(transfer.rowsRead, "row")) read"
    }

    private var stepLine: String? {
        AppModel.transferStepLine(transfer.step, source: request.source.name, target: request.target.name)
    }

    /// The columns the target got wider or as text, and the engine's other warnings.
    @ViewBuilder
    private var columnNotes: some View {
        ForEach(Array(transfer.fallbacks.enumerated()), id: \.offset) { _, line in
            TransferBanner(symbol: "textformat", tint: Tone.amber, text: line)
        }
        warnings
    }

    @ViewBuilder
    private var warnings: some View {
        ForEach(Array(transfer.warnings.enumerated()), id: \.offset) { _, line in
            TransferBanner(symbol: "exclamationmark.triangle", tint: Tone.amber, text: line)
        }
    }

    @ViewBuilder
    private var buttons: some View {
        HStack(spacing: 8) {
            Spacer()
            switch transfer.phase {
            case .confirming:
                // Esc cancels and starts focused; nothing is the default action, so a Return still in
                // flight from the editor cannot approve a copy.
                PillButton(title: "Cancel", role: .quiet) { model.dismissTransfer() }
                    .keyboardShortcut(.cancelAction)
                    .focused($cancelFocused)
                PillButton(title: request.confirmTitle, role: request.destructive ? .destructive : .secondary) {
                    model.approveTransfer(transfer)
                }
                .disabled(!transfer.mayApprove)
            case .running:
                PillButton(title: "Hide", role: .quiet) { model.dismissTransfer() }
                    .keyboardShortcut(.cancelAction)
                    .help("Close this window; the copy goes on and its tab keeps reporting")
                PillButton(title: "Stop", symbol: "stop.fill", role: .destructive) {
                    if let tab = model.tabs.first(where: { $0.id == transfer.tabID }) { model.stop(tab) }
                }
            case .finished, .stopped, .failed, .stopUnconfirmed:
                PillButton(title: "Close", role: .secondary) { model.dismissTransfer() }
                    .keyboardShortcut(.cancelAction)
            }
        }
    }
}

/// One end of the copy, with everything the person approving it should see.
private struct SideCard: View {
    let role: String
    let side: RunConfirmation.TransferSide

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            SectionLabel(text: role)
            HStack(spacing: 6) {
                Text(side.name)
                    .font(.ui(12.5, weight: .semibold))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                    .truncationMode(.tail)
                if let environment = side.environment {
                    Chip(text: environment.title, tint: environment == .prod ? Tone.coral : Tone.ice,
                         kind: "Environment")
                }
            }
            line(side.kind)
            line(side.address)
            if !side.database.isEmpty { line(side.database) }
            line("TLS \(side.tls)", tint: side.tlsUnverified ? Tone.amber : Tone.secondary)
            line("Safe Mode \(side.safeMode.title)")
        }
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Tone.ink.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Tone.ink.opacity(0.08)))
        .accessibilityElement(children: .combine)
    }

    private func line(_ text: String, tint: Color = Tone.secondary) -> some View {
        Text(text)
            .font(.code(11))
            .foregroundStyle(tint)
            .lineLimit(1)
            .truncationMode(.middle)
    }
}
