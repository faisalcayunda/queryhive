import Sparkle
import SwiftUI

/// Sparkle, wrapped thin enough that the rest of the app can keep not caring about it.
///
/// The app is *replaced*, not patched. Sparkle reads an appcast over HTTPS, compares the build
/// number in it against the one in this bundle, downloads the archive, checks it against the EdDSA
/// signature `generate_appcast` made (the public half of that key sits in `Info.plist` as
/// `SUPublicEDKey`), then swaps the bundle and relaunches. The scheduled half of that happens
/// without any of this code running: the controller starts the timer, Sparkle decides when to ask
/// the user, and the answer to "is there something newer" never passes through the app. The one
/// exception is `init`, which also asks once at launch (see there), because the timer alone waits
/// a day from the last check.
///
/// What is left is the manual check, and a flag to keep the menu item honest while one is already
/// running — `SPUUpdater` publishes that over KVO, which SwiftUI cannot observe, so it is mirrored
/// into `@Published` here.
@MainActor
final class Updater: ObservableObject {
    private let controller: SPUStandardUpdaterController

    /// False while a check — scheduled or manual — is already in flight. The menu item is disabled
    /// then, because asking twice cannot make the network faster and a second alert is a bug.
    @Published var canCheckForUpdates = false

    /// `startingUpdater: true` is what schedules the automatic checks. The delegates stay nil: the
    /// standard user driver is the UI we want, and nothing here needs to override how Sparkle
    /// decides to install.
    init() {
        // Not under `--bench`: a benchmark must not phone home or raise Sparkle's own dialogs.
        let starting = !CommandLine.arguments.contains("--bench")
        controller = SPUStandardUpdaterController(startingUpdater: starting,
                                                  updaterDelegate: nil,
                                                  userDriverDelegate: nil)
        // Sparkle's own schedule asks once per `SUScheduledCheckInterval` (a day) counted from the
        // last check, so a launch inside that window found out nothing. Asking at launch is what
        // "check for updates automatically" means to the person who agreed to it. SPUUpdater.h
        // allows this only immediately after starting, and only when automatic checks are on.
        // The check is silent unless something is newer, and a no-op while a session is running.
        let updater = controller.updater
        if starting, updater.automaticallyChecksForUpdates {
            updater.checkForUpdatesInBackground()
        }
        controller.updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .assign(to: &$canCheckForUpdates)
    }

    /// The menu item's action. Named for what the menu says, not for what Sparkle calls it.
    func checkForUpdates() {
        controller.checkForUpdates(nil)
    }
}
