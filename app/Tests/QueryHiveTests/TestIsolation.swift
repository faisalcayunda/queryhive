import XCTest

@testable import QueryHive

/// Test isolation for the connections store.
///
/// Two things have to be true, and only the first is optional.
///
/// **The suite must never touch the user's `connections.json`.** `AppModel.init` reads that file,
/// and any mutating action writes it, so a test that constructs a model and then moves a connection
/// is a write to real data. That has already cost one real file, and the app keeps no backup of it.
/// `ConnectionStore.directory()` therefore redirects to a temporary directory whenever the process
/// is a test run, whatever a test does or forgets to do.
///
/// **A test must not inherit another test's file.** That guard alone is per process, and the whole
/// suite shares one process: a class that saves fixtures leaves them for the next class to read, and
/// a test that seeds its own connections then has to fight somebody else's. So every class that
/// builds a model calls `isolateConnectionStore()` in `setUp`, which gives that test its own empty
/// directory.
extension XCTestCase {
    /// Point the connections store at a fresh, empty directory for this test, and remove it after.
    ///
    /// Call from `setUp` **before** anything constructs an `AppModel`.
    func isolateConnectionStore() {
        // Every class that builds a model or a tab goes through here, and a tab holds its rows in a
        // store (`QueryTab.showRows`), so the shared host is configured here too (`TestStores`).
        TestStores.ensureConfigured()
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-test-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        ConnectionStore.root = root
        addTeardownBlock {
            ConnectionStore.root = nil
            try? FileManager.default.removeItem(at: root)
        }
    }

    /// Whether a test may compare what this machine *renders*.
    ///
    /// Some tests here measure pictures: they draw the real AppKit/SwiftUI view tree into a bitmap
    /// and read pixels, or they compare a fresh render to a baseline recorded on the owner's Mac.
    /// Those results pin the machine as much as the app — the font rasteriser, the Xcode that ships
    /// the SDK, the GPU a headless runner falls back to, the traffic-light row a window server draws,
    /// and the system's Accessibility > Display switches (one of which the CI runner leaves on, which
    /// flips the whole surface policy to its "enhanced" colours). On CI a scene differs by ~20% of its
    /// pixels, far past the 0.1% budget, so comparing there tests the runner and not the shell.
    ///
    /// The comparison is therefore the developer's, run where it means something, exactly as the
    /// baselines were recorded. It still runs on any machine that asks for it: set `QH_RENDER=1` (or
    /// the older, per-file `QH_CHROME` / `QH_RECORD_BASELINES`) to force it, e.g. a dedicated snapshot
    /// job or a run against a machine whose rendering matches. Recording never skips; it writes.
    ///
    /// - Returns: `true` when a comparison should run here.
    static var renderComparisonAllowed: Bool {
        let environment = ProcessInfo.processInfo.environment
        if let forced = environment["QH_RENDER"], !forced.isEmpty { return true }
        if let chrome = environment["QH_CHROME"], !chrome.isEmpty { return true }
        if let record = environment["QH_RECORD_BASELINES"], !record.isEmpty { return true }
        if let record = environment["QH_RECORD_CHROME"], !record.isEmpty { return true }
        return (environment["CI"] ?? "").isEmpty
    }

    /// Call at the top of a test that compares rendered pixels, so it skips where that cannot mean
    /// anything rather than failing on the runner's rendering.
    func requireRenderComparison(file: StaticString = #filePath, line: UInt = #line) throws {
        try XCTSkipIf(!Self.renderComparisonAllowed,
                      "rendered pixels are compared on a machine whose rendering matches the baselines; "
                          + "set QH_RENDER=1 to force it here",
                      file: file, line: line)
    }
}
