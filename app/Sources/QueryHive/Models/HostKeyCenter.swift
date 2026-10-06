import Foundation
import Observation

/// What the host-key sheet is about, built from one `error` event and nothing else.
///
/// There is no initializer that takes a fingerprint as text: `HostKeyCenter.trust` sends the one
/// this value was built with, which came from the engine's own event for the attempt the sheet
/// describes. A fingerprint that was typed, pasted, cached from an earlier attempt or computed here
/// has no way in (blueprint w11 section 5.4 item 2).
struct HostKeyPrompt: Identifiable, Equatable {
    let id = UUID()
    let detail: Event.HostKeyDetail

    var host: String { detail.host }
    var port: Int { detail.port ?? 22 }
    var state: String { detail.state }
    var fingerprint: String? { detail.fingerprint }

    /// Whether the person can be asked at all. Only an **unknown** host is: a changed, revoked,
    /// certificate-covered, mismatched-pin or unwritable-store host is refused outright, and its
    /// sheet has no way to accept (section 5.4 item 7). A host that `known_hosts` says should
    /// present a certificate (`ca_covered`) is refused too, whatever the state says.
    var isTrustable: Bool {
        state == "unknown" && detail.caCovered != true && !(fingerprint ?? "").isEmpty
    }

    /// `host` for port 22, `[host]:port` otherwise: how a `known_hosts` line spells the name.
    var hostSpelling: String { port == 22 ? host : "[\(host)]:\(port)" }

    /// The fingerprint in groups of four, for reading aloud and for comparing by eye. The copy
    /// button and the pin use the plain text.
    var groupedFingerprint: String {
        guard let fingerprint else { return "" }
        let body = fingerprint.hasPrefix("SHA256:") ? String(fingerprint.dropFirst("SHA256:".count)) : fingerprint
        let groups = stride(from: 0, to: body.count, by: 4).map { start -> String in
            let from = body.index(body.startIndex, offsetBy: start)
            let to = body.index(from, offsetBy: min(4, body.count - start))
            return String(body[from..<to])
        }
        return "SHA256: " + groups.joined(separator: " ")
    }
}

/// The one place a refused SSH host key becomes something a person decides about.
///
/// `HostKeyGate` reports every `error` event that carries a `host_key` object; this holds the
/// sheet's state, coalesces events for the same host and key, and runs the one thing the sheet can
/// do: pin the fingerprint for a single `test` run.
///
/// **What it never does.** It has no method that takes a fingerprint; it never trusts a state
/// other than `unknown`; it never writes `known_hosts` (the engine does, before anything is sent
/// to the bastion, and only for a host that is genuinely new); and nothing here is stored beyond
/// the sheet's life. The environment of the run that failed carries the connection's secrets, so it
/// is kept in memory only, for at most five minutes, and dropped when the sheet closes.
///
/// Used on the main queue only: the gate hops there before it reports, and the sheet's buttons are
/// already there.
@Observable
final class HostKeyCenter {
    /// The shared center the app runs on. Tests make their own.
    static let shared = HostKeyCenter()

    /// What the sheet shows below the fingerprint.
    enum Status: Equatable {
        /// Waiting for the person. For a refusal there is nothing to wait for.
        case asking
        case trusting
        /// The key is recorded and the connection still failed: said in those words, because the
        /// key being recorded is true and is not undone.
        case trustedButFailed(String)
        /// The pinned attempt was itself refused; the sheet now shows why, with no way to accept.
        case refused(Event.HostKeyDetail)
    }

    /// The sheet that is up, if any. One at a time.
    private(set) var prompt: HostKeyPrompt?
    private(set) var status = Status.asking
    /// Counts successful trusts, so anything that failed because of the key can run again.
    private(set) var trustedCount = 0
    private(set) var lastTrusted: (host: String, port: Int)?

    /// How long a failed run's environment may wait for the person: past it, Trust asks for a new
    /// attempt instead of reusing secrets that have sat in memory.
    static let environmentLifetime: TimeInterval = 5 * 60

    @ObservationIgnored var engine: (any DatabaseEngine)?
    @ObservationIgnored private let now: () -> Date
    @ObservationIgnored private var pending: (env: [String: String], expires: Date)?
    @ObservationIgnored private var expiry: DispatchWorkItem?

    init(engine: (any DatabaseEngine)? = nil, now: @escaping () -> Date = Date.init) {
        self.engine = engine
        self.now = now
    }

    /// An `error` event with a `host_key` object arrived from a run whose environment was `env`.
    ///
    /// A second event for the same host, port and fingerprint while the sheet is up (the tree and a
    /// Run racing each other) opens nothing; an event for a different key while a sheet is up waits
    /// for the person to try again, because there is one sheet.
    func report(_ detail: Event.HostKeyDetail, env: [String: String]) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard prompt == nil else { return }
        prompt = HostKeyPrompt(detail: detail)
        status = .asking
        pending = (env, now().addingTimeInterval(Self.environmentLifetime))
        scheduleExpiry()
    }

    /// Pin the fingerprint this prompt was built with for one `test` run.
    ///
    /// Refused unless `prompt` is the sheet that is up, is trustable, and the environment is still
    /// fresh. The run gets `SSH_HOST_KEY_ACCEPT` and `SSH_HOST_KEY_DETAIL` and nothing else added;
    /// the engine records the key before it authenticates and accepts only an *unknown* host whose
    /// key has exactly this fingerprint.
    func trust(_ prompt: HostKeyPrompt) {
        dispatchPrecondition(condition: .onQueue(.main))
        guard prompt == self.prompt, prompt.isTrustable, let fingerprint = prompt.fingerprint,
              status == .asking, let engine, let pending, pending.expires > now()
        else { return }
        status = .trusting
        var env = pending.env
        env["SSH_HOST_KEY_ACCEPT"] = fingerprint
        env["SSH_HOST_KEY_DETAIL"] = "1"
        var refusal: Event.HostKeyDetail?
        var message: String?
        engine.run("test", env: env, onEvent: { event in
            guard event.event == "error" else { return }
            refusal = event.hostKey
            message = event.message
        }, onExit: { [weak self] exit, stderr in
            guard let self, self.prompt == prompt else { return }
            if let refusal {
                // Refused again (the key changed between the prompt and now, or the file could not
                // be written): show that, with no button that accepts.
                self.status = .refused(refusal)
            } else if exit == 0 {
                self.trusted(prompt)
            } else {
                // The key check passed and was recorded; something else failed.
                self.status = .trustedButFailed(message ?? (stderr.isEmpty ? "exit status \(exit)" : stderr))
                self.lastTrusted = (prompt.host, prompt.port)
                self.trustedCount += 1
            }
        })
    }

    /// Closes the sheet and forgets the environment.
    func dismiss() {
        dispatchPrecondition(condition: .onQueue(.main))
        prompt = nil
        status = .asking
        forgetEnvironment()
    }

    private func trusted(_ prompt: HostKeyPrompt) {
        lastTrusted = (prompt.host, prompt.port)
        trustedCount += 1
        dismiss()
    }

    private func scheduleExpiry() {
        expiry?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.pending = nil }
        expiry = work
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.environmentLifetime, execute: work)
    }

    private func forgetEnvironment() {
        pending = nil
        expiry?.cancel()
        expiry = nil
    }

    /// Whether a failed run's environment is still held (a test reads this to prove it is dropped).
    var holdsEnvironment: Bool { pending != nil }
}

extension HostKeyPrompt {
    /// What the sheet says about a refusal, and what the person can do about it. Words, not
    /// behaviour: no state here has an accept path.
    var explanation: String {
        switch state {
        case "changed":
            return "The server at \(hostSpelling) is presenting a different key than the one on record. "
                + "That can mean the server was rebuilt or its key rotated, or that something between you and it is pretending to be it. "
                + "QueryHive will not connect until the old record is removed on purpose."
        case "revoked":
            return "The key this server presents is marked as revoked in a known_hosts file. QueryHive will not connect to it."
        case "certificate":
            return "The server offered a host certificate, which this build can't verify, so it was refused."
        case "certificate_expected":
            return "Your own known_hosts says \(hostSpelling) should present a certificate signed by a certificate authority, "
                + "but it presented a plain key, and this build can't verify certificates. A plain key for a host "
                + "managed that way is what an attacker on the network would present, so QueryHive refuses. "
                + "If the host is no longer managed by a CA, edit the @cert-authority line in known_hosts yourself."
        case "pin_mismatch":
            return "The key the server presented just now is not the one that was shown for confirmation, so nothing was recorded. "
                + "Start again and compare the new fingerprint."
        case "record_failed":
            return "The key could not be written to QueryHive's known_hosts file, so it was not trusted. "
                + (detail.appKnownHosts.map { "Check that \($0) can be written." } ?? "")
        case "store_unsafe":
            return "QueryHive's known_hosts file is not safe to rely on (a symbolic link, the wrong owner, or writable by others), "
                + "so no host key was trusted. Fix its owner and permissions, for example chmod 600."
        default:
            return "This host key can't be trusted."
        }
    }

    /// The command that removes the record a `changed` host clashes with, per recorded key whose
    /// file the person owns. The host and the path are quoted for a POSIX shell: either can come
    /// from a Navicat export or an ssh config, and the person pastes this into a terminal.
    ///
    /// A record in the system file has no command: only an administrator changes it. `ssh-keygen -R`
    /// keeps a `.old` copy beside the file, which the sheet says.
    var removalAdvice: [String] {
        guard state == "changed" else { return [] }
        var lines: [String] = []
        var seen = Set<String>()
        for record in detail.recorded ?? [] {
            switch record.source {
            case "system":
                lines.append("A record in \(record.path ?? "/etc/ssh/ssh_known_hosts") belongs to your administrator; only they can change it.")
            default:
                guard let path = record.path ?? detail.appKnownHosts, seen.insert(path).inserted else { continue }
                lines.append("ssh-keygen -R \(Self.shellQuote(hostSpelling)) -f \(Self.shellQuote(path))")
            }
        }
        if lines.contains(where: { $0.hasPrefix("ssh-keygen") }) {
            lines.append("ssh-keygen keeps a copy of the file as known_hosts.old beside it.")
        }
        return lines
    }

    /// POSIX single-quote quoting: `'` becomes `'\''`, so the result is one word whatever the value
    /// holds (spaces, `;`, `$(…)`, a leading `-`).
    static func shellQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
