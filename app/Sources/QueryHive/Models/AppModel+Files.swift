import AppKit
import CryptoKit
import SwiftUI
import UniformTypeIdentifiers

// `.sql` files as tabs (blueprint w12 §6, D-18, D-19): open, save, save as, and the check that the
// file on disk is still the one the tab read. Every question put to the person goes through a static
// seam (like `AppModel.confirmDiscard`) so a test answers for it and no test opens a modal.

/// What the close prompt can be answered with.
enum SaveChoice: Equatable { case save, discard, cancel }

/// What the "File changed on disk" question can be answered with.
enum FileChangeChoice: Equatable { case reload, keepMine, saveAs }

/// How a save ended. `failed` has already been said to the person (`AppModel.notice`).
enum SaveOutcome: Equatable {
    case saved
    /// Nothing was written and nothing is wrong: a cancelled panel, a declined overwrite, a reload.
    case cancelled
    case failed(String)
}

// MARK: Disk

/// Reading and writing one `.sql` file. No UI and no model: the rules live here so they are tested
/// with real files.
enum SQLFileIO {
    /// The largest file that opens (D-18). Above it the editor's own ceiling (`CEILING_UTF16`) has
    /// switched colour and folding off anyway; the engine's Import `.sql` handles a big dump.
    static let maxBytes: Int64 = 8 << 20

    struct Loaded {
        var text: String
        var eol: FileBinding.LineEnding
        var hadBOM: Bool
        var stamp: FileBinding.Stamp
    }

    enum ReadError: LocalizedError, Equatable {
        case tooLarge(Int64)
        case notUTF8
        case unreadable(String)

        var errorDescription: String? {
            switch self {
            case .tooLarge(let size):
                let megabytes = Double(size) / Double(1 << 20)
                return String(format: "The file is %.1f MiB, over the 8 MiB the editor opens. "
                              + "Use Import .sql to run a large script.", megabytes)
            case .notUTF8:
                return "The file is not valid UTF-8, and QueryHive will not change its encoding "
                    + "behind your back."
            case .unreadable(let reason):
                return reason
            }
        }
    }

    /// Modification time (nanoseconds) and size, or `nil` when the file is not there.
    static func fileStat(_ url: URL) -> (mtimeNS: Int64, size: Int64)? {
        var info = stat()
        guard stat(url.path, &info) == 0 else { return nil }
        let mtime = Int64(info.st_mtimespec.tv_sec) * 1_000_000_000 + Int64(info.st_mtimespec.tv_nsec)
        return (mtime, Int64(info.st_size))
    }

    static func sha256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// The file's bytes with the stamp they belong to. The stat comes **before** the read: a write
    /// that lands in between then leaves a stamp older than the content, which the next check sees
    /// as a change, instead of a stamp newer than the content, which it would never see.
    static func readBytes(_ url: URL) throws -> (data: Data, mtimeNS: Int64, size: Int64) {
        guard let before = fileStat(url) else { throw ReadError.unreadable("The file is not there any more.") }
        guard before.size <= maxBytes else { throw ReadError.tooLarge(before.size) }
        let data: Data
        do { data = try Data(contentsOf: url) } catch { throw ReadError.unreadable(error.localizedDescription) }
        guard Int64(data.count) <= maxBytes else { throw ReadError.tooLarge(Int64(data.count)) }
        return (data, before.mtimeNS, before.size)
    }

    static func read(_ url: URL) throws -> Loaded {
        let bytes = try readBytes(url)
        let format = detectFormat(bytes.data)
        let body = format.hadBOM ? bytes.data.dropFirst(3) : bytes.data[...]
        // `String(data:encoding:)` answers nil for bytes that are not UTF-8; `String(decoding:)` would
        // repair them with U+FFFD and the next save would write the repair over the file.
        guard let text = String(data: Data(body), encoding: .utf8) else { throw ReadError.notUTF8 }
        return Loaded(text: normalized(text), eol: format.eol, hadBOM: format.hadBOM,
                      stamp: .init(mtimeNS: bytes.mtimeNS, size: bytes.size, sha256: sha256(bytes.data)))
    }

    /// How many line endings of each kind the bytes hold, counted on the bytes (Swift's `Character`
    /// sees `\r\n` as one character, which is not equal to `\n`).
    static func lineEndingCounts(_ data: Data) -> (lf: Int, crlf: Int, cr: Int) {
        var lf = 0, crlf = 0, cr = 0
        var previousWasCR = false
        for byte in data {
            if byte == 0x0A {
                if previousWasCR { crlf += 1; cr -= 1 } else { lf += 1 }
                previousWasCR = false
            } else if byte == 0x0D {
                cr += 1
                previousWasCR = true
            } else {
                previousWasCR = false
            }
        }
        return (lf, crlf, cr)
    }

    /// More than one kind of line ending (a lone `\r` inside an LF file counts): a save writes every
    /// line with one ending, so it would change lines the person never touched (D-18).
    static func hasMixedEndings(_ data: Data) -> Bool {
        let counts = lineEndingCounts(data)
        return [counts.lf, counts.crlf, counts.cr].filter { $0 > 0 }.count > 1
    }

    /// The BOM and the dominant line ending. A tie, or no line ending at all, is LF.
    static func detectFormat(_ data: Data) -> (eol: FileBinding.LineEnding, hadBOM: Bool) {
        let hadBOM = data.count >= 3 && data[data.startIndex] == 0xEF
            && data[data.startIndex + 1] == 0xBB && data[data.startIndex + 2] == 0xBF
        let (lf, crlf, cr) = lineEndingCounts(data)
        if crlf > lf && crlf >= cr { return (.crlf, hadBOM) }
        if cr > lf && cr > crlf { return (.cr, hadBOM) }
        return (.lf, hadBOM)
    }

    /// Every line ending as `\n`, which is what the editor holds.
    static func normalized(_ text: String) -> String {
        guard text.utf8.contains(0x0D) else { return text }
        return text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
    }

    /// The editor's text as the file holds it: the line ending and the BOM put back.
    static func encode(_ text: String, eol: FileBinding.LineEnding, bom: Bool) -> Data {
        var body = normalized(text)
        if eol != .lf { body = body.replacingOccurrences(of: "\n", with: eol.text) }
        var data = Data()
        if bom { data.append(contentsOf: [0xEF, 0xBB, 0xBF]) }
        data.append(Data(body.utf8))
        return data
    }

    /// Write by replacing: the bytes go to a temporary file on the same volume and `replaceItemAt`
    /// swaps it in, so a crash or a full disk leaves the old file whole. A symlink is followed, so
    /// the link stays a link. The stamp comes from what was written. Permissions of an existing file
    /// are kept by `replaceItemAt`; extended attributes and ACLs have not been verified (R-10).
    static func write(_ data: Data, to url: URL) throws -> FileBinding.Stamp {
        let target = url.resolvingSymlinksInPath()
        let manager = FileManager.default
        if manager.fileExists(atPath: target.path) {
            let directory = try manager.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                            appropriateFor: target, create: true)
            defer { try? manager.removeItem(at: directory) }
            let temporary = directory.appendingPathComponent(target.lastPathComponent)
            try data.write(to: temporary)
            _ = try manager.replaceItemAt(target, withItemAt: temporary)
        } else {
            try data.write(to: target, options: .atomic)
        }
        guard let after = fileStat(target) else {
            throw ReadError.unreadable("The file could not be read back after saving.")
        }
        return .init(mtimeNS: after.mtimeNS, size: after.size, sha256: sha256(data))
    }

    /// Where the file stands now, for the checks: the stamp and the bytes when they could be read.
    enum Disk {
        case missing
        /// Present; `data` is `nil` over the size limit.
        case present(FileBinding.Stamp, data: Data?)
    }

    static func inspect(_ url: URL) -> Disk {
        guard let now = fileStat(url) else { return .missing }
        guard now.size <= maxBytes, let bytes = try? readBytes(url) else {
            return .present(.init(mtimeNS: now.mtimeNS, size: now.size, sha256: ""), data: nil)
        }
        return .present(.init(mtimeNS: bytes.mtimeNS, size: bytes.size, sha256: sha256(bytes.data)),
                        data: bytes.data)
    }
}

// MARK: Seams

extension AppModel {
    /// Whether this process is the test runner, where a default must never open a modal.
    static var underTest: Bool { NSClassFromString("XCTestCase") != nil }

    /// "Do you want to save the changes to ...?" The sentence is data so it is tested without an alert.
    static func saveQuestion(for name: String) -> String {
        "Do you want to save the changes to “\(name)”?"
    }

    /// Save, Discard or Cancel, for one tab with unsaved SQL. The test default is Discard, which is
    /// what closing a tab did before the prompt existed.
    static var confirmSave: (String) -> SaveChoice = { name in
        if underTest { return .discard }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = saveQuestion(for: name)
        alert.informativeText = "Your changes will be lost if you don't save them."
        alert.addButton(withTitle: "Save")
        let discard = alert.addButton(withTitle: "Discard")
        discard.hasDestructiveAction = true
        // A button titled "Cancel" answers Esc.
        alert.addButton(withTitle: "Cancel")
        switch alert.runModal() {
        case .alertFirstButtonReturn: return .save
        case .alertSecondButtonReturn: return .discard
        default: return .cancel
        }
    }

    /// The save panel: a name to suggest and a folder to start in; `nil` when it is cancelled. The
    /// test default is a cancelled panel.
    static var chooseSaveURL: (_ suggestedName: String, _ directory: URL?) -> URL? = { name, directory in
        if underTest { return nil }
        let panel = NSSavePanel()
        panel.allowedContentTypes = UTType(filenameExtension: "sql").map { [$0] } ?? []
        panel.nameFieldStringValue = name
        panel.directoryURL = directory
        panel.canCreateDirectories = true
        panel.title = "Save SQL File"
        return panel.runModal() == .OK ? panel.url : nil
    }

    /// "File changed on disk": the file's name and whether the tab holds unsaved changes (which Reload
    /// would discard). The test default keeps the person's text, the answer that destroys nothing.
    static var chooseFileChange: (_ name: String, _ hasUnsavedChanges: Bool) -> FileChangeChoice = { name, dirty in
        if underTest { return .keepMine }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "File changed on disk"
        alert.informativeText = "“\(name)” was changed by another program."
            + (dirty ? " Reloading discards the unsaved changes in this tab." : "")
        // The default (Return) is the answer that loses nothing: a tab with edits keeps them.
        let order: [(String, FileChangeChoice)] = dirty
            ? [("Keep Mine", .keepMine), ("Reload", .reload), ("Save As…", .saveAs)]
            : [("Reload", .reload), ("Keep Mine", .keepMine), ("Save As…", .saveAs)]
        for (title, _) in order { alert.addButton(withTitle: title) }
        let index = alert.runModal().rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
        return order.indices.contains(index) ? order[index].1 : .keepMine
    }

    /// The second confirmation before a save writes over a file somebody else changed. The test
    /// default refuses.
    static var confirmOverwrite: (_ name: String) -> Bool = { name in
        if underTest { return false }
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "Overwrite “\(name)”?"
        alert.informativeText = "The file on disk was changed by another program. Saving replaces those changes with this tab's text."
        alert.addButton(withTitle: "Cancel")
        let overwrite = alert.addButton(withTitle: "Overwrite")
        overwrite.hasDestructiveAction = true
        return alert.runModal() == .alertSecondButtonReturn
    }

    /// "<name> mixes line endings": asked before a save overwrites a file that holds more than one kind,
    /// because the write gives every line `becomes`. Cancel is the default and answers `false`. The
    /// test default declines.
    // ponytail: the editor holds `\n` only, so a mixed file is always written back uniform once the
    // person says yes; a byte-exact round trip of a mixed file is not supported.
    static var confirmMixedEndings: (_ name: String, _ becomes: FileBinding.LineEnding) -> Bool = { name, becomes in
        if underTest { return false }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "“\(name)” mixes line endings."
        alert.informativeText = "Saving writes every line with \(becomes.rawValue.uppercased())."
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Save")
        return alert.runModal() == .alertSecondButtonReturn
    }
}

// MARK: Open

extension AppModel {
    /// One identity for a file however it was reached: symlinks resolved, `.` and `..` gone.
    static func fileIdentity(_ url: URL) -> String {
        url.resolvingSymlinksInPath().standardizedFileURL.path
    }

    static func isSQLFile(_ url: URL) -> Bool {
        ["sql", "txt"].contains(url.pathExtension.lowercased())
    }

    /// The tab bound to this file, if any.
    func boundTab(for url: URL) -> QueryTab? {
        let identity = Self.fileIdentity(url)
        return tabs.first { $0.file.map { Self.fileIdentity($0.url) } == identity }
    }

    /// Opens a file in a **new** tab, or selects the tab that already holds it. The active tab's SQL
    /// is never touched (F-16: "Load SQL File" used to overwrite it without asking). A file that
    /// cannot be opened is said so in an alert and opens nothing.
    @discardableResult
    func openFile(url: URL, connectionID: UUID? = nil) -> QueryTab? {
        switch openOne(url, connectionID: connectionID) {
        case .success(let tab): return tab
        case .failure(let problem):
            notice = Notice(title: "Couldn't open \(url.lastPathComponent)", message: problem.message)
            return nil
        }
    }

    struct FileProblem: Error { let message: String }

    private func openOne(_ url: URL, connectionID: UUID?) -> Result<QueryTab, FileProblem> {
        if let open = boundTab(for: url) {
            selectTab(open.id)
            return .success(open)
        }
        let loaded: SQLFileIO.Loaded
        do {
            loaded = try SQLFileIO.read(url)
        } catch {
            return .failure(FileProblem(message: (error as? LocalizedError)?.errorDescription
                                        ?? error.localizedDescription))
        }
        newTab(connectionID: connectionID)
        guard let tab = selectedTab else { return .failure(FileProblem(message: "No tab to open it in.")) }
        tab.title = url.lastPathComponent
        tab.loadSaved(loaded.text)
        tab.file = FileBinding(url: url, lastKnown: loaded.stamp, eol: loaded.eol, hadBOM: loaded.hadBOM)
        saveSession()
        return .success(tab)
    }

    /// Several files, several tabs; what could not be opened is one alert naming each.
    func openFiles(_ urls: [URL], connectionID: UUID? = nil) {
        var problems: [String] = []
        for url in urls {
            if case .failure(let problem) = openOne(url, connectionID: connectionID) {
                problems.append("\(url.lastPathComponent): \(problem.message)")
            }
        }
        if !problems.isEmpty {
            notice = Notice(title: problems.count == 1 ? "Couldn't open a file" : "Couldn't open \(problems.count) files",
                            message: problems.joined(separator: "\n\n"))
        }
    }

    /// File > Open SQL File, and the tree's "Open SQL File…".
    func openSQLFilesPanel(connectionID: UUID? = nil) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = Self.sqlContentTypes
        panel.allowsMultipleSelection = true
        panel.message = "Choose .sql files to open, each in a new query"
        guard panel.runModal() == .OK else { return }
        openFiles(panel.urls, connectionID: connectionID)
    }

    /// Files dropped on the window: `.sql` and `.txt` open (the connection follows the selected tab,
    /// which is `newTab`'s own default); anything else is not ours to take. Answers whether any file
    /// was taken.
    @discardableResult
    func openDropped(_ urls: [URL]) -> Bool {
        let files = urls.filter(Self.isSQLFile)
        guard !files.isEmpty else { return false }
        openFiles(files)
        return true
    }

    /// The drop handler for SwiftUI: resolves the providers' file URLs (in drop order) and opens them.
    func openDroppedProviders(_ providers: [NSItemProvider]) -> Bool {
        let wanted = providers.filter { $0.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) }
        guard !wanted.isEmpty else { return false }
        final class Box: @unchecked Sendable {
            let lock = NSLock()
            var urls: [Int: URL] = [:]
        }
        let box = Box()
        let group = DispatchGroup()
        for (index, provider) in wanted.enumerated() {
            group.enter()
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) { item, _ in
                let url = (item as? Data).flatMap { URL(dataRepresentation: $0, relativeTo: nil) } ?? (item as? URL)
                if let url {
                    box.lock.lock(); box.urls[index] = url; box.lock.unlock()
                }
                group.leave()
            }
        }
        group.notify(queue: .main) { [weak self] in
            let urls = box.urls.sorted { $0.key < $1.key }.map(\.value)
            self?.openDropped(urls)
        }
        return true
    }
}

// MARK: Save

extension AppModel {
    /// The name a tab with no file suggests to the save panel.
    static func suggestedFileName(for title: String) -> String {
        let cleaned = title.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            .trimmingCharacters(in: .whitespaces)
        let base = cleaned.isEmpty ? "Query" : cleaned
        return base.lowercased().hasSuffix(".sql") ? base : base + ".sql"
    }

    /// What a tab is called to the person asking about it: its file name, or its title.
    static func displayName(of tab: QueryTab) -> String {
        tab.file?.url.lastPathComponent ?? tab.title
    }

    /// Save: a tab with a file writes it (after the checks below); a tab without one is Save As.
    @discardableResult
    func save(_ tab: QueryTab) -> SaveOutcome {
        Self.flushEditorsNow()
        guard var binding = tab.file else { return saveAs(tab) }
        // Nothing to write; the checks below are for a write.
        guard tab.isDirty else { return .saved }
        let name = binding.url.lastPathComponent

        // The content check: what is on disk is what this tab read, or somebody changed it (D-18).
        let disk = SQLFileIO.inspect(binding.url)
        guard case .present(let stamp, let data) = disk else {
            unbindMissingFile(of: tab)
            return saveAs(tab)
        }
        if let data, binding.eol == nil || binding.hadBOM == nil {
            let format = SQLFileIO.detectFormat(data)
            binding.eol = format.eol
            binding.hadBOM = format.hadBOM
        }
        if stamp.sha256 != binding.lastKnown.sha256 {
            if binding.keptOver?.sha256 != stamp.sha256 {
                switch Self.chooseFileChange(name, true) {
                case .reload:
                    reload(tab)
                    return .cancelled
                case .saveAs:
                    return saveAs(tab)
                case .keepMine:
                    binding.keptOver = stamp
                }
            }
            // The second confirmation: Keep Mine alone never overwrites.
            guard Self.confirmOverwrite(name) else {
                tab.file = binding
                return .cancelled
            }
        }

        // The bytes about to be overwritten decide, not a flag from when the tab opened.
        let eol = binding.eol ?? .lf
        if let data, SQLFileIO.hasMixedEndings(data), !Self.confirmMixedEndings(name, eol) {
            tab.file = binding
            return .cancelled
        }

        let bytes = SQLFileIO.encode(tab.sql, eol: eol, bom: binding.hadBOM ?? false)
        do {
            binding.lastKnown = try SQLFileIO.write(bytes, to: binding.url)
        } catch {
            notice = Notice(title: "Couldn't save \(name)", message: error.localizedDescription)
            tab.file = binding
            return .failed(error.localizedDescription)
        }
        binding.keptOver = nil
        tab.file = binding
        tab.markSaved()
        saveSession()
        Announcer.post("Saved \(name)")
        return .saved
    }

    /// Save As: the panel, then a write to the chosen file, which the tab is bound to from then on.
    /// A name from the tab's title when it has no file.
    @discardableResult
    func saveAs(_ tab: QueryTab) -> SaveOutcome {
        Self.flushEditorsNow()
        let suggested = tab.file?.url.lastPathComponent ?? Self.suggestedFileName(for: tab.title)
        guard let url = Self.chooseSaveURL(suggested, tab.file?.url.deletingLastPathComponent()) else {
            return .cancelled
        }
        if let other = boundTab(for: url), other !== tab {
            let why = "“\(url.lastPathComponent)” is already open in another tab. Close that tab first, or choose another name."
            notice = Notice(title: "Couldn't save \(url.lastPathComponent)", message: why)
            return .failed(why)
        }
        let eol = tab.file?.eol ?? .lf
        let bom = tab.file?.hadBOM ?? false
        // Overwriting an existing mixed file asks; a new path never does.
        if case .present(_, let existing?) = SQLFileIO.inspect(url), SQLFileIO.hasMixedEndings(existing),
           !Self.confirmMixedEndings(url.lastPathComponent, eol) {
            return .cancelled
        }
        let stamp: FileBinding.Stamp
        do {
            stamp = try SQLFileIO.write(SQLFileIO.encode(tab.sql, eol: eol, bom: bom), to: url)
        } catch {
            notice = Notice(title: "Couldn't save \(url.lastPathComponent)", message: error.localizedDescription)
            return .failed(error.localizedDescription)
        }
        tab.file = FileBinding(url: url, lastKnown: stamp, eol: eol, hadBOM: bom)
        tab.title = url.lastPathComponent
        tab.markSaved()
        saveSession()
        Announcer.post("Saved \(url.lastPathComponent)")
        return .saved
    }

    /// Whether Save has something to do for this tab: a tab with a file when it has unsaved changes,
    /// a tab without one when there is text to put in a file.
    func canSaveSQL(_ tab: QueryTab) -> Bool {
        guard !tab.isObjects else { return false }
        // `hasUnsavedSQL`, not `isDirty`: the menu item is enabled from observed state, and the counters
        // behind `isDirty` are not observed. For a bound tab the two mean the same.
        return tab.file != nil ? tab.hasUnsavedSQL : tab.hasSQL
    }

    func canSaveSQLAs(_ tab: QueryTab) -> Bool {
        !tab.isObjects && (tab.file != nil || tab.hasSQL)
    }
}

// MARK: External changes

extension AppModel {
    /// The file under a tab was deleted or moved: the tab lets go of it, keeps its text, and is dirty
    /// (the text now exists nowhere else). Said in the tab's Log.
    func unbindMissingFile(of tab: QueryTab) {
        guard let binding = tab.file else { return }
        tab.file = nil
        tab.markDirty()
        tab.note(.warning, "\(binding.url.path) is gone. This tab keeps its text and is no longer bound to a file; "
                 + "Save asks where to put it.")
        saveSession()
    }

    /// Selecting a tab: has its file changed under it? The cheap look is the modification time and
    /// size; only a difference there reads the content, and the same content is no change (a `touch`,
    /// a branch switch back). A real change asks (D-18): Reload, Keep Mine or Save As.
    func checkExternalChange(_ tab: QueryTab) {
        guard let binding = tab.file else { return }
        guard let now = SQLFileIO.fileStat(binding.url) else {
            unbindMissingFile(of: tab)
            return
        }
        func sameStat(_ stamp: FileBinding.Stamp?) -> Bool {
            stamp.map { $0.mtimeNS == now.mtimeNS && $0.size == now.size } ?? false
        }
        if sameStat(binding.lastKnown) || sameStat(binding.keptOver) { return }
        guard case .present(let stamp, _) = SQLFileIO.inspect(binding.url) else {
            unbindMissingFile(of: tab)
            return
        }
        if stamp.sha256 == binding.lastKnown.sha256 {
            tab.file?.lastKnown = stamp
            return
        }
        if stamp.sha256 == binding.keptOver?.sha256 {
            tab.file?.keptOver = stamp
            return
        }
        switch Self.chooseFileChange(binding.url.lastPathComponent, tab.hasUnsavedSQL) {
        case .reload:
            reload(tab)
        case .keepMine:
            tab.file?.keptOver = stamp
            tab.markDirty()
        case .saveAs:
            saveAs(tab)
        }
    }

    /// Replace the tab's text with the file's. The tab's own text is gone afterwards, which is what
    /// the person chose; a file that no longer opens (not UTF-8, over the limit) changes nothing.
    @discardableResult
    func reload(_ tab: QueryTab) -> Bool {
        guard let binding = tab.file else { return false }
        Self.flushEditorsNow()
        let loaded: SQLFileIO.Loaded
        do {
            loaded = try SQLFileIO.read(binding.url)
        } catch {
            notice = Notice(title: "Couldn't reload \(binding.url.lastPathComponent)",
                            message: (error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
            return false
        }
        tab.loadSaved(loaded.text)
        tab.file = FileBinding(url: binding.url, lastKnown: loaded.stamp, eol: loaded.eol, hadBOM: loaded.hadBOM)
        saveSession()
        return true
    }
}
