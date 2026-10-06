import AppKit
import SwiftUI
import XCTest
@testable import QueryHive

final class ErrorBannerTests: XCTestCase {
    /// Banner and sheet are plain views over closures, so the contract is checked on the sources of
    /// truth: the closures fire, and the sheet declares Esc as cancel and no default action.
    @MainActor
    func testBannerActionsFireAndTheSheetHasNoDefaultAction() throws {
        var copied = 0, logged = 0, dismissed = 0
        let banner = ErrorBanner(message: "boom", onCopy: { copied += 1 },
                                 onShowLog: { logged += 1 }, onDismiss: { dismissed += 1 })
        banner.onCopy(); banner.onShowLog(); banner.onDismiss()
        XCTAssertEqual([copied, logged, dismissed], [1, 1, 1])

        let source = try String(contentsOfFile: #filePath.replacingOccurrences(
            of: "Tests/QueryHiveTests/ErrorBannerTests.swift",
            with: "Sources/QueryHive/Views/RunConfirmationSheet.swift"), encoding: .utf8)
        XCTAssertTrue(source.contains(".keyboardShortcut(.cancelAction)"))
        XCTAssertFalse(source.contains(".defaultAction"))
        let banners = try String(contentsOfFile: #filePath.replacingOccurrences(
            of: "Tests/QueryHiveTests/ErrorBannerTests.swift",
            with: "Sources/QueryHive/Views/ResultGrid.swift"), encoding: .utf8)
        XCTAssertTrue(banners.contains(".textSelection(.enabled)"))
    }
}
