import AppKit
import SwiftUI

extension AppModel {
    static let restoreTabsKey = "restoreTabsOnLaunch"

    /// The stored answer, for the initializer and for tests.
    static func storedRestoreTabsOnLaunch(in defaults: UserDefaults = .standard) -> Bool {
        // `object(forKey:)` rather than `bool(forKey:)`: the latter cannot tell "off" from "never
        // set", and a setting nobody has opened Settings for has to default to on.
        defaults.object(forKey: restoreTabsKey) as? Bool ?? true
    }

    /// Reads the session the last launch wrote, and replaces the placeholder tab with it.
    ///
    /// The placeholder is made first so the model always has a tab to show, and the restore takes
    /// over only a workspace nobody has used yet. If the load is slower than the user, or the
    /// session is empty, the placeholder stays — which is also what a first launch gets.
    ///
    /// `sessionReady` is set when this answers, whichever way it went: from then on the workspace
    /// is real and worth writing down.
    func restoreSession() {
        var restored: [SessionTab]?
        var active: String?
        Engine.current.run("session", env: Self.localEnvironment(["SESSION_ACTION": "load"]),
                           onEvent: { event in
            guard event.event == "session", event.saved == true else { return }
            restored = event.tabs
            active = event.activeTabId
        }, onExit: { [weak self] status, _ in
            guard let self else { return }
            self.sessionReady = true
            guard status == 0, let restored, !restored.isEmpty else { return }
            // Only a workspace nobody has used: one tab, no SQL, no run, not a listing. Otherwise
            // the user is already working and their tab is the one that wins.
            // An editor may still hold typing the model has not been given.
            Self.flushEditorsNow()
            guard self.tabs.count == 1, let only = self.tabs.first,
                  !only.hasSQL, only.stage == .idle, only.preview == nil,
                  only.objectScope == nil else { return }
            self.tabs = restored.map { snapshot in
                let tab = snapshot.tab()
                // A session saved before the ceiling existed may hold a bigger number: show the
                // clamped one, and say so in that tab's log.
                let limit = Self.clampedRowLimit(tab.rowLimit)
                if limit != tab.rowLimit {
                    tab.note(.warning, Self.rowLimitClampMessage(tab.rowLimit))
                    tab.rowLimit = limit
                }
                return tab
            }
            self.tabCounter = max(self.tabCounter, self.tabs.count)
            if let active, let front = self.tabs.first(where: {
                $0.id.uuidString.caseInsensitiveCompare(active) == .orderedSame
            }) {
                self.selectedTabID = front.id
            } else {
                self.selectedTabID = self.tabs.first?.id
            }
            self.syncPanelToSelectedTab()
        })
    }

    /// Writes the open tabs to the session store.
    ///
    /// Fire and forget, like the history write: the reply is one line nobody reads, and a failed
    /// save has nowhere useful to be shown. Called when the set of tabs changes, when the tab in
    /// front changes, and once more at termination — the last one is what catches SQL typed since
    /// the last tab event.
    func saveSession() {
        guard let env = sessionSaveEnvironment() else { return }
        _ = Engine.current.run("session", env: env, onEvent: { _ in }, onExit: { _, _ in })
    }

    /// Writes the session on the calling thread, for termination.
    ///
    /// The same write as `saveSession`, through the engine's blocking path: at
    /// `applicationWillTerminate` the main queue is the one waiting, so a completion hopped to it
    /// would never run and the last edits would be lost.
    func saveSessionBlocking() {
        guard let env = sessionSaveEnvironment() else { return }
        Engine.current.runBlocking("session", env: env)
    }

    /// The settings a session save runs with, or `nil` when there is nothing to save yet.
    private func sessionSaveEnvironment() -> [String: String]? {
        guard persistsSession, sessionReady else { return nil }
        Self.flushEditorsNow()
        let snapshot = tabs.map(SessionTab.init)
        guard let data = try? JSONEncoder().encode(snapshot),
              let json = String(data: data, encoding: .utf8) else { return nil }
        var env = Self.localEnvironment(["SESSION_ACTION": "save", "TABS_JSON": json])
        if let id = selectedTabID { env["ACTIVE_TAB_ID"] = id.uuidString }
        return env
    }

    /// The settings a command that reaches the local database runs with.
    ///
    /// `DB_PATH` is added only when something redirected the connections store — the test suite and
    /// a snapshot render. The app leaves it out so these commands land in the engine's own default
    /// database, the same file the history and the saved queries live in; naming the file here would
    /// be a second place that has to agree with the engine about what it is called.
    ///
    /// Named for the database rather than for the session because the account and profile commands
    /// need the same redirection: a snapshot must not create an account row in the database the user
    /// actually uses.
    static func localEnvironment(_ base: [String: String]) -> [String: String] {
        guard let root = ConnectionStore.root else { return base }
        var env = base
        env["DB_PATH"] = root.appendingPathComponent("queryhive.sqlite3").path
        return env
    }

    /// Writes the session once more on the way out, which is what catches SQL typed since the last
    /// tab event.
    func observeTermination() {
        terminationObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.saveSessionBlocking()
        }
    }

    /// Reads the account row, which the engine creates on first use, then the profiles it owns.
    ///
    /// Chained rather than fired side by side: both reach the same SQLite file and the account read
    /// is a write the first time, so running them together is a collision for no gain. The engine's
    /// busy timeout would wait it out; not racing is better.
    func loadAccount() {
        _ = Engine.current.run("account", env: Self.localEnvironment(["ACCOUNT_ACTION": "load"]),
                               onEvent: { event in
            switch event.event {
            case "account": self.account = event.account
            case "error": self.accountNotice = event.message
            default: break
            }
        }, onExit: { _, _ in
            self.loadProfiles()
        })
    }

    /// Reads the profiles this account owns.
    func loadProfiles() {
        _ = Engine.current.run("profiles", env: Self.localEnvironment(["PROFILE_ACTION": "list"]),
                               onEvent: { event in
            switch event.event {
            case "profiles": self.profiles = event.profiles ?? []
            case "error": self.accountNotice = event.message
            default: break
            }
        }, onExit: { _, _ in })
    }

    /// Sign in with Google, then write what the provider said into the account row.
    ///
    /// The browser round trip happens here rather than in the engine, because the engine has no
    /// window and cannot open one. What reaches the engine is the three facts the provider gave
    /// us: the provider, the subject, and optionally an email and a name. No token is stored, so
    /// there is nothing to leak later.
    func signInWithGoogle() {
        guard !signingIn else { return }
        guard let clientID = GoogleSignIn.clientID else {
            accountNotice = GoogleSignIn.Failure.noClientID.errorDescription
            return
        }
        signingIn = true
        accountNotice = nil
        Task { @MainActor in
            defer { signingIn = false }
            do {
                let identity = try await GoogleSignInFlow.signIn(clientID: clientID)
                signIn(identity)
            } catch {
                accountNotice = (error as? LocalizedError)?.errorDescription
                    ?? error.localizedDescription
            }
        }
    }

    /// Write a completed sign-in into the account row.
    ///
    /// Separate from the flow above so it can be driven without a browser: the flow's job ends at
    /// the identity, and this is what turns one into the engine's own words.
    func signIn(_ identity: GoogleSignIn.Identity) {
        var env: [String: String] = [
            "ACCOUNT_ACTION": "sign_in",
            "PROVIDER": "google",
            "SUBJECT": identity.subject,
        ]
        if let email = identity.email { env["EMAIL"] = email }
        if let name = identity.displayName { env["DISPLAY_NAME"] = name }
        _ = Engine.current.run("account", env: Self.localEnvironment(env), onEvent: { event in
            switch event.event {
            case "account": self.account = event.account
            case "error": self.accountNotice = event.message
            default: break
            }
        }, onExit: { _, _ in })
    }

    /// Sign out. The row keeps the subject and the email: signing out is a fact, not an erasure.
    func signOut() {
        _ = Engine.current.run("account", env: Self.localEnvironment(["ACCOUNT_ACTION": "sign_out"]),
                               onEvent: { event in
            switch event.event {
            case "account": self.account = event.account
            case "error": self.accountNotice = event.message
            default: break
            }
        }, onExit: { _, _ in })
    }

    /// Remove one profile, then re-read the list.
    func deleteProfile(_ id: String) {
        _ = Engine.current.run("profile_delete", env: Self.localEnvironment(["PROFILE_ID": id]),
                               onEvent: { event in
            if event.event == "error" { self.accountNotice = event.message }
        }, onExit: { _, _ in
            self.loadProfiles()
        })
    }
}
