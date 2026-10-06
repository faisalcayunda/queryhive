import SwiftUI

/// The window's half of the engine's `confirm` Safe Mode level: the question the engine cannot ask.
///
/// The engine runs a write on a `confirm` connection only when the run carries
/// `SAFE_MODE_CONFIRMED=1`. The app decides when to ask (`RunConfirmation`), names the statement(s)
/// here, and adds the flag for the approved run alone. There is deliberately no "always allow":
/// one approval is one run, which is the engine's own model of the level, and a switch that
/// remembered would turn `confirm` into `no_ddl` with extra steps.
struct RunConfirmationSheet: View {
    let request: RunConfirmation.Request
    let onApprove: () -> Void
    let onCancel: () -> Void

    @FocusState private var cancelFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: "shield.lefthalf.filled")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Tone.amber)
                Text(request.title)
                    .font(.ui(13, weight: .semibold))
                    .foregroundStyle(Tone.ink)
            }

            Text("About to run:")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)

            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(request.statements.enumerated()), id: \.offset) { _, statement in
                        Text(statement)
                            .font(.code(11.5))
                            .foregroundStyle(Tone.ink)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(10)
            }
            .frame(maxHeight: 260)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            Text(request.note)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 8) {
                Spacer()
                // Esc cancels and starts focused; nothing is the default action, so a Return still in
                // flight from the editor cannot approve a write.
                PillButton(title: "Cancel", role: .quiet, action: onCancel)
                    .keyboardShortcut(.cancelAction)
                    .focused($cancelFocused)
                PillButton(title: request.confirmTitle, role: .destructive, action: onApprove)
            }
        }
        .padding(18)
        .frame(width: 620)
        .onAppear { cancelFocused = true }
    }
}
