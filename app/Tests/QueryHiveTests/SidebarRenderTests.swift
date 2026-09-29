import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// Draws the sidebar offscreen and leaves a PNG behind.
///
/// `ImageRenderer` goes through SwiftUI's own layout and drawing, so it needs no window and none of
/// the screen-recording permission that capturing the running app does. That makes it the only way
/// this view can be looked at from an unattended shell.
///
/// What it proves: the view lays out at a real size, draws something rather than an empty rectangle,
/// and shows the favourites it was handed. What it does not prove: that the app puts this view on
/// screen, at this size, next to the rest of the window. A rendered view is evidence about the view.
///
/// The PNG is written for a person to look at. The assertions here are a smoke test; the image is the
/// actual deliverable of running it.
///
/// `QH_RENDER_DIR=app/.build/render swift test --filter SidebarRenderTests` writes both PNGs.
final class SidebarRenderTests: XCTestCase {
    /// One query kept within reach and one that is not, so the render shows a favourite and shows
    /// that a non-favourite is left out of the section.
    private func model() -> AppModel {
        let model = AppModel()
        model.savedQueries = [
            Event.SavedQuery(
                id: "01a0eb14-e488-77b0-9fc4-3a1351cda583",
                name: "Penerima per kabupaten",
                sql: "SELECT kabupaten, count(*) FROM penerima_manfaat GROUP BY kabupaten ORDER BY 2 DESC",
                connectionId: nil,
                folderId: nil,
                favourite: true,
                deleted: false,
                version: 1
            ),
            Event.SavedQuery(
                id: "01a0eb14-e488-77b0-9fc4-3a1351cda584",
                name: "Bukan favorit",
                sql: "SELECT 1",
                connectionId: nil,
                folderId: nil,
                favourite: false,
                deleted: false,
                version: 1
            ),
        ]
        return model
    }

    // `ImageRenderer` is main-actor isolated, so the test has to be too: drawing goes through the
    // same SwiftUI machinery the app runs on the main actor, and pretending otherwise would be a
    // different test than the one that matters.
    @MainActor
    func testTheSidebarRendersWithoutAWindow() throws {
        let renderer = ImageRenderer(
            content: SidebarTree()
                .environment(model())
                .frame(width: 260, height: 460)
        )
        renderer.scale = 2

        let image = try XCTUnwrap(renderer.nsImage, "the sidebar did not render at all")
        XCTAssertEqual(image.size.height, 460, "the render came out at the size it was asked for")

        let data = try XCTUnwrap(
            NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation))?
                .representation(using: .png, properties: [:]),
            "the render could not be encoded as a PNG"
        )

        // A deliberately weak check, and worth being honest about: one colour means nothing was
        // drawn. It cannot tell a correct sidebar from a wrong one -- only that something is there.
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: data))
        var colours = Set<Int>()
        for x in stride(from: 0, to: bitmap.pixelsWide, by: 7) {
            for y in stride(from: 0, to: bitmap.pixelsHigh, by: 7) {
                if let colour = bitmap.colorAt(x: x, y: y) {
                    colours.insert(colour.hash)
                }
            }
        }
        XCTAssertGreaterThan(colours.count, 4, "the sidebar drew one flat colour")

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("sidebar.png")
        try data.write(to: path)
        print("rendered the sidebar to \(path.path)")
    }

    /// The favourites section on its own.
    ///
    /// Rendered separately because the whole-sidebar render above cannot show it: the sidebar's rows
    /// live in a `LazyVStack` inside a `ScrollView`, and an offscreen render gives that no viewport,
    /// so the lazily-laid-out rows never draw. The section does draw when it stands alone, and it is
    /// the part this test exists for.
    @MainActor
    func testTheFavouritesSectionRenders() throws {
        let renderer = ImageRenderer(
            content: FavouritesSection(
                queries: model().savedQueries.filter(\.favourite),
                load: { _ in }
            )
            .frame(width: 260)
        )
        renderer.scale = 2

        let image = try XCTUnwrap(renderer.nsImage, "the favourites section did not render at all")
        let data = try XCTUnwrap(
            NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation))?
                .representation(using: .png, properties: [:]),
            "the render could not be encoded as a PNG"
        )

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("favourites.png")
        try data.write(to: path)
        print("rendered the favourites section to \(path.path)")
    }

    /// The three views nothing has ever looked at: the History panel, the Saved panel, and the Data
    /// tab of Settings.
    ///
    /// All three read what they draw from `AppModel`, so a stub model with rows in it is enough to
    /// put real content on screen. Like the sidebar, the two panels keep rows in a `LazyVStack`
    /// inside a `ScrollView`, and an offscreen render gives that no viewport — so what comes out is
    /// the chrome around the rows, not the rows. That is still worth looking at, and it is the honest
    /// limit of the technique.
    @MainActor
    func testTheLibraryPanelsAndTheDataTabRender() throws {
        let model = AppModel()

        // The connection column resolves the name from the model's own list, so a row is seeded with
        // an id the model really has. An id nobody has renders as nothing, which would make the
        // render look correct while proving less than it appears to.
        let connection = model.connections.first.map { $0.id.uuidString.lowercased() }
        let now = Int(Date().timeIntervalSince1970 * 1000)
        model.historyEntries = [
            Event.HistoryEntry(
                id: "01a0eb14-e488-77b0-9fc4-3a1351cda583",
                sql: "SELECT kabupaten, count(*) FROM penerima_manfaat GROUP BY kabupaten ORDER BY 2 DESC",
                startedAt: now - 900_000,
                elapsedMs: 412,
                rowCount: 514,
                outcome: "ok",
                error: nil,
                connectionId: connection,
                deleted: false,
                version: 1
            ),
            Event.HistoryEntry(
                id: "01a0eb14-e488-77b0-9fc4-3a1351cda584",
                sql: "SELECT * FROM jadwal_distribusi WHERE tanggal > current_date",
                startedAt: now - 3_600_000,
                elapsedMs: 88,
                rowCount: nil,
                outcome: "error",
                error: "relation \"jadwal_distribusi\" does not exist",
                connectionId: connection,
                deleted: false,
                version: 1
            ),
            Event.HistoryEntry(
                id: "01a0eb14-e488-77b0-9fc4-3a1351cda585",
                sql: "SELECT 1",
                startedAt: now - 60_000,
                elapsedMs: nil,
                rowCount: nil,
                outcome: nil,
                error: nil,
                connectionId: nil,
                deleted: false,
                version: 1
            ),
        ]

        let tab = try XCTUnwrap(model.selectedTab, "the model makes a tab on init")
        try render(HistoryPanel(tab: tab).environment(model).frame(width: 520, height: 300),
                   named: "panel-history.png")
        try render(SavedQueriesPanel(tab: tab).environment(model).frame(width: 520, height: 260),
                   named: "panel-saved.png")
        try render(SettingsView(pane: .data).environment(model)
                    .preferredColorScheme(.light).frame(width: 640, height: 420),
                   named: "settings-data.png")

        // The rows on their own, because the panel cannot draw them: this is where the star is, and
        // the star is the whole visible half of the favourites feature.
        // `self.` because the local `model` above shadows this helper's name.
        let queries = self.model().savedQueries
        try render(
            VStack(spacing: 0) {
                ForEach(queries) { query in
                    SavedQueryRow(query: query, load: {}, delete: {}, toggleFavourite: {})
                }
            }
            .background(Color.white)
            .frame(width: 420),
            named: "saved-rows.png"
        )

        // The history rows on their own, for the same reason — and this is the only place the
        // connection column can be looked at, since the panel that holds them cannot draw them.
        //
        // The name is passed in rather than resolved from the model, because a model built in a test
        // process loads whatever `connections.json` it finds and can legitimately have none, which
        // leaves that column empty and the render looking finished while showing nothing. The last row
        // keeps its nil so the absent case is drawn too.
        try render(
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(model.historyEntries.enumerated()), id: \.element.id) { index, entry in
                    HistoryRow(entry: entry,
                               connection: index == 0 ? "BGN Production" : nil) {}
                }
            }
            .background(Color.white)
            .frame(width: 460),
            named: "history-rows.png"
        )

        // `DataSettings` on its own rather than through `SettingsView`, which renders as an unreadable
        // dark rectangle offscreen. Nothing is written here: the controls are drawn with whatever the
        // user's real settings already say, so looking at them cannot change them.
        try render(
            DataSettings().environment(model).preferredColorScheme(.light).frame(width: 560),
            named: "settings-data-pane.png"
        )
        // The same two views through AppKit instead of `ImageRenderer`, because the two do not draw
        // interactive controls the same way and the difference is worth seeing side by side.
        try renderThroughAppKit(
            DataSettings().environment(model),
            named: "settings-data-appkit.png",
            size: CGSize(width: 560, height: 520)
        )
        try renderThroughAppKit(
            HistoryPanel(tab: tab).environment(model),
            named: "panel-history-appkit.png",
            size: CGSize(width: 520, height: 320)
        )
    }

    /// Draws a view through an AppKit hosting view instead of through `ImageRenderer`.
    ///
    /// `ImageRenderer` goes down SwiftUI's display-list path, which has no window behind it, and that
    /// is why interactive controls come out of it as yellow "unavailable" placeholders: a `Toggle` and
    /// a `TextField` draw their content from the control, not from the display list.
    /// `NSHostingView` is the real AppKit view tree, laid out and then drawn into a bitmap — which is
    /// how a window draws it, except nothing is put on screen and no screen-recording permission is
    /// involved.
    ///
    /// This still proves nothing about the app putting the view up. It only draws the view better.
    @MainActor
    private func renderThroughAppKit(_ content: some View, named name: String, size: CGSize) throws {
        let host = NSHostingView(rootView: content)
        host.frame = CGRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()

        let rep = try XCTUnwrap(
            host.bitmapImageRepForCachingDisplay(in: host.bounds),
            "\(name): no bitmap to draw into"
        )
        host.cacheDisplay(in: host.bounds, to: rep)

        let data = try XCTUnwrap(
            rep.representation(using: .png, properties: [:]),
            "\(name) could not be encoded as a PNG"
        )
        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent(name)
        try data.write(to: path)
        print("rendered \(name) through AppKit to \(path.path)")
    }

    /// Draws one view through `ImageRenderer` at a fixed size and writes it out.
    @MainActor
    private func render(_ content: some View, named name: String) throws {
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        let image = try XCTUnwrap(renderer.nsImage, "\(name) did not render at all")
        let data = try XCTUnwrap(
            NSBitmapImageRep(data: try XCTUnwrap(image.tiffRepresentation))?
                .representation(using: .png, properties: [:]),
            "\(name) could not be encoded as a PNG"
        )
        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent(name)
        try data.write(to: path)
        print("rendered \(name) to \(path.path)")
    }

    /// Where the PNG goes. `QH_RENDER_DIR` when it is set, so a caller can ask for it somewhere
    /// findable instead of digging through the temporary directory.
    private static var outputDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }
}
