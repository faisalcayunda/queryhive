import XCTest

@testable import QueryHive

/// The close prompt for a tab with unsaved SQL: Save / Discard / Cancel, asked once for every unsaved
/// tab of whatever set is closing, before anything closes (the five close paths share one gate).
@MainActor
final class UnsavedSQLCloseTests: XCTestCase {
    private var directory: URL!
    private var seams: FileSeams!
    private var model: AppModel!
    private var savedDiscard: ((UnsavedWork, DiscardIntent) -> Bool)!
    private var discardAsked: [(UnsavedWork, DiscardIntent)] = []

    override func setUpWithError() throws {
        try super.setUpWithError()
        isolateConnectionStore()
        directory = try makeScratchDirectory()
        seams = FileSeams()
        savedDiscard = AppModel.confirmDiscard
        discardAsked = []
        AppModel.confirmDiscard = { [unowned self] work, intent in
            discardAsked.append((work, intent))
            return true
        }
        model = AppModel()
    }

    override func tearDown() {
        seams.restore()
        AppModel.confirmDiscard = savedDiscard
        super.tearDown()
    }

    /// A new tab holding SQL the person typed.
    @discardableResult
    private func dirtyTab(_ sql: String = "SELECT 1", connection: UUID? = nil) -> QueryTab {
        model.newTab(connectionID: connection)
        let tab = model.selectedTab!
        tab.sql = sql
        return tab
    }

    private func url(_ name: String) -> URL { directory.appendingPathComponent(name) }

    private func boundDirtyTab(_ name: String, onDisk: String = "on disk\n", edited: String = "edited\n") throws -> QueryTab {
        try Data(onDisk.utf8).write(to: url(name))
        let tab = try XCTUnwrap(model.openFile(url: url(name)))
        tab.sql = edited
        return tab
    }

    private func text(_ name: String) throws -> String {
        String(decoding: try Data(contentsOf: url(name)), as: UTF8.self)
    }

    // MARK: One tab

    func testCancelKeepsTheTabAndItsText() {
        let tab = dirtyTab("SELECT mine")
        seams.saveAnswers = [.cancel]

        XCTAssertFalse(model.closeTab(tab.id))

        XCTAssertTrue(model.tabs.contains { $0.id == tab.id })
        XCTAssertEqual(tab.sql, "SELECT mine")
        XCTAssertTrue(seams.panelAsked.isEmpty)
    }

    func testDiscardClosesWithoutWriting() throws {
        let tab = try boundDirtyTab("d.sql")
        seams.saveAnswers = [.discard]

        XCTAssertTrue(model.closeTab(tab.id))

        XCTAssertFalse(model.tabs.contains { $0.id == tab.id })
        XCTAssertEqual(try text("d.sql"), "on disk\n")
    }

    func testSaveWritesTheBoundFileAndThenCloses() throws {
        let tab = try boundDirtyTab("s.sql")
        seams.saveAnswers = [.save]

        XCTAssertTrue(model.closeTab(tab.id))

        XCTAssertFalse(model.tabs.contains { $0.id == tab.id })
        XCTAssertEqual(try text("s.sql"), "edited\n")
    }

    func testSaveOnAnUnboundTabOpensTheSavePanelAndWritesThere() throws {
        let tab = dirtyTab("SELECT 5")
        let target = url("five.sql")
        seams.saveAnswers = [.save]
        seams.panelAnswers = [target]

        XCTAssertTrue(model.closeTab(tab.id))

        XCTAssertEqual(seams.panelAsked.count, 1)
        XCTAssertEqual(try text("five.sql"), "SELECT 5")
        XCTAssertFalse(model.tabs.contains { $0.id == tab.id })
    }

    func testTheQuestionNamesTheTabOrItsFile() throws {
        let plain = dirtyTab()
        seams.saveAnswers = [.discard]
        model.closeTab(plain.id)
        XCTAssertEqual(seams.saveAsked, [plain.title])
        XCTAssertEqual(AppModel.saveQuestion(for: "Query 3"), "Do you want to save the changes to “Query 3”?")

        let bound = try boundDirtyTab("named.sql")
        seams.saveAnswers = [.discard]
        model.closeTab(bound.id)
        XCTAssertEqual(seams.saveAsked.last, "named.sql")
    }

    // MARK: Nothing to ask

    func testGeneratedAndRestoredCleanTabsCloseWithNoQuestion() {
        model.newTab()
        let generated = model.selectedTab!
        generated.applyGeneratedSQL("SELECT * FROM public.t")
        model.newTab()
        let loaded = model.selectedTab!
        model.loadIntoEditor("SELECT from_history", in: loaded)
        var snapshot = SessionTab(QueryTab(title: "Restored"))
        snapshot.sql = "SELECT restored"
        snapshot.dirty = false
        let restored = snapshot.tab()
        model.tabs.append(restored)

        for tab in [generated, loaded, restored] { XCTAssertTrue(model.closeTab(tab.id)) }

        XCTAssertTrue(seams.saveAsked.isEmpty)
    }

    func testLoadingAHistoryStatementOverUnsavedTextOpensANewTab() {
        let typed = dirtyTab("SELECT mine")
        let before = model.tabs.count
        model.loadIntoEditor("SELECT from_history", in: typed)
        XCTAssertEqual(typed.sql, "SELECT mine", "unsaved text is not replaced")
        XCTAssertEqual(model.tabs.count, before + 1)
        XCTAssertEqual(model.selectedTab?.sql, "SELECT from_history")
        XCTAssertFalse(model.selectedTab?.hasUnsavedSQL ?? true, "the loaded statement is the app's text")
    }

    func testAnEmptyOrBlankTabClosesWithNoQuestion() {
        let blank = dirtyTab("SELECT 1")
        blank.sql = "   \n"
        XCTAssertTrue(model.closeTab(blank.id))
        let empty = model.selectedTab
        XCTAssertTrue(empty.map { model.closeTab($0.id) } ?? true)
        XCTAssertTrue(seams.saveAsked.isEmpty)
    }

    func testARestoredTabFromAnOldBlobIsUnsavedSoAnUpgradeNeverMarksWorkClean() {
        var old = SessionTab(QueryTab(title: "Old"))
        old.sql = "SELECT my_old_work"
        old.dirty = nil
        let tab = old.tab()
        model.tabs.append(tab)
        seams.saveAnswers = [.cancel]

        XCTAssertFalse(model.closeTab(tab.id))
        XCTAssertEqual(seams.saveAsked, ["Old"])
    }

    // MARK: Sets

    /// The five ways a tab closes, each given a set of three unsaved tabs (A, B, C in strip order) and
    /// the answers Save, Cancel, Save. A Cancel in the middle closes none, writes none, and stops asking.
    func testACancelInTheMiddleOfAnyCloseClosesNothingAndSavesNothing() {
        let paths: [(String, (AppModel, [QueryTab], UUID) -> Bool?)] = [
            ("closeAllTabs", { model, _, _ in model.closeAllTabs(); return nil }),
            ("closeOtherTabs", { model, tabs, _ in model.closeOtherTabs(keeping: tabs[0].id); return nil }),
            ("closeTabs(after:)", { model, tabs, _ in model.closeTabs(after: tabs[0].id); return nil }),
            ("closeTabs(of:)", { model, _, connection in model.closeTabs(of: connection) }),
        ]
        for (name, close) in paths {
            seams.restore()
            seams = FileSeams()
            model = AppModel()
            let connection = UUID()
            model.tabs = []
            // `closeOtherTabs` and `closeTabs(after:)` keep or skip the first tab, so give them a kept
            // one in front: the three doomed tabs are the last three in every path.
            model.newTab(connectionID: connection)
            let kept = model.selectedTab!
            let doomed = (1...3).map { index -> QueryTab in
                model.newTab(connectionID: connection)
                let tab = model.selectedTab!
                tab.sql = "SELECT \(index)"
                return tab
            }
            let all = [kept] + doomed
            // The kept tab is clean for the sets that include it as doomed (closeAllTabs, closeTabs(of:)).
            seams.saveAnswers = [.save, .cancel, .save]
            seams.panelAnswers = [directory.appendingPathComponent("\(name)-a.sql"),
                                  directory.appendingPathComponent("\(name)-c.sql")]

            let result = close(model, all, connection)

            XCTAssertEqual(model.tabs.count, 4, "\(name): nothing closed")
            XCTAssertEqual(seams.saveAsked.count, 2, "\(name): asked up to the Cancel and no further")
            XCTAssertTrue(seams.panelAsked.isEmpty, "\(name): no tab was saved before the set was settled")
            XCTAssertFalse(FileManager.default.fileExists(
                atPath: directory.appendingPathComponent("\(name)-a.sql").path), "\(name)")
            if let result { XCTAssertFalse(result, "\(name): closeConnection relies on false") }
            for tab in doomed { XCTAssertTrue(model.tabs.contains { $0 === tab }, "\(name)") }
        }
    }

    func testASetWhereEveryAnswerIsSaveWritesEachTabAndClosesThemAll() throws {
        model.tabs = []
        model.newTab(); let first = model.selectedTab!; first.sql = "SELECT 1"
        model.newTab(); let second = model.selectedTab!; second.sql = "SELECT 2"
        seams.saveAnswers = [.save, .save]
        seams.panelAnswers = [url("one.sql"), url("two.sql")]

        model.closeAllTabs()

        XCTAssertTrue(model.tabs.isEmpty)
        XCTAssertEqual(try text("one.sql"), "SELECT 1")
        XCTAssertEqual(try text("two.sql"), "SELECT 2")
    }

    func testDiscardedAndCleanTabsInASetCloseWithoutAnyWrite() {
        model.tabs = []
        model.newTab(); model.selectedTab!.sql = "SELECT 1"
        model.newTab(); model.selectedTab!.applyGeneratedSQL("SELECT generated")
        model.newTab(); model.selectedTab!.sql = "SELECT 3"
        seams.saveAnswers = [.discard, .discard]

        model.closeAllTabs()

        XCTAssertTrue(model.tabs.isEmpty)
        XCTAssertEqual(seams.saveAsked.count, 2, "the generated tab is not asked about")
        XCTAssertTrue(seams.panelAsked.isEmpty)
    }

    func testDecliningTheMixedEndingsQuestionInTheClosePromptClosesNothing() throws {
        let tab = try boundDirtyTab("mixed.sql", onDisk: "a\r\nb\nc\r\n", edited: "edited\n")
        seams.saveAnswers = [.save]
        seams.mixedAnswer = false

        XCTAssertFalse(model.closeTab(tab.id), "a decline is a Cancel for the close gate")

        XCTAssertTrue(model.tabs.contains { $0.id == tab.id })
        XCTAssertTrue(tab.hasUnsavedSQL)
        XCTAssertEqual(try text("mixed.sql"), "a\r\nb\nc\r\n")
        XCTAssertEqual(seams.mixedAsked.count, 1)
        XCTAssertNil(model.notice)
    }

    func testACancelledSavePanelClosesNothing() {
        model.tabs = []
        model.newTab(); let first = model.selectedTab!; first.sql = "SELECT 1"
        model.newTab(); let second = model.selectedTab!; second.sql = "SELECT 2"
        seams.saveAnswers = [.save, .discard]
        seams.panelAnswers = [nil]

        model.closeAllTabs()

        XCTAssertEqual(model.tabs.count, 2, "the discarded tab stays too: the whole set is aborted")
        XCTAssertEqual(first.sql, "SELECT 1")
        XCTAssertNil(model.notice, "a cancelled panel is not an error")
    }

    func testAFailedWriteClosesNothingAndSaysWhy() {
        let tab = dirtyTab("SELECT 1")
        seams.saveAnswers = [.save]
        seams.panelAnswers = [directory.appendingPathComponent("no-such-folder/x.sql")]

        XCTAssertFalse(model.closeTab(tab.id))

        XCTAssertTrue(model.tabs.contains { $0.id == tab.id })
        XCTAssertNotNil(model.notice, "an alert says why")
        XCTAssertEqual(tab.sql, "SELECT 1")
    }

    func testAnUnsavedTabAndStagedChangesAreTwoQuestionsNotOneEachOfEveryKind() {
        let tab = dirtyTab("SELECT 1")
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "0")
        seams.saveAnswers = [.discard]

        XCTAssertTrue(model.closeTab(tab.id))

        XCTAssertEqual(discardAsked.count, 1, "the staged-changes question is asked once for the set")
        XCTAssertEqual(discardAsked.first?.1, .closeTabs(1))
        XCTAssertEqual(seams.saveAsked.count, 1, "and one question for the unsaved tab")
    }

    func testDecliningTheStagedChangesQuestionAsksNothingElseAndWritesNothing() {
        let tab = dirtyTab("SELECT 1")
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "0")
        AppModel.confirmDiscard = { _, _ in false }
        seams.saveAnswers = [.save]
        seams.panelAnswers = [url("never.sql")]

        XCTAssertFalse(model.closeTab(tab.id))

        XCTAssertTrue(seams.saveAsked.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url("never.sql").path))
    }

    // MARK: Quit

    func testQuitStaysAsItWasAndAddsNoPrompt() {
        dirtyTab("SELECT 1")
        XCTAssertTrue(model.confirmQuit())
        XCTAssertTrue(seams.saveAsked.isEmpty, "the session keeps the text, as before")
    }

    // MARK: The defaults

    func testUnderTestTheDefaultsNeverOpenAModal() {
        seams.restore()
        defer { seams = FileSeams() }
        XCTAssertTrue(AppModel.underTest)
        XCTAssertEqual(AppModel.confirmSave("x"), .discard)
        XCTAssertNil(AppModel.chooseSaveURL("x.sql", nil))
        XCTAssertEqual(AppModel.chooseFileChange("x", true), .keepMine)
        XCTAssertFalse(AppModel.confirmOverwrite("x"))
        XCTAssertFalse(AppModel.confirmMixedEndings("x", .lf))
    }
}
