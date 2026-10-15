import XCTest

@testable import QueryHive

/// The answers a test gives to every question the file feature would put to a person, and what it
/// was asked. Installing it replaces the five static seams (`AppModel.confirmSave`, `chooseSaveURL`,
/// `chooseFileChange`, `confirmOverwrite`, `confirmMixedEndings`) and `restore()` puts the originals back, so no test in this
/// process can open a modal.
@MainActor
final class FileSeams {
    /// Consumed in order; once empty the answer is Discard, the test default.
    var saveAnswers: [SaveChoice] = []
    private(set) var saveAsked: [String] = []
    /// Consumed in order; once empty the panel is cancelled.
    var panelAnswers: [URL?] = []
    private(set) var panelAsked: [String] = []
    var changeAnswer = FileChangeChoice.keepMine
    private(set) var changeAsked: [(name: String, dirty: Bool)] = []
    var overwriteAnswer = false
    private(set) var overwriteAsked: [String] = []
    var mixedAnswer = false
    private(set) var mixedAsked: [(name: String, becomes: FileBinding.LineEnding)] = []

    private let original = (AppModel.confirmSave, AppModel.chooseSaveURL, AppModel.chooseFileChange,
                            AppModel.confirmOverwrite, AppModel.confirmMixedEndings)

    init() {
        AppModel.confirmSave = { [unowned self] name in
            saveAsked.append(name)
            return saveAnswers.isEmpty ? .discard : saveAnswers.removeFirst()
        }
        AppModel.chooseSaveURL = { [unowned self] name, _ in
            panelAsked.append(name)
            return panelAnswers.isEmpty ? nil : panelAnswers.removeFirst()
        }
        AppModel.chooseFileChange = { [unowned self] name, dirty in
            changeAsked.append((name, dirty))
            return changeAnswer
        }
        AppModel.confirmOverwrite = { [unowned self] name in
            overwriteAsked.append(name)
            return overwriteAnswer
        }
        AppModel.confirmMixedEndings = { [unowned self] name, becomes in
            mixedAsked.append((name, becomes))
            return mixedAnswer
        }
    }

    func restore() {
        (AppModel.confirmSave, AppModel.chooseSaveURL, AppModel.chooseFileChange,
         AppModel.confirmOverwrite, AppModel.confirmMixedEndings) = original
    }
}

/// A scratch directory for one test's files, removed afterwards.
extension XCTestCase {
    func makeScratchDirectory() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-files-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        return directory
    }
}

/// `.sql` tabs bound to a file (blueprint w12 §6, D-18): what opening, saving and the external-change
/// check do to the file and to the tab, against real files.
@MainActor
final class FileBindingTests: XCTestCase {
    private var directory: URL!
    private var seams: FileSeams!
    private var model: AppModel!

    override func setUpWithError() throws {
        try super.setUpWithError()
        isolateConnectionStore()
        directory = try makeScratchDirectory()
        seams = FileSeams()
        model = AppModel()
    }

    override func tearDown() {
        seams.restore()
        super.tearDown()
    }

    private func write(_ name: String, _ bytes: [UInt8]) throws -> URL {
        let url = directory.appendingPathComponent(name)
        try Data(bytes).write(to: url)
        return url
    }

    private func write(_ name: String, _ text: String) throws -> URL {
        try write(name, Array(text.utf8))
    }

    private func contents(_ url: URL) throws -> [UInt8] { Array(try Data(contentsOf: url)) }

    private func opened(_ url: URL) throws -> QueryTab {
        try XCTUnwrap(model.openFile(url: url), "the file did not open: \(model.notice?.message ?? "no notice")")
    }

    // MARK: Open

    func testOpeningAFileMakesANewBoundTabWithTheFilesName() throws {
        let url = try write("report.sql", "SELECT 1;\n")
        let before = model.tabs.count

        let tab = try opened(url)

        XCTAssertEqual(model.tabs.count, before + 1)
        XCTAssertEqual(model.selectedTabID, tab.id)
        XCTAssertEqual(tab.sql, "SELECT 1;\n")
        XCTAssertEqual(tab.title, "report.sql")
        XCTAssertEqual(tab.file?.url, url)
        XCTAssertFalse(tab.isDirty, "text that was just read is the saved text")
        XCTAssertFalse(tab.hasUnsavedSQL)
    }

    func testOpeningDoesNotOverwriteTheActiveTabsSQL() throws {
        let active = try XCTUnwrap(model.selectedTab)
        active.sql = "SELECT my_work FROM unsaved"
        let url = try write("other.sql", "SELECT 2;\n")

        let tab = try opened(url)

        XCTAssertNotEqual(tab.id, active.id)
        XCTAssertEqual(active.sql, "SELECT my_work FROM unsaved", "F-16: the old path overwrote this")
        XCTAssertTrue(model.tabs.contains { $0.id == active.id })
    }

    func testOpeningAnOpenFileSelectsItsTabInsteadOfDuplicatingIt() throws {
        let url = try write("once.sql", "SELECT 1;\n")
        let first = try opened(url)
        model.newTab()
        let count = model.tabs.count

        let again = try opened(url)

        XCTAssertEqual(again.id, first.id)
        XCTAssertEqual(model.tabs.count, count)
        XCTAssertEqual(model.selectedTabID, first.id)
    }

    func testANonUTF8FileIsRefusedAndNothingOpens() throws {
        let url = try write("latin1.sql", [0x53, 0xE9, 0x4C])   // "SéL" in Latin-1, not valid UTF-8
        let before = model.tabs.count

        XCTAssertNil(model.openFile(url: url))

        XCTAssertEqual(model.tabs.count, before)
        XCTAssertTrue(model.notice?.message.contains("UTF-8") == true, "says why")
    }

    func testAFileOverTheLimitIsRefusedWithAPointerToImport() throws {
        let url = try write("huge.sql", [UInt8](repeating: 0x61, count: Int(SQLFileIO.maxBytes) + 1))
        let before = model.tabs.count

        XCTAssertNil(model.openFile(url: url))

        XCTAssertEqual(model.tabs.count, before)
        XCTAssertTrue(model.notice?.message.contains("Import .sql") == true)
    }

    func testSeveralDroppedFilesOpenSeveralTabsAndOtherFilesAreLeftAlone() throws {
        let one = try write("a.sql", "SELECT 1;\n")
        let two = try write("b.txt", "SELECT 2;\n")
        let image = try write("c.png", [0x89, 0x50])
        let before = model.tabs.count

        XCTAssertTrue(model.openDropped([one, image, two]))

        XCTAssertEqual(model.tabs.count, before + 2)
        XCTAssertEqual(model.tabs.suffix(2).map(\.title), ["a.sql", "b.txt"], "in drop order")
        XCTAssertFalse(model.openDropped([image]), "not ours to take")
    }

    // MARK: Format

    func testCRLFIsEditedAsLFAndRestoredOnSave() throws {
        let url = try write("crlf.sql", "SELECT 1;\r\nSELECT 2;\r\n")
        let tab = try opened(url)
        XCTAssertEqual(tab.sql, "SELECT 1;\nSELECT 2;\n", "the editor's text is always \\n")
        XCTAssertEqual(tab.file?.eol, .crlf)

        tab.sql += "SELECT 3;\n"
        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(try contents(url), Array("SELECT 1;\r\nSELECT 2;\r\nSELECT 3;\r\n".utf8),
                       "every line of a CRLF file stays CRLF, or a VCS sees every line changed")
    }

    func testLoneCRFilesComeBackAsLoneCR() throws {
        let url = try write("cr.sql", "a\rb\r")
        let tab = try opened(url)
        XCTAssertEqual(tab.sql, "a\nb\n")
        tab.sql += "c\n"
        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertEqual(try contents(url), Array("a\rb\rc\r".utf8))
    }

    func testTheBOMIsKeptAndNeverShownInTheText() throws {
        let url = try write("bom.sql", [0xEF, 0xBB, 0xBF] + Array("SELECT 1;\n".utf8))
        let tab = try opened(url)
        XCTAssertEqual(tab.sql, "SELECT 1;\n", "no U+FEFF in the editor")
        XCTAssertEqual(tab.file?.hadBOM, true)

        tab.sql = "SELECT 2;\n"
        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(try contents(url), [0xEF, 0xBB, 0xBF] + Array("SELECT 2;\n".utf8))
    }

    func testAFileWithNoBOMAndLFStaysThatWay() throws {
        let url = try write("plain.sql", "SELECT 1;\n")
        let tab = try opened(url)
        XCTAssertEqual(tab.file?.eol, .lf)
        XCTAssertEqual(tab.file?.hadBOM, false)
        tab.sql = "SELECT 2;\n"
        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertEqual(try contents(url), Array("SELECT 2;\n".utf8))
    }

    // MARK: Mixed line endings (D-18: never changed silently)

    func testDecliningAMixedEndingsSaveLeavesTheFileByteIdenticalAndTheTabDirty() throws {
        let url = try write("mixed.sql", "a\r\nb\nc\r\n")
        let before = try contents(url)
        let tab = try opened(url)
        tab.sql += "d\n"

        seams.mixedAnswer = false
        XCTAssertEqual(model.save(tab), .cancelled)

        XCTAssertEqual(try contents(url), before, "nothing written")
        XCTAssertTrue(tab.isDirty)
        XCTAssertTrue(model.tabs.contains { $0.id == tab.id })
        XCTAssertEqual(seams.mixedAsked.map(\.name), ["mixed.sql"])
        XCTAssertEqual(seams.mixedAsked.map(\.becomes), [.crlf], "the dominant ending")
        XCTAssertNil(model.notice, "a decline is not an error")
    }

    func testAcceptingAMixedEndingsSaveWritesTheFileUniformInTheDominantEnding() throws {
        let url = try write("mixed.sql", "a\r\nb\nc\r\n")
        let tab = try opened(url)
        tab.sql += "d\n"

        seams.mixedAnswer = true
        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(try contents(url), Array("a\r\nb\r\nc\r\nd\r\n".utf8))
        XCTAssertFalse(tab.isDirty)
        XCTAssertEqual(seams.mixedAsked.count, 1)
    }

    func testALoneCRInsideALiteralOfAnLFFileCountsAsMixedAndDecliningKeepsTheBytes() throws {
        let url = try write("lit.sql", "SELECT 'a\rb';\nSELECT 2;\n")
        let before = try contents(url)
        let tab = try opened(url)
        tab.sql += "SELECT 3;\n"

        XCTAssertEqual(model.save(tab), .cancelled)

        XCTAssertEqual(try contents(url), before, "the literal's CR is not turned into LF")
        XCTAssertEqual(seams.mixedAsked.map(\.becomes), [.lf])
    }

    func testAUniformCRLFFileSavesWithNoQuestionAndStaysCRLF() throws {
        let url = try write("uniform.sql", "a\r\nb\r\n")
        let tab = try opened(url)
        tab.sql += "c\n"

        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertTrue(seams.mixedAsked.isEmpty)
        XCTAssertEqual(try contents(url), Array("a\r\nb\r\nc\r\n".utf8))
    }

    func testSaveAsToANewPathNeverAsksButOverwritingAMixedFileDoes() throws {
        let url = try write("src.sql", "a\r\nb\nc\r\n")
        let tab = try opened(url)
        tab.sql += "d\n"

        let fresh = directory.appendingPathComponent("fresh.sql")
        seams.panelAnswers = [fresh]
        XCTAssertEqual(model.saveAs(tab), .saved)
        XCTAssertTrue(seams.mixedAsked.isEmpty, "a new file has nothing to mix with")
        XCTAssertEqual(try contents(fresh), Array("a\r\nb\r\nc\r\nd\r\n".utf8))

        let other = try write("other.sql", "x\ny\r\n")
        let before = try contents(other)
        tab.sql += "e\n"
        seams.panelAnswers = [other]
        XCTAssertEqual(model.saveAs(tab), .cancelled)
        XCTAssertEqual(seams.mixedAsked.map(\.name), ["other.sql"])
        XCTAssertEqual(try contents(other), before)
        XCTAssertEqual(tab.file?.url, fresh, "still bound to where it was")
    }

    // MARK: Dirty

    func testDirtyComesFromTheEditCounterAndSavingClearsIt() throws {
        let url = try write("d.sql", "SELECT 1;\n")
        let tab = try opened(url)
        XCTAssertEqual(tab.sqlEdits, tab.savedEdits)

        tab.sql = "SELECT 1;\nSELECT 2;\n"
        XCTAssertNotEqual(tab.sqlEdits, tab.savedEdits)
        XCTAssertTrue(tab.hasUnsavedSQL)

        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertEqual(tab.sqlEdits, tab.savedEdits)
        XCTAssertFalse(tab.hasUnsavedSQL)
        XCTAssertEqual(try contents(url), Array("SELECT 1;\nSELECT 2;\n".utf8))
    }

    func testWritingTheSameTextStillCountsAsAnEditAndUndoToSavedStaysDirty() throws {
        // Blueprint §6: "Save, then undo back to the saved text, stays dirty (accepted)". The counter
        // is how a megabyte document avoids a string comparison on every write.
        let url = try write("u.sql", "SELECT 1;\n")
        let tab = try opened(url)
        tab.sql = "SELECT 1;\nX"
        tab.sql = "SELECT 1;\n"
        XCTAssertTrue(tab.hasUnsavedSQL)
    }

    func testGeneratedTextIsNotAnEditUnlessTheTabAlreadyHeldWork() throws {
        let tab = QueryTab(title: "Query 1")
        tab.applyGeneratedSQL("SELECT * FROM t")
        XCTAssertFalse(tab.hasUnsavedSQL, "a generated SELECT is not the person's work")

        tab.sql = "SELECT * FROM t WHERE a = 1"
        XCTAssertTrue(tab.hasUnsavedSQL)
        tab.applyGeneratedSQL("SELECT 2")
        XCTAssertTrue(tab.hasUnsavedSQL, "replacing the person's text does not make it clean")

        let url = try write("g.sql", "SELECT 1;\n")
        let bound = try opened(url)
        bound.applyGeneratedSQL("SELECT generated")
        XCTAssertTrue(bound.hasUnsavedSQL, "a bound tab whose text no longer matches its file is unsaved")
    }

    func testBlankTextInAnUnboundTabIsNothingToSave() {
        let tab = QueryTab(title: "Query 1")
        tab.sql = "SELECT 1"
        tab.sql = "  \n "
        XCTAssertTrue(tab.isDirty)
        XCTAssertFalse(tab.hasUnsavedSQL)
    }

    func testTheChipSaysUnsavedInWordsAsWellAsADot() {
        XCTAssertEqual(TabChipSpec(title: "a", stage: .idle, isSelected: false).value, nil)
        XCTAssertEqual(TabChipSpec(title: "a", stage: .idle, isSelected: false, unsaved: true).value,
                       "unsaved changes.")
        XCTAssertEqual(TabChipSpec(title: "a", stage: .done, isSelected: false, unsaved: true).value,
                       "finished. unsaved changes.")
    }

    // MARK: External changes

    private func setModificationTime(_ url: URL, toNanoseconds ns: Int64) throws {
        let spec = timespec(tv_sec: Int(ns / 1_000_000_000), tv_nsec: Int(ns % 1_000_000_000))
        var times = [spec, spec]
        XCTAssertEqual(utimensat(AT_FDCWD, url.path, &times, 0), 0)
    }

    func testAChangeWithTheSameSizeAndModificationTimeIsFoundByContentBeforeASave() throws {
        let url = try write("same.sql", "aaaa\n")
        let tab = try opened(url)
        let known = try XCTUnwrap(tab.file?.lastKnown)

        _ = try write("same.sql", "bbbb\n")
        try setModificationTime(url, toNanoseconds: known.mtimeNS)
        let now = try XCTUnwrap(SQLFileIO.fileStat(url))
        XCTAssertEqual(now.mtimeNS, known.mtimeNS)
        XCTAssertEqual(now.size, known.size, "the cheap check cannot tell")

        // Selecting the tab uses the cheap check, by design (§6): no question.
        model.newTab()
        model.selectTab(tab.id)
        XCTAssertTrue(seams.changeAsked.isEmpty)

        // A save reads the content: it sees the change, asks, and Keep Mine still needs a second yes.
        tab.sql = "mine\n"
        seams.changeAnswer = .keepMine
        seams.overwriteAnswer = false
        XCTAssertEqual(model.save(tab), .cancelled)
        XCTAssertEqual(seams.changeAsked.count, 1)
        XCTAssertEqual(try contents(url), Array("bbbb\n".utf8), "the other program's file is intact")
    }

    func testSelectingATabWhoseFileGrewAsksWithWhetherTheTabHoldsEdits() throws {
        let url = try write("grow.sql", "aaaa\n")
        let tab = try opened(url)
        _ = try write("grow.sql", "aaaa\nmore\n")
        model.newTab()

        seams.changeAnswer = .reload
        model.selectTab(tab.id)

        XCTAssertEqual(seams.changeAsked.map(\.name), ["grow.sql"])
        XCTAssertEqual(seams.changeAsked.map(\.dirty), [false])
        XCTAssertEqual(tab.sql, "aaaa\nmore\n", "Reload takes the file's text")
        XCTAssertFalse(tab.hasUnsavedSQL)
    }

    func testTouchingAFileWithoutChangingItAsksNothing() throws {
        let url = try write("touch.sql", "aaaa\n")
        let tab = try opened(url)
        try setModificationTime(url, toNanoseconds: (tab.file?.lastKnown.mtimeNS ?? 0) + 5_000_000_000)
        model.newTab()
        model.selectTab(tab.id)
        XCTAssertTrue(seams.changeAsked.isEmpty, "same content, new mtime: no question")
        XCTAssertEqual(tab.file?.lastKnown.mtimeNS, SQLFileIO.fileStat(url)?.mtimeNS,
                       "the stamp follows the file, so the cheap check stays quiet next time")
    }

    func testKeepMineAtSelectionMarksTheTabDirtyAndTheNextSaveStillAsksBeforeOverwriting() throws {
        let url = try write("keep.sql", "aaaa\n")
        let tab = try opened(url)
        _ = try write("keep.sql", "theirs, longer\n")
        model.newTab()

        seams.changeAnswer = .keepMine
        model.selectTab(tab.id)
        XCTAssertEqual(seams.changeAsked.count, 1)
        XCTAssertTrue(tab.isDirty, "Keep Mine marks the tab dirty")

        // The sheet is not asked again for the same disk content; only the second confirmation is.
        seams.overwriteAnswer = false
        XCTAssertEqual(model.save(tab), .cancelled)
        XCTAssertEqual(seams.changeAsked.count, 1)
        XCTAssertEqual(seams.overwriteAsked, ["keep.sql"])
        XCTAssertEqual(try contents(url), Array("theirs, longer\n".utf8), "no overwrite without the second yes")
        XCTAssertTrue(tab.isDirty)

        seams.overwriteAnswer = true
        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertEqual(try contents(url), Array("aaaa\n".utf8), "the tab's text, after the second yes")
        XCTAssertFalse(tab.isDirty)
    }

    func testKeepMineInTheSaveSheetAlsoNeedsTheSecondConfirmation() throws {
        let url = try write("sheet.sql", "aaaa\n")
        let tab = try opened(url)
        tab.sql = "mine\n"
        _ = try write("sheet.sql", "theirs!\n")

        seams.changeAnswer = .keepMine
        seams.overwriteAnswer = false
        XCTAssertEqual(model.save(tab), .cancelled)
        XCTAssertEqual(try contents(url), Array("theirs!\n".utf8))
        XCTAssertEqual(seams.overwriteAsked.count, 1)
    }

    func testReloadInTheSaveSheetReplacesTheTabsTextAndWritesNothing() throws {
        let url = try write("reload.sql", "aaaa\n")
        let tab = try opened(url)
        tab.sql = "mine\n"
        _ = try write("reload.sql", "theirs!\n")

        seams.changeAnswer = .reload
        XCTAssertEqual(model.save(tab), .cancelled)

        XCTAssertEqual(tab.sql, "theirs!\n")
        XCTAssertFalse(tab.isDirty)
        XCTAssertEqual(try contents(url), Array("theirs!\n".utf8))
    }

    func testSaveAsInTheSaveSheetWritesTheTabsTextToANewFileAndLeavesTheOldOne() throws {
        let url = try write("orig.sql", "aaaa\n")
        let tab = try opened(url)
        tab.sql = "mine\n"
        _ = try write("orig.sql", "theirs!\n")
        let target = directory.appendingPathComponent("mine.sql")

        seams.changeAnswer = .saveAs
        seams.panelAnswers = [target]
        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(try contents(target), Array("mine\n".utf8))
        XCTAssertEqual(try contents(url), Array("theirs!\n".utf8))
        XCTAssertEqual(tab.file?.url, target)
        XCTAssertEqual(tab.title, "mine.sql")
    }

    func testAMissingFileUnbindsTheTabKeepsItsTextAndSaysSoInTheLog() throws {
        let url = try write("gone.sql", "SELECT 1;\n")
        let tab = try opened(url)
        model.newTab()
        try FileManager.default.removeItem(at: url)

        model.selectTab(tab.id)

        XCTAssertNil(tab.file)
        XCTAssertEqual(tab.sql, "SELECT 1;\n", "the editor's text is kept")
        XCTAssertTrue(tab.hasUnsavedSQL, "it exists nowhere else now")
        XCTAssertTrue(tab.logLines.last?.text.contains("gone") == true)
        XCTAssertTrue(seams.changeAsked.isEmpty, "nothing to choose between")
    }

    func testSavingATabWhoseFileVanishedAsksWhereToPutIt() throws {
        let url = try write("vanish.sql", "SELECT 1;\n")
        let tab = try opened(url)
        tab.sql = "SELECT 2;\n"
        try FileManager.default.removeItem(at: url)
        let target = directory.appendingPathComponent("elsewhere.sql")
        seams.panelAnswers = [target]

        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(try contents(target), Array("SELECT 2;\n".utf8))
        XCTAssertEqual(tab.file?.url, target)
    }

    // MARK: Save and Save As

    func testSavingATabWithNoFileIsSaveAsWithANameFromTheTitle() throws {
        model.newTab()
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = "SELECT 7;"
        let suggested = AppModel.suggestedFileName(for: tab.title)
        let target = directory.appendingPathComponent("seven.sql")
        seams.panelAnswers = [target]

        XCTAssertEqual(model.save(tab), .saved)

        XCTAssertEqual(seams.panelAsked, [suggested])
        XCTAssertEqual(try contents(target), Array("SELECT 7;".utf8))
        XCTAssertEqual(tab.file?.url, target)
        XCTAssertEqual(tab.title, "seven.sql")
        XCTAssertFalse(tab.hasUnsavedSQL)
    }

    func testACancelledSavePanelWritesNothingAndChangesNothing() throws {
        model.newTab()
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = "SELECT 7;"
        seams.panelAnswers = [nil]

        XCTAssertEqual(model.save(tab), .cancelled)

        XCTAssertNil(tab.file)
        XCTAssertTrue(tab.hasUnsavedSQL)
    }

    func testSuggestedNamesAreSafeAndEndInSQL() {
        XCTAssertEqual(AppModel.suggestedFileName(for: "Query 1"), "Query 1.sql")
        XCTAssertEqual(AppModel.suggestedFileName(for: "a/b:c"), "a-b-c.sql")
        XCTAssertEqual(AppModel.suggestedFileName(for: "x.SQL"), "x.SQL")
        XCTAssertEqual(AppModel.suggestedFileName(for: "  "), "Query.sql")
    }

    func testSavingACleanBoundTabWritesNothing() throws {
        let url = try write("clean.sql", "SELECT 1;\n")
        let tab = try opened(url)
        let before = try XCTUnwrap(SQLFileIO.fileStat(url))
        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertEqual(SQLFileIO.fileStat(url)?.mtimeNS, before.mtimeNS)
    }

    func testSaveAsRefusesAFileThatAnotherTabHolds() throws {
        let held = try write("held.sql", "SELECT 1;\n")
        _ = try opened(held)
        model.newTab()
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = "SELECT 2;"
        seams.panelAnswers = [held]

        if case .failed = model.saveAs(tab) {} else { XCTFail("two tabs on one file would each overwrite the other") }
        XCTAssertEqual(try contents(held), Array("SELECT 1;\n".utf8))
        XCTAssertNotNil(model.notice)
    }

    func testSavingThroughASymlinkKeepsTheLink() throws {
        let real = try write("real.sql", "SELECT 1;\n")
        let link = directory.appendingPathComponent("link.sql")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: real)
        let tab = try opened(link)
        tab.sql = "SELECT 2;\n"
        XCTAssertEqual(model.save(tab), .saved)
        XCTAssertNotNil(try? FileManager.default.destinationOfSymbolicLink(atPath: link.path))
        XCTAssertEqual(try contents(real), Array("SELECT 2;\n".utf8))
    }

    func testTheOneSaveItemSavesTheSQLWhenNoGridChangeIsStaged() throws {
        let url = try write("menu.sql", "SELECT 1;\n")
        let tab = try opened(url)
        XCTAssertFalse(model.canSaveFocused, "a clean bound tab has nothing to save")
        tab.sql = "SELECT 9;\n"
        XCTAssertTrue(model.canSaveFocused)

        model.saveFocused()

        XCTAssertEqual(try contents(url), Array("SELECT 9;\n".utf8))
        XCTAssertFalse(tab.reviewRequested, "no staged edit, so no review")
    }

    /// The menu item is enabled from observed state: typing into a clean bound tab must reach it, or
    /// ⌘S stays greyed out after the first edit. `canSaveFocused` always recomputes, so only the
    /// observation can show it.
    func testTypingIntoACleanBoundTabInvalidatesWhatEnablesSave() throws {
        let url = try write("obs.sql", "SELECT 1;\n")
        let tab = try opened(url)
        XCTAssertFalse(model.canSaveFocused)
        var fired = false
        withObservationTracking({ _ = model.canSaveFocused }, onChange: { fired = true })

        tab.sql = "SELECT 2;\n"

        XCTAssertTrue(fired, "an edit must invalidate the Save item's enabled state")
        XCTAssertTrue(model.canSaveFocused)
    }

    func testSaveAsIsEnabledForAnyTabWithTextOrAFile() throws {
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertFalse(model.canSaveSQLAs(tab), "nothing to put in a file")
        tab.sql = "SELECT 1"
        XCTAssertTrue(model.canSaveSQLAs(tab))
    }

    func testStagedGridChangesWinOverSavingTheSQL() throws {
        let url = try write("both.sql", "SELECT 1;\n")
        let tab = try opened(url)
        tab.sql = "SELECT 9;\n"
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "0")
        XCTAssertTrue(model.canReviewStagedEdits)

        model.saveFocused()

        XCTAssertTrue(tab.reviewRequested, "the review of the staged changes")
        XCTAssertEqual(try contents(url), Array("SELECT 1;\n".utf8), "the file is untouched this press")
    }
}
