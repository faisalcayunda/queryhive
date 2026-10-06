import AppKit

/// Spoken feedback for changes a sighted user sees without being told: a tab switch, a panel
/// opening, a failed query.
/// Call from the main thread.
enum Announcer {
    /// The latest message, kept so a test can read it. Recorded whether or not VoiceOver is on.
    nonisolated(unsafe) private(set) static var last: String?

    /// Test hook: forget the recorded message.
    static func reset() { last = nil }

    static func post(_ message: String, priority: NSAccessibilityPriorityLevel = .medium) {
        last = message
        // Nothing to say it to when VoiceOver is off.
        guard NSWorkspace.shared.isVoiceOverEnabled else { return }
        NSAccessibility.post(element: NSApplication.shared as Any, notification: .announcementRequested,
                             userInfo: [.announcement: message, .priority: priority.rawValue])
    }
}
