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
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-test-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        ConnectionStore.root = root
        addTeardownBlock {
            ConnectionStore.root = nil
            try? FileManager.default.removeItem(at: root)
        }
    }
}
