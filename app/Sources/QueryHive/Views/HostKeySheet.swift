import AppKit
import SwiftUI

/// The question the engine cannot ask: is this the server you meant to reach?
///
/// Shown for an SSH bastion whose key nobody has verified (blueprint w11 section 5.4). What it
/// may do is deliberately narrow:
///
/// - **First use** (`unknown`) shows the host, the key type, the full fingerprint in groups, and
///   where the record will be written, and asks the person to compare the fingerprint with a source
///   *outside this connection*. It records nothing until the person presses the one button that says
///   so.
/// - **Everything else** (a changed or revoked key, a certificate, a pin that no longer matches, a
///   store that cannot be trusted) has **no button that accepts**, not even a disabled one. It
///   explains, and for a changed key it shows the command that removes the old record, because
///   making the person do that in a terminal is the point.
///
/// The default button is Cancel, and nothing is the default action: a Return still in flight from the
/// connection sheet cannot trust a key. There is no "always trust" and no "don't ask again".
struct HostKeySheet: View {
    let prompt: HostKeyPrompt
    var center: HostKeyCenter

    @FocusState private var cancelFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if prompt.isTrustable { firstUse } else { refusal }
            footer
        }
        .padding(20)
        .frame(width: 520)
        .background(Tone.canvas)
        .onAppear { cancelFocused = true }
        .onExitCommand { center.dismiss() }
    }

    // MARK: Pieces

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: prompt.isTrustable ? "key.viewfinder" : "exclamationmark.shield")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(prompt.isTrustable ? Tone.accent : Tone.coral)
            VStack(alignment: .leading, spacing: 2) {
                Text(prompt.isTrustable ? "Verify the server's identity" : "This server can't be trusted")
                    .font(.ui(15, weight: .bold, rounded: true))
                Text(target)
                    .font(.code(12))
                    .foregroundStyle(Tone.secondary)
                    .textSelection(.enabled)
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// `bastion.corp:22`, with the alias in front when the connection named one.
    private var target: String {
        let address = "\(prompt.host):\(prompt.port)"
        guard let alias = prompt.detail.alias, !alias.isEmpty, alias != prompt.host else { return address }
        return "\(alias) · \(address)"
    }

    @ViewBuilder private var firstUse: some View {
        Text("You are connecting to this server for the first time. QueryHive has no record of its key.")
            .font(.ui(12))
            .fixedSize(horizontal: false, vertical: true)

        VStack(alignment: .leading, spacing: 8) {
            row("Key type", prompt.detail.keyType ?? "unknown")
            VStack(alignment: .leading, spacing: 3) {
                Text("FINGERPRINT")
                    .font(.ui(11, weight: .semibold)).tracking(0.8)
                    .foregroundStyle(Tone.secondary)
                Text(prompt.groupedFingerprint)
                    .font(.code(13))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    // Read in groups of four, the way it is compared, not as one unbroken word.
                    .accessibilityLabel("Fingerprint, in groups of four: "
                        + prompt.groupedFingerprint.replacingOccurrences(of: " ", with: ", "))
            }
            row("Recorded in", prompt.detail.appKnownHosts ?? "QueryHive's known_hosts file")
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

        Text("Compare this fingerprint with one you got from outside this connection: ask the server's "
             + "administrator, or run ssh-keygen -lf /etc/ssh/ssh_host_*_key.pub on that server. "
             + "A fingerprint fetched from this computer's own network proves nothing.")
            .font(.ui(11.5))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)

        if case .trustedButFailed(let message) = center.status {
            Text("Host key trusted, but the connection failed: \(message)")
                .font(.ui(11.5)).foregroundStyle(Tone.amber)
                .fixedSize(horizontal: false, vertical: true)
        }
        if case .refused(let detail) = center.status {
            // The pinned attempt was itself refused; say so, with no way to accept.
            Text(HostKeyPrompt(detail: detail).explanation)
                .font(.ui(11.5)).foregroundStyle(Tone.coral)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder private var refusal: some View {
        Text(prompt.explanation)
            .font(.ui(12))
            .fixedSize(horizontal: false, vertical: true)
        if let fingerprint = prompt.fingerprint {
            row("Presented", "\(prompt.detail.keyType ?? "key") \(fingerprint)")
        }
        ForEach(Array((prompt.detail.recorded ?? []).enumerated()), id: \.offset) { _, record in
            row("On record", [record.keyType, record.fingerprint, record.path.map { "in \($0)" + (record.line.map { ", line \($0)" } ?? "") }]
                .compactMap { $0 }.joined(separator: " "))
        }
        let advice = prompt.removalAdvice
        if !advice.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("To forget the old record on purpose, run this in a terminal and connect again:")
                    .font(.ui(11.5)).foregroundStyle(Tone.secondary)
                ForEach(advice, id: \.self) { line in
                    Text(line)
                        .font(line.hasPrefix("ssh-keygen -R") ? .code(11.5) : .ui(11.5))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label.uppercased())
                .font(.ui(11, weight: .semibold)).tracking(0.8)
                .foregroundStyle(Tone.secondary)
                .frame(width: 84, alignment: .leading)
            Text(value)
                .font(.code(11.5))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            if case .trusting = center.status { ProgressView().controlSize(.small) }
            Spacer()
            if prompt.isTrustable, !isTrustedButFailed {
                PillButton(title: "Cancel", role: .quiet) { center.dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .focused($cancelFocused)
                // Not the default action and no key of its own: pressing it is the whole decision.
                PillButton(title: "Trust and connect to \(prompt.host)", role: .secondary) { center.trust(prompt) }
                    .disabled(center.status != .asking)
                    .accessibilityLabel("Trust and connect to \(prompt.host)")
            } else {
                PillButton(title: "Close", role: .quiet) { center.dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .focused($cancelFocused)
            }
        }
    }

    private var isTrustedButFailed: Bool {
        if case .trustedButFailed = center.status { return true }
        return false
    }
}

/// Presents `HostKeyCenter`'s prompt as a sheet from whatever it is attached to.
///
/// Attached twice: once to the window's root, active while no connection sheet is up, and once to the
/// connection sheet itself, because a sheet cannot be presented over a sheet from the view that
/// presented the first one. Exactly one of the two is `active` at any moment, so one prompt is one
/// sheet. Also forwards "a host key was trusted" so the tree's failed nodes load again.
struct HostKeyPresenter: ViewModifier {
    @Environment(AppModel.self) private var model
    var center: HostKeyCenter = .shared
    /// Whether this attachment is the one that should show the sheet right now.
    var active: Bool

    func body(content: Content) -> some View {
        content
            .sheet(item: Binding(
                get: { active ? center.prompt : nil },
                set: { if $0 == nil, active { center.dismiss() } }
            )) { prompt in
                HostKeySheet(prompt: prompt, center: center)
            }
            // A trusted key is what the tree's failed nodes were waiting for.
            .onChange(of: center.trustedCount) { _, _ in
                if active { model.reloadAfterHostKeyTrusted() }
            }
    }
}

extension View {
    func hostKeySheet(active: Bool = true) -> some View { modifier(HostKeyPresenter(active: active)) }
}
