import XCTest

@testable import QueryHive

@MainActor
private final class FakePresenter: NotificationPresenting {
    var onSelect: ((UUID) -> Void)?
    var status: NotificationAuthorization
    /// What the person answers when asked.
    var grants: Bool
    var postError: Error?
    var onRequest: () -> Void = {}
    private(set) var requests = 0
    private(set) var posts: [(identifier: String, title: String, body: String, tab: UUID)] = []

    init(status: NotificationAuthorization, grants: Bool = true) {
        self.status = status
        self.grants = grants
    }

    func authorizationStatus() async -> NotificationAuthorization { status }

    func requestAuthorization() async -> Bool {
        requests += 1
        onRequest()
        // A real prompt takes a while; a second finisher arrives meanwhile.
        await Task.yield()
        status = grants ? .authorized : .denied
        return grants
    }

    func post(identifier: String, title: String, body: String, tabID: UUID) async throws {
        if let postError { throw postError }
        posts.append((identifier, title, body, tabID))
    }
}

@MainActor
private final class FakeAnnouncer: RunAnnouncing {
    private(set) var spoken: [String] = []
    func announce(_ text: String) { spoken.append(text) }
}

@MainActor
private final class Rig {
    let presenter: FakePresenter
    let announcer = FakeAnnouncer()
    var appActive = false
    private(set) var attentions = 0
    private(set) var notifier: LongRunNotifier!

    init(status: NotificationAuthorization = .authorized, grants: Bool = true) {
        presenter = FakePresenter(status: status, grants: grants)
        notifier = LongRunNotifier(presenter: presenter, announcer: announcer,
                                   isAppActive: { [unowned self] in appActive },
                                   attention: { [unowned self] in attentions += 1 })
    }
}

@MainActor
final class LongRunNotifierTests: XCTestCase {
    private let tab = UUID()

    private func report(_ kind: LongRunKind = .export, _ outcome: LongRunOutcome = .done,
                        elapsed: TimeInterval = 21, rows: Int? = 1_203,
                        failure: String? = nil) -> LongRunReport {
        LongRunReport(kind: kind, outcome: outcome, elapsed: elapsed, rows: rows, failure: failure)
    }

    // MARK: Policy

    func testNineteenSecondsDoesNotNotify() async {
        let rig = Rig()
        let delivery = await rig.notifier.finished(report(elapsed: 19), tab: tab)
        XCTAssertEqual(delivery, .none)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
    }

    func testExactlyTheThresholdDoesNotNotify() async {
        let rig = Rig()
        await rig.notifier.finished(report(elapsed: 20), tab: tab)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
    }

    func testTwentyOneSecondsInTheBackgroundNotifies() async {
        let rig = Rig()
        let delivery = await rig.notifier.finished(report(elapsed: 21), tab: tab)
        XCTAssertEqual(delivery, .posted)
        XCTAssertEqual(rig.presenter.posts.count, 1)
        XCTAssertEqual(rig.presenter.posts.first?.tab, tab)
        XCTAssertEqual(rig.presenter.posts.first?.title, "Export finished")
        XCTAssertEqual(rig.attentions, 0)
    }

    func testAFrontmostAppDoesNotNotifyButStillAnnounces() async {
        let rig = Rig()
        rig.appActive = true
        let delivery = await rig.notifier.finished(report(elapsed: 90), tab: tab)
        XCTAssertEqual(delivery, .none)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
        XCTAssertEqual(rig.announcer.spoken.count, 1)
    }

    func testAStoppedRunNeverNotifiesAndSaysStopped() async {
        let rig = Rig()
        let delivery = await rig.notifier.finished(report(.query, .cancelled, elapsed: 300), tab: tab)
        XCTAssertEqual(delivery, .none)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
        XCTAssertEqual(rig.announcer.spoken, ["Query stopped"])
    }

    func testAFailureNotifiesToo() async {
        let rig = Rig()
        await rig.notifier.finished(report(.toTable, .failed, elapsed: 40, rows: nil,
                                           failure: "boom"), tab: tab)
        XCTAssertEqual(rig.presenter.posts.first?.title, "Export to table failed")
    }

    func testReturningToTheAppDuringThePermissionPromptSuppressesIt() async {
        let rig = Rig(status: .notDetermined)
        rig.presenter.onRequest = { [unowned rig] in rig.appActive = true }
        let delivery = await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(delivery, .none)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
    }

    // MARK: Permission

    func testNothingIsRequestedUntilANotificationIsDue() async {
        let rig = Rig(status: .notDetermined)
        XCTAssertEqual(rig.presenter.requests, 0)
        await rig.notifier.finished(report(elapsed: 5), tab: tab)
        rig.appActive = true
        await rig.notifier.finished(report(elapsed: 50), tab: tab)
        XCTAssertEqual(rig.presenter.requests, 0, "a short run and a frontmost app never ask")
    }

    func testTheFirstDueNotificationAsksOnceAndThenPosts() async {
        let rig = Rig(status: .notDetermined, grants: true)
        let delivery = await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(delivery, .posted)
        XCTAssertEqual(rig.presenter.requests, 1)
    }

    func testADeniedPermissionFallsToAttentionOnce() async {
        let rig = Rig(status: .denied)
        let delivery = await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(delivery, .attention)
        XCTAssertEqual(rig.attentions, 1)
        XCTAssertTrue(rig.presenter.posts.isEmpty)
        XCTAssertEqual(rig.presenter.requests, 0, "a denial is final; the app does not ask again")
    }

    func testAnAnswerOfNoAtThePromptFallsToAttention() async {
        let rig = Rig(status: .notDetermined, grants: false)
        let delivery = await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(delivery, .attention)
        XCTAssertEqual(rig.attentions, 1)
    }

    func testTwoNotificationsInARowAskOnce() async {
        let rig = Rig(status: .notDetermined, grants: false)
        await rig.notifier.finished(report(), tab: tab)
        await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(rig.presenter.requests, 1)
        XCTAssertEqual(rig.attentions, 2)
    }

    func testTwoNotificationsFinishingTogetherShareOneRequest() async {
        let rig = Rig(status: .notDetermined, grants: true)
        async let first = rig.notifier.finished(report(), tab: tab)
        async let second = rig.notifier.finished(report(), tab: UUID())
        let deliveries = await [first, second]
        XCTAssertEqual(deliveries, [.posted, .posted])
        XCTAssertEqual(rig.presenter.requests, 1)
        XCTAssertEqual(rig.presenter.posts.count, 2)
    }

    func testAPostThatFailsFallsToAttention() async {
        struct Refused: Error {}
        let rig = Rig()
        rig.presenter.postError = Refused()
        let delivery = await rig.notifier.finished(report(), tab: tab)
        XCTAssertEqual(delivery, .attention)
        XCTAssertEqual(rig.attentions, 1)
    }

    func testTheSystemCenterIsNotUsedInATestProcess() {
        XCTAssertFalse(LongRunNotifier.systemCenterAvailable)
    }

    // MARK: Wording

    func testNothingFromTheStatementReachesTheNotification() async {
        let rig = Rig()
        let leak = #"ERROR: relation "salaries" does not exist: SELECT card FROM salaries WHERE card = '4111'"#
        await rig.notifier.finished(report(.query, .failed, elapsed: 61, rows: nil, failure: leak),
                                    tab: tab)
        let post = rig.presenter.posts.first
        XCTAssertNotNil(post)
        let text = "\(post?.title ?? "") \(post?.body ?? "")"
        for fragment in ["SELECT", "salaries", "4111", "relation", "ERROR"] {
            XCTAssertFalse(text.contains(fragment), "the notification must not carry \(fragment)")
        }
        XCTAssertEqual(post?.title, "Query failed")
    }

    func testADoneBodyHasTheCountAndTheDuration() async {
        let rig = Rig()
        await rig.notifier.finished(report(elapsed: 65, rows: 1_203), tab: tab)
        XCTAssertEqual(rig.presenter.posts.first?.body.hasPrefix(pluralized(1_203, "row")), true)
        XCTAssertTrue(rig.presenter.posts.first?.body.contains("1") == true)
    }

    func testAnnouncements() {
        XCTAssertEqual(LongRunNotifier.announcement(for: report(.query, .done, rows: 1_203)),
                       "Query finished, \(pluralized(1_203, "row"))")
        XCTAssertEqual(LongRunNotifier.announcement(for: report(.query, .done, rows: 1)),
                       "Query finished, 1 row")
        XCTAssertEqual(LongRunNotifier.announcement(for: report(.script, .done, rows: nil)),
                       "Script finished")
        XCTAssertEqual(LongRunNotifier.announcement(for: report(.export, .cancelled)), "Export stopped")
    }

    func testAFailureAnnouncementKeepsTheFirstEightyCharacters() {
        let message = "first line\nsecond   line " + String(repeating: "x", count: 200)
        let spoken = LongRunNotifier.announcement(for: report(.query, .failed, failure: message))
        XCTAssertTrue(spoken.hasPrefix("Query failed: first line second line x"))
        XCTAssertEqual(spoken.count, "Query failed: ".count + 80)
        XCTAssertEqual(LongRunNotifier.announcement(for: report(.query, .failed, failure: "  ")),
                       "Query failed")
    }

    func testEveryRunIsAnnouncedWhateverTheOutcome() async {
        let rig = Rig()
        rig.appActive = true
        await rig.notifier.finished(report(.query, .done, elapsed: 1), tab: tab)
        await rig.notifier.finished(report(.query, .failed, elapsed: 1, failure: "no"), tab: tab)
        await rig.notifier.finished(report(.query, .cancelled, elapsed: 1), tab: tab)
        XCTAssertEqual(rig.announcer.spoken.count, 3)
    }

    // MARK: The model's hook

    func testAnAttentionFallbackIsLoggedInTheTab() async throws {
        let rig = Rig(status: .denied)
        let saved = AppModel.longRunNotifier
        AppModel.longRunNotifier = rig.notifier
        addTeardownBlock { @MainActor in AppModel.longRunNotifier = saved }
        isolateConnectionStore()
        let model = AppModel()
        let tab = QueryTab(title: "Query 1")
        model.reportLongRun(.export, .done, tab: tab, elapsed: 30, rows: 10)
        for _ in 0..<200 where !tab.logLines.contains(where: { $0.text.contains("Notifications are off") }) {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("Notifications are off") })
        XCTAssertEqual(rig.attentions, 1)
    }

    func testAClickSelectsTheTab() async throws {
        let rig = Rig()
        isolateConnectionStore()
        let saved = AppModel.longRunNotifier
        AppModel.longRunNotifier = rig.notifier
        addTeardownBlock { @MainActor in AppModel.longRunNotifier = saved }
        let model = AppModel()
        let tab = QueryTab(title: "Query 1")
        model.tabs.append(tab)
        model.reportLongRun(.export, .done, tab: tab, elapsed: 30, rows: 10)
        for _ in 0..<200 where rig.presenter.posts.isEmpty {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertEqual(rig.presenter.posts.count, 1)
        rig.presenter.onSelect?(tab.id)
        XCTAssertEqual(model.selectedTabID, tab.id)
    }
}
