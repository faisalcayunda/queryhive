import SwiftUI

/// The window shell, Navicat's shape with CleanMyMac's surfaces: a title strip, the object tree
/// on the left, the tabbed workspace on the right, and a status bar across the bottom.
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            TitleStrip()
            HStack(spacing: 0) {
                SidebarTree()
                    .frame(width: model.sidebarWidth)
                SidebarResizer()
                Workspace()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            StatusBar()
        }
        .foregroundStyle(Tone.ink)
        .background(Tone.canvas)
        .ignoresSafeArea()
        .sheet(item: $model.editingConnection) { target in
            ConnectionEditorSheet(target: target)
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
    }
}

/// The strip the traffic lights live in.
///
/// It carries nothing at all, and that is the point. A mark placed just after the window controls
/// sat closer to them than to its own wordmark, so the two read as one object and the lights
/// looked crowded; the app's identity moved to the sidebar header, which is a panel and can give
/// it room.
///
/// The connection chip that used to sit at the right end is gone too. It restated the connection
/// the toolbar's breadcrumb already names, on every tab, in the one strip that has no other job —
/// and the row it lived in read as a toolbar with a single control in it. Nothing became
/// unreachable: the editor it opened is on the connection picker's own menu, on the tree's
/// context menu, and on a double-click of any row under the connection.
///
/// What is left is a drag area for the window, at the height the traffic lights need.
struct TitleStrip: View {
    var body: some View {
        Color.clear.frame(height: Metrics.titleStrip)
    }
}

struct StatusBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 9) {
            Circle().fill(dot).frame(width: 7, height: 7)
            Text(model.statusConnection)
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .lineLimit(1)
                .layoutPriority(1)
            Spacer(minLength: 12)
            if let tab = model.selectedTab {
                if tab.stage == .running {
                    ProgressView().controlSize(.mini)
                }
                Text(tab.summary)
                    .font(.system(size: 11, weight: tab.stage == .failed ? .semibold : .regular))
                    .foregroundStyle(tab.stage == .failed ? Tone.coral : Tone.ink.opacity(0.9))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.statusBar)
        .background(Tone.recess.opacity(0.30))
        .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
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

/// Drag handle between the tree and the workspace. A 7pt band with a 1pt line down its middle,
/// so the hit area is comfortable without the seam looking thick.
struct SidebarResizer: View {
    @Environment(AppModel.self) private var model
    @State private var startWidth: CGFloat?

    var body: some View {
        ZStack {
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1)
            Color.clear.contentShape(Rectangle())
        }
        .frame(width: 7)
        .onHover { $0 ? NSCursor.resizeLeftRight.set() : NSCursor.arrow.set() }
        .gesture(
            // `.global`, not the default local space. Same reason as the panel seam: this handle
            // moves as the drag resizes the sidebar, so a local coordinate space measures the
            // translation against an origin that is itself moving.
            DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { value in
                    if startWidth == nil { startWidth = model.sidebarWidth }
                    let base = startWidth ?? model.sidebarWidth
                    model.sidebarWidth = min(460, max(190, base + value.translation.width))
                }
                .onEnded { _ in startWidth = nil }
        )
    }
}
