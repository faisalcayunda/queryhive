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
        .foregroundStyle(.white)
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
/// It carries nothing on the left any more. A mark placed just after the window controls sat
/// closer to them than to its own wordmark, so the two read as one object and the lights looked
/// crowded; the app's identity moved to the sidebar header, which is a panel and can give it
/// room. What is left here is a drag area and the connection this window is pointed at.
struct TitleStrip: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 9) {
            Spacer()
            if let connection = model.selectedConnection {
                Button { model.presentConnectionEditor(connection.id) } label: {
                    HStack(spacing: 7) {
                        Circle().fill(connection.color.color).frame(width: 7, height: 7)
                        Text(connection.name).font(.system(size: 11, weight: .medium))
                        Text(connection.displayTarget)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(Tone.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    .padding(.horizontal, 9)
                    .padding(.vertical, 4)
                    .background(Color.white.opacity(0.06), in: Capsule())
                    .overlay(Capsule().strokeBorder(.white.opacity(0.08)))
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .help("Edit this connection")
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.titleStrip)
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
                    .foregroundStyle(tab.stage == .failed ? Tone.coral : .white.opacity(0.9))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.statusBar)
        .background(Color.black.opacity(0.30))
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }

    private var dot: Color {
        // The dot answers the same question as the label beside it, so the two can never disagree:
        // a green dot next to "Disconnected" is worse than no dot at all. The query's own state is
        // still reported on the right, which is where it belongs.
        switch model.connectionState(for: model.selectedTab?.connectionID) {
        case .connected: return Tone.mint
        case .connecting: return Tone.ice
        case .disconnected: return Tone.gray.opacity(0.6)
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
            Rectangle().fill(.white.opacity(0.07)).frame(width: 1)
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
