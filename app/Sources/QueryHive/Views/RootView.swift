import AppKit
import SwiftUI

/// The numbers the window shell is built from, in one place so the window, the harness and the
/// tests read the same ones.
enum Shell {
    /// The object tree's column: the divider can be dragged between these, and a new window opens at
    /// `ideal`. They are the numbers the old hand-made `SidebarResizer` clamped to.
    static let sidebarMin: CGFloat = 190
    static let sidebarIdeal: CGFloat = 264
    static let sidebarMax: CGFloat = 460

    /// The smallest content size of the window (blueprint W9 P-8c). Measured with the widest
    /// breadcrumb the app can draw (three fixed 200 pt levels) and both badges, with the sidebar at
    /// its ideal width: below this width the toolbar's items start to move into the overflow menu.
    static let minWidth: CGFloat = 1400
    static let minHeight: CGFloat = 700
}

/// The window shell: a native split view with the object tree on the left and, on the right, the
/// tabbed workspace over the status bar. The window's own toolbar carries what used to be the
/// query toolbar row (the breadcrumb, the badges and the Run group), and its title is the tab.
///
/// Navicat's shape with CleanMyMac's surfaces, now on the system's split view and toolbar: the
/// divider, the sidebar toggle, Full Keyboard Access and the traffic lights are AppKit's.
struct RootView: View {
    @Environment(AppModel.self) private var model

    /// The sidebar's visibility, owned by the model (`NavigationState.sidebarHidden`) so the menu,
    /// the focus move and the toolbar's own toggle all say the same thing.
    private var columnVisibility: Binding<NavigationSplitViewVisibility> {
        Binding(
            get: { model.navigation.sidebarHidden ? .detailOnly : .all },
            set: { visibility in
                let hidden = visibility == .detailOnly
                guard hidden != model.navigation.sidebarHidden else { return }
                model.navigation.sidebarHidden = hidden
                Announcer.post(hidden ? "Sidebar hidden" : "Sidebar shown")
            })
    }

    var body: some View {
        @Bindable var model = model
        let windowTitle = WindowTitle.make(model: model)
        NavigationSplitView(columnVisibility: columnVisibility) {
            SidebarTree()
                .navigationSplitViewColumnWidth(min: Shell.sidebarMin, ideal: Shell.sidebarIdeal,
                                                max: Shell.sidebarMax)
        } detail: {
            VStack(spacing: 0) {
                Workspace()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                StatusBar()
            }
            // The canvas runs up under the title bar and the toolbar, so the glass has something
            // of ours to sit on; only the background ignores the safe area, never the content.
            .background { Tone.canvas.ignoresSafeArea() }
            .toolbar { QueryToolbarContent(model: model) }
            .navigationTitle(windowTitle.title)
            .navigationSubtitle(windowTitle.subtitle)
        }
        .background(MainWindowTag())
        .foregroundStyle(Tone.ink)
        .sheet(item: $model.editingConnection) { target in
            ConnectionEditorSheet(target: target)
                .environment(model)
        }
        // The engine's `confirm` Safe Mode level, asked where the engine cannot: one approval per
        // run, never remembered. The closure the model holds is what starts the approved run.
        .sheet(item: $model.pendingConfirmation) { pending in
            RunConfirmationSheet(
                request: pending.request,
                onApprove: {
                    model.pendingConfirmation = nil
                    pending.approve()
                },
                onCancel: { model.pendingConfirmation = nil })
        }
        // The import mapping sheet, for a file the user picked.
        .sheet(item: $model.importDraft) { draft in
            ImportSheet(draft: draft)
                .environment(model)
        }
        // The `:name` values, asked once per run. A sheet rather than an overlay because it is a
        // question with an answer, and the answer is the statement that runs.
        .sheet(item: $model.parameterPrompt) { prompt in
            ParameterSheet(prompt: prompt)
                .environment(model)
        }
        .confirmationDialog("Delete \(model.pendingDeletionName)?",
                            isPresented: Binding(
                                get: { model.pendingDeletion != nil },
                                set: { if !$0 { model.pendingDeletion = nil } }
                            )) {
            Button("Delete", role: .destructive) {
                if let id = model.pendingDeletion { model.deleteConnection(id) }
                model.pendingDeletion = nil
            }
            Button("Cancel", role: .cancel) { model.pendingDeletion = nil }
        } message: {
            Text("This removes the saved connection and its Keychain password. This can't be undone.")
        }
        .alert(model.notice?.title ?? "", isPresented: Binding(
            get: { model.notice != nil },
            set: { if !$0 { model.notice = nil } }
        )) {
            Button("OK") { model.notice = nil }
        } message: {
            Text(model.notice?.message ?? "")
        }
        // Naming a group, for both "New Group…" and "Rename Group…". One alert for both because
        // they are the same question — what is this folder called — and the only difference is
        // whether the answer creates a group or changes one.
        .alert(model.groupNaming?.groupID == nil ? "New Group" : "Rename Group",
               isPresented: Binding(
                   get: { model.groupNaming != nil },
                   set: { if !$0 { model.groupNaming = nil } }
               )) {
            TextField("Name", text: Binding(
                get: { model.groupNaming?.name ?? "" },
                set: { model.groupNaming?.name = $0 }
            ))
            Button("Save") { model.commitGroupNaming() }
            Button("Cancel", role: .cancel) { model.groupNaming = nil }
        } message: {
            Text(model.groupNaming?.fileConnectionID == nil
                 ? "Connections can be moved into it from a connection's Group menu."
                 : "The connection will be filed into it.")
        }
        // Open Quickly floats over the workspace rather than taking a sheet: it is a keyboard
        // detour, and Escape has to be able to close it without leaving the window.
        .overlay {
            if model.openQuicklyOpen {
                ZStack(alignment: .top) {
                    Color.black.opacity(0.16)
                        .ignoresSafeArea()
                        .onTapGesture { model.openQuicklyOpen = false }
                    OpenQuickly()
                        .padding(.top, 84)
                }
            }
        }
    }
}

/// What the window's title bar says: the tab's name, and under it the connection and where the
/// query lands. Values so a test can read them without a window.
struct WindowTitle: Equatable {
    let title: String
    let subtitle: String

    static let app = "QueryHive"

    @MainActor
    static func make(model: AppModel) -> WindowTitle {
        guard let tab = model.selectedTab else { return WindowTitle(title: app, subtitle: "") }
        guard let connection = model.connection(for: tab) else { return WindowTitle(title: tab.title, subtitle: "") }
        // Where a bare table name resolves: `catalog.schema`, `database` or `schema` by driver.
        let target = [model.database(for: tab), model.schema(for: tab)].filter { !$0.isEmpty }.joined(separator: ".")
        // The limits are part of the connection's name, said in words because a title has no colour.
        let badges = BadgeSpec.badges(for: connection, in: .breadcrumb).map(\.label)
        return WindowTitle(title: tab.title,
                           subtitle: ([connection.name, target].filter { !$0.isEmpty } + badges).joined(separator: " · "))
    }
}

/// The window this app's main view lives in, found by the view rather than by its title: the title
/// is the open tab's name now, so `window.title == "QueryHive"` no longer says which window it is.
@MainActor
enum MainWindow {
    private(set) static weak var window: NSWindow?

    static func isMain(_ candidate: NSWindow?) -> Bool { candidate != nil && candidate === window }

    fileprivate static func adopt(_ candidate: NSWindow?) {
        if let candidate { window = candidate }
    }
}

/// A zero-size view that tells `MainWindow` which window it landed in.
struct MainWindowTag: NSViewRepresentable {
    final class Tagger: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            MainActor.assumeIsolated { MainWindow.adopt(window) }
        }
    }

    func makeNSView(context: Context) -> NSView { Tagger() }
    func updateNSView(_ view: NSView, context: Context) {}
}

struct StatusBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 9) {
            Circle().fill(dot).frame(width: 7, height: 7)
            Text(model.statusConnection)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .lineLimit(1)
                .layoutPriority(1)
            // Always both, `Full` included: in the one place that is always visible, "no limit"
            // has to be said rather than implied by an empty slot.
            ConnectionBadges(specs: BadgeSpec.badges(for: model.statusConnectionTarget, in: .statusBar))
            Spacer(minLength: 12)
            if let tab = model.selectedTab {
                if tab.stage == .running {
                    ProgressView().controlSize(.mini)
                }
                Text(tab.summary)
                    .font(.ui(11, weight: tab.stage == .failed ? .semibold : .regular))
                    .foregroundStyle(tab.stage == .failed ? Tone.coral : Tone.ink.opacity(0.9))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.statusBar)
        .background(Tone.recess.opacity(0.30))
        .overlay(alignment: .top) { Rectangle().fill(Tone.hairline).frame(height: 1) }
    }

    private var dot: Color {
        // The dot answers the same question as the label beside it, so the two can never disagree:
        // a green dot next to "Disconnected" is worse than no dot at all. The query's own state is
        // still reported on the right, which is where it belongs.
        //
        // It reads the same target the label does — the tree's selection, falling back to the
        // active tab. Reading `selectedTab` directly here was how a bar could name one connection
        // while its dot reported another.
        switch model.connectionState(for: model.statusConnectionTarget?.id) {
        case .connected: return Tone.mint
        case .connecting: return Tone.ice
        case .idle: return Tone.gray.opacity(0.6)
        case .disconnected: return Tone.coral
        }
    }
}
