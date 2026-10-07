import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// Renders the two new sheets so their layout can be looked at without a window or a server.
///
/// What this proves: each sheet lays out at a real size and draws. What it does not prove: that the
/// app presents it, that a button's action runs, or that the engine accepts what the sheet built —
/// those are the pure-model tests' job and the manual run's. The import sheet is rendered with no
/// connection so its `onAppear` column load returns immediately; no engine call is made here.
///
/// `QH_RENDER_DIR=app/.build/render swift test --filter AppSheetRenderTests` writes the PNGs.
final class AppSheetRenderTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any `AppModel` is built: its init reads and can write the connections store.
        isolateConnectionStore()
    }

    @MainActor
    func testTheConfirmationSheetRenders() throws {
        let request = RunConfirmation.Request(
            statements: ["UPDATE hive.analytics.penerima_manfaat SET aktif = false WHERE tahun = 2026"],
            title: "Run this write?",
            note: "Safe Mode is Confirm on this connection, so the engine runs a write only after "
                + "an explicit approval.")
        try render(RunConfirmationSheet(request: request, onApprove: {}, onCancel: {})
            .background(Tone.canvas),
                   named: "confirmation-sheet.png", size: CGSize(width: 620, height: 320))
    }

    @MainActor
    func testTheImportSheetRenders() throws {
        let model = AppModel()
        var mapping = ImportMapping()
        mapping.path = "/Users/someone/penerima_manfaat_2026.csv"
        mapping.format = .csv
        mapping.targetCatalog = "hive"
        mapping.targetSchema = "analytics"
        mapping.targetTable = "penerima_manfaat"
        mapping.targetColumns = ["id", "nama", "jumlah_jiwa"]
        mapping.fields = ImportMapping.mappedFields(headers: ["id", "nama", "catatan"],
                                                    targetColumns: mapping.targetColumns)
        // No connection, so the sheet's `onAppear` column read returns before touching an engine.
        let draft = ImportDraft(mapping: mapping, connectionID: nil)
        try render(ImportSheet(draft: draft).environment(model).background(Tone.canvas),
                   named: "import-sheet.png", size: CGSize(width: 660, height: 520))
    }

    @MainActor
    func testTheImportSheetRendersWhileItReadsTheFile() throws {
        // The running footer draws a progress figure from the engine's `progress` events: a file's
        // bytes with their total, or a sheet's declared rows. Rendered so the figure's line is laid
        // out, not only written.
        let model = AppModel()
        var mapping = ImportMapping()
        mapping.path = "/Users/someone/penerima_manfaat_2026.csv"
        mapping.format = .csv
        mapping.targetTable = "penerima_manfaat"
        mapping.fields = ImportMapping.mappedFields(headers: ["id", "nama"],
                                                    targetColumns: ["id", "nama"])
        let draft = ImportDraft(mapping: mapping, connectionID: nil)
        draft.running = true
        draft.bytesRead = 3_000_000
        draft.bytesTotal = 4_000_000
        try render(ImportSheet(draft: draft).environment(model).background(Tone.canvas),
                   named: "import-sheet-running.png", size: CGSize(width: 660, height: 520))
    }

    @MainActor
    func testThePartialOrderNoteRenders() throws {
        // The one thin row for an in-memory order over a cut-short result.
        // Rendered because a row of text is exactly where a layout breaks silently.
        try render(
            PartialOrderNote(fetched: 1000)
                .background(Tone.canvas)
                .frame(width: 720, height: 30),
            named: "grid-sort-banner-truncated.png", size: CGSize(width: 720, height: 30))
    }

    @MainActor
    private func render(_ content: some View, named name: String, size: CGSize) throws {
        let host = NSHostingView(rootView: content)
        host.frame = CGRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()

        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds),
                                "\(name): no bitmap to draw into")
        host.cacheDisplay(in: host.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]),
                                 "\(name) could not be encoded as a PNG")

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent(name)
        try data.write(to: path)
        print("rendered \(name) to \(path.path)")
    }

    private static var outputDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return FileManager.default.temporaryDirectory
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }
}
