import AppKit
import UserNotifications

/// What kind of run finished. The label is what the announcement and the notification title say.
enum LongRunKind {
    case query, export, toTable, script

    var label: String {
        switch self {
        case .query: "Query"
        case .export: "Export"
        case .toTable: "Export to table"
        case .script: "Script"
        }
    }
}

enum LongRunOutcome { case done, failed, cancelled }

enum NotificationAuthorization { case notDetermined, denied, authorized }

/// What `LongRunNotifier.finished` did, so the caller can say so in the tab's log.
enum LongRunDelivery {
    /// Nothing visible beyond the announcement: a short run, a frontmost app, or a Stop.
    case none
    case posted
    /// No notification could be shown (permission off, or the post failed), so the Dock icon
    /// bounced instead.
    case attention
}

/// Counts and durations only. Nothing here may carry SQL, a table or connection name, or a value,
/// because a notification can be read on a locked screen (FR-RUN-04, D-22).
struct LongRunReport {
    var kind: LongRunKind
    var outcome: LongRunOutcome
    var elapsed: TimeInterval
    /// Rows the run returned or wrote.
    var rows: Int?
    /// The engine's message for a failure. It is spoken (first 80 characters) and never put into
    /// a notification: it can quote the statement, a table or a value.
    var failure: String?
}

/// The seam in front of `UNUserNotificationCenter`, so the policy runs in tests without a bundle.
@MainActor
protocol NotificationPresenting: AnyObject {
    /// Called with the id of the tab whose notification was clicked.
    var onSelect: ((UUID) -> Void)? { get set }
    func authorizationStatus() async -> NotificationAuthorization
    func requestAuthorization() async -> Bool
    func post(identifier: String, title: String, body: String, tabID: UUID) async throws
}

@MainActor
protocol RunAnnouncing {
    func announce(_ text: String)
}

/// Tells the person a long run is over, in the places they will look: a VoiceOver announcement for
/// every run, and a system notification for a long one that finished while the app was in the
/// background (blueprint w12 §8, D-22).
///
/// This type is the policy. It never touches `UNUserNotificationCenter` itself, so nothing here can
/// throw in a process without an app bundle, and permission is requested only when the first
/// notification is about to be shown, never at launch.
@MainActor
final class LongRunNotifier {
    /// A run shorter than this never notifies; the person is still looking at the app.
    static let threshold: TimeInterval = 20

    private let presenter: any NotificationPresenting
    private let announcer: any RunAnnouncing
    private let isAppActive: () -> Bool
    private let attention: () -> Void
    /// The one permission request. Kept after it resolves, so a second notification, or two that
    /// finish together, never ask again within this launch.
    private var permission: Task<Bool, Never>?

    init(presenter: any NotificationPresenting, announcer: any RunAnnouncing,
         isAppActive: @escaping () -> Bool, attention: @escaping () -> Void) {
        self.presenter = presenter
        self.announcer = announcer
        self.isAppActive = isAppActive
        self.attention = attention
    }

    /// What a click on a notification does with the tab it came from.
    var onSelectTab: ((UUID) -> Void)? {
        get { presenter.onSelect }
        set { presenter.onSelect = newValue }
    }

    /// Announces the end of a run, and notifies when the policy says so.
    @discardableResult
    func finished(_ report: LongRunReport, tab: UUID) async -> LongRunDelivery {
        announcer.announce(Self.announcement(for: report))
        guard Self.notifies(report, appActive: isAppActive()) else { return .none }
        guard await isAuthorized() else {
            attention()
            return .attention
        }
        // The permission prompt can take a while, and the person may have come back to the app.
        guard !isAppActive() else { return .none }
        let content = Self.content(for: report)
        do {
            try await presenter.post(identifier: "longrun-\(tab.uuidString)", title: content.title,
                                     body: content.body, tabID: tab)
            return .posted
        } catch {
            attention()
            return .attention
        }
    }

    private func isAuthorized() async -> Bool {
        switch await presenter.authorizationStatus() {
        case .authorized: return true
        case .denied: return false
        case .notDetermined:
            // No await between the check and the assignment, so two finishers share one request.
            if permission == nil {
                permission = Task { await presenter.requestAuthorization() }
            }
            return await permission?.value ?? false
        }
    }

    // MARK: Policy and wording

    /// Longer than the threshold, finished with the app in the background, and not stopped by the
    /// person: pressing Stop means they are in the app.
    static func notifies(_ report: LongRunReport, appActive: Bool) -> Bool {
        report.outcome != .cancelled && report.elapsed > threshold && !appActive
    }

    static func announcement(for report: LongRunReport) -> String {
        let label = report.kind.label
        switch report.outcome {
        case .done:
            return report.rows.map { "\(label) finished, \(pluralized($0, "row"))" } ?? "\(label) finished"
        case .failed:
            let message = (report.failure ?? "").split(whereSeparator: \.isWhitespace).joined(separator: " ")
            return message.isEmpty ? "\(label) failed" : "\(label) failed: \(message.prefix(80))"
        case .cancelled:
            return "\(label) stopped"
        }
    }

    /// Built from the kind, the outcome, the count and the duration alone, so there is no path by
    /// which a statement or a name reaches it.
    static func content(for report: LongRunReport) -> (title: String, body: String) {
        let label = report.kind.label
        let took = Duration.seconds(Int(report.elapsed.rounded()))
            .formatted(.units(allowed: [.hours, .minutes, .seconds], width: .abbreviated))
        switch report.outcome {
        case .done:
            let body = report.rows.map { "\(pluralized($0, "row")) in \(took)" } ?? "Took \(took)"
            return ("\(label) finished", body)
        case .failed:
            return ("\(label) failed", "Failed after \(took)")
        case .cancelled:
            return ("\(label) stopped", "Stopped after \(took)")
        }
    }
}

// MARK: - The real thing

/// VoiceOver reads a priority announcement posted on the application element, the same place the
/// grid's selection announcement goes.
struct SystemAnnouncer: RunAnnouncing {
    func announce(_ text: String) {
        guard let app = NSApp else { return }
        NSAccessibility.post(element: app, notification: .announcementRequested,
                             userInfo: [.announcement: text,
                                        .priority: NSAccessibilityPriorityLevel.high.rawValue])
    }
}

/// `UNUserNotificationCenter` for a signed app bundle. Built only by `LongRunNotifier.system()`,
/// and the center itself is touched on first use, never at launch.
@MainActor
final class SystemNotificationPresenter: NSObject, NotificationPresenting {
    var onSelect: ((UUID) -> Void)?

    /// `current()` raises in a process without a proper bundle, so it waits for the first call.
    private lazy var center: UNUserNotificationCenter = {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        return center
    }()

    func authorizationStatus() async -> NotificationAuthorization {
        switch await center.notificationSettings().authorizationStatus {
        case .notDetermined: .notDetermined
        case .denied: .denied
        default: .authorized
        }
    }

    func requestAuthorization() async -> Bool {
        (try? await center.requestAuthorization(options: [.alert])) ?? false
    }

    func post(identifier: String, title: String, body: String, tabID: UUID) async throws {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = ["tab": tabID.uuidString]
        try await center.add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil))
    }
}

extension SystemNotificationPresenter: UNUserNotificationCenterDelegate {
    /// A click brings the app forward and selects the tab. One that arrives after the app was
    /// quit and relaunched finds no handler yet and only activates (R-11).
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let tab = (response.notification.request.content.userInfo["tab"] as? String)
            .flatMap(UUID.init(uuidString:))
        completionHandler()
        Task { @MainActor in
            NSApp?.activate()
            if let tab { self.onSelect?(tab) }
        }
    }
}

/// Nothing to talk to: a test, `--snapshot` or `--bench` process, or a binary run outside an app
/// bundle. The status says "off", so a caller falls to `attention`, which is silent there too.
@MainActor
private final class UnavailablePresenter: NotificationPresenting {
    var onSelect: ((UUID) -> Void)?
    func authorizationStatus() async -> NotificationAuthorization { .denied }
    func requestAuthorization() async -> Bool { false }
    func post(identifier: String, title: String, body: String, tabID: UUID) async throws {}
}

extension LongRunNotifier {
    /// Whether this process may create the system notification center at all.
    static var systemCenterAvailable: Bool {
        NSClassFromString("XCTestCase") == nil
            && Bundle.main.bundleIdentifier != nil
            && Bundle.main.bundleURL.pathExtension == "app"
            && Snapshot.requestedPath() == nil
            && !CommandLine.arguments.contains("--bench")
    }

    /// The notifier the app uses. The real presenter exists only where the center can be used.
    static func system() -> LongRunNotifier {
        guard systemCenterAvailable else {
            return LongRunNotifier(presenter: UnavailablePresenter(), announcer: SystemAnnouncer(),
                                   isAppActive: { NSApp?.isActive ?? true }, attention: {})
        }
        return LongRunNotifier(presenter: SystemNotificationPresenter(), announcer: SystemAnnouncer(),
                               isAppActive: { NSApp?.isActive ?? true },
                               attention: { _ = NSApp?.requestUserAttention(.informationalRequest) })
    }
}
