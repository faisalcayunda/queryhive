import AppKit
import XCTest

@testable import QueryHive

/// The grid's keyboard, one row of the blueprint's table at a time (W10 §3.1). Pure: no window, no
/// `NSEvent` except where a test says it is checking the conversion itself.
final class GridKeyMapTests: XCTestCase {

    /// A grid with a cursor on one cell and nothing else going on.
    private let ready = GridKeyState(hasCursor: true, hasSelection: true, selectionIsBlock: false,
                                     peekOpen: false, editing: false)

    private func action(_ code: GridKey.Code, shift: Bool = false, command: Bool = false,
                        option: Bool = false, control: Bool = false,
                        in state: GridKeyState? = nil) -> GridKeyAction? {
        GridKeyMap.action(for: GridKey(code, shift: shift, command: command, option: option,
                                       control: control),
                          in: state ?? ready)
    }

    // MARK: Arrows

    func testAnArrowMovesOneCellAndCollapsesTheBlock() {
        XCTAssertEqual(action(.left), .move(.step(rows: 0, columns: -1), extending: false))
        XCTAssertEqual(action(.right), .move(.step(rows: 0, columns: 1), extending: false))
        XCTAssertEqual(action(.up), .move(.step(rows: -1, columns: 0), extending: false))
        XCTAssertEqual(action(.down), .move(.step(rows: 1, columns: 0), extending: false))
    }

    func testShiftAndAnArrowExtends() {
        XCTAssertEqual(action(.down, shift: true), .move(.step(rows: 1, columns: 0), extending: true))
        XCTAssertEqual(action(.left, shift: true), .move(.step(rows: 0, columns: -1), extending: true))
    }

    func testCommandAndAnArrowJumpsToTheEdge() {
        XCTAssertEqual(action(.left, command: true), .move(.toEdge(.left), extending: false))
        XCTAssertEqual(action(.right, command: true), .move(.toEdge(.right), extending: false))
        XCTAssertEqual(action(.up, command: true), .move(.toEdge(.top), extending: false))
        XCTAssertEqual(action(.down, command: true), .move(.toEdge(.bottom), extending: false))
    }

    func testCommandShiftAndAnArrowExtendsToTheEdge() {
        XCTAssertEqual(action(.down, shift: true, command: true), .move(.toEdge(.bottom), extending: true))
        XCTAssertEqual(action(.right, shift: true, command: true), .move(.toEdge(.right), extending: true))
    }

    /// Option and Control with an arrow are nobody's binding here: no action, and the table
    /// swallows the arrow all the same (`GridKey.isArrow`).
    func testOptionAndControlArrowsAreNotMapped() {
        XCTAssertNil(action(.down, option: true))
        XCTAssertNil(action(.left, control: true))
        XCTAssertTrue(GridKey(.down, option: true).isArrow)
        XCTAssertFalse(GridKey(.pageDown).isArrow)
    }

    // MARK: Pages, Home and End

    func testPageKeysMoveAPageAndShiftExtends() {
        XCTAssertEqual(action(.pageDown), .move(.page(down: true), extending: false))
        XCTAssertEqual(action(.pageUp), .move(.page(down: false), extending: false))
        XCTAssertEqual(action(.pageDown, shift: true), .move(.page(down: true), extending: true))
        XCTAssertNil(action(.pageDown, command: true))
    }

    func testHomeAndEndGoToTheRowEndsAndWithCommandToTheCorners() {
        XCTAssertEqual(action(.home), .move(.rowStart, extending: false))
        XCTAssertEqual(action(.end), .move(.rowEnd, extending: false))
        XCTAssertEqual(action(.home, command: true), .move(.gridStart, extending: false))
        XCTAssertEqual(action(.end, command: true), .move(.gridEnd, extending: false))
        XCTAssertEqual(action(.end, shift: true), .move(.rowEnd, extending: true))
    }

    // MARK: Tab

    func testTabAndShiftTabWalkTheCellsAndNeverExtend() {
        XCTAssertEqual(action(.tab), .move(.next, extending: false))
        XCTAssertEqual(action(.tab, shift: true), .move(.previous, extending: false))
        // Command-Tab is the app switcher and Control-Tab the tab strip (`TabKeyRouter`).
        XCTAssertNil(action(.tab, command: true))
        XCTAssertNil(action(.tab, control: true))
        XCTAssertNil(action(.tab, option: true))
    }

    // MARK: Return and Space

    func testReturnActsOnTheCursorOnlyWhenThereIsOne() {
        XCTAssertEqual(action(.returnKey), .activate)
        var empty = ready
        empty.hasCursor = false
        XCTAssertNil(action(.returnKey, in: empty))
        XCTAssertNil(action(.returnKey, shift: true))
        XCTAssertNil(action(.returnKey, command: true))
    }

    func testSpaceTogglesThePeekOnlyWhenThereIsACursor() {
        XCTAssertEqual(action(.space), .togglePeek)
        var empty = ready
        empty.hasCursor = false
        XCTAssertNil(action(.space, in: empty))
        XCTAssertNil(action(.space, shift: true))
        XCTAssertNil(action(.space, command: true))
    }

    // MARK: Escape, one step per tap

    func testEscapeUndoesOneThingAtATime() {
        var state = ready
        state.editing = true
        state.peekOpen = true
        state.selectionIsBlock = true
        XCTAssertEqual(action(.escape, in: state), .cancelEdit, "the edit first")

        state.editing = false
        XCTAssertEqual(action(.escape, in: state), .closePeek, "then the peek")

        state.peekOpen = false
        XCTAssertEqual(action(.escape, in: state), .collapseSelection, "then the block, to the cursor")

        state.selectionIsBlock = false
        XCTAssertEqual(action(.escape, in: state), .clearSelection, "then the selection")

        state.hasSelection = false
        XCTAssertNil(action(.escape, in: state), "with nothing left, the key belongs to someone else")
    }

    // MARK: Delete

    func testDeleteMarksRowsOnlyWhenTheGridAllowsIt() {
        XCTAssertNil(action(.deleteBackward), "no gesture to mark rows exists before W10-T3")
        var editable = ready
        editable.canDeleteRows = true
        XCTAssertEqual(action(.deleteBackward, in: editable), .deleteRows)
        XCTAssertEqual(action(.deleteForward, in: editable), .deleteRows)
        XCTAssertNil(action(.deleteBackward, command: true, in: editable), "Command-Delete is the footer's")
        editable.hasSelection = false
        XCTAssertNil(action(.deleteBackward, in: editable))
    }

    // MARK: Editing owns the keyboard

    func testWhileEditingOnlyEscapeIsTheGrids() {
        var state = ready
        state.editing = true
        for code: GridKey.Code in [.left, .right, .up, .down, .pageUp, .pageDown, .home, .end,
                                   .returnKey, .tab, .space, .deleteBackward] {
            XCTAssertNil(action(code, in: state), "\(code) belongs to the editor field")
        }
        XCTAssertEqual(action(.escape, in: state), .cancelEdit)
    }

    // MARK: From an event

    private func event(_ keyCode: UInt16, _ flags: NSEvent.ModifierFlags = []) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                                       windowNumber: 0, context: nil, characters: "",
                                       charactersIgnoringModifiers: "", isARepeat: false,
                                       keyCode: keyCode))
    }

    func testAnEventBecomesAKeyByItsHardwareCodeAndModifiers() throws {
        XCTAssertEqual(GridKey(event: try event(125, [.shift, .numericPad, .function])),
                       GridKey(.down, shift: true), "the arrow's own flags are not modifiers")
        XCTAssertEqual(GridKey(event: try event(126, [.command, .shift])),
                       GridKey(.up, shift: true, command: true))
        XCTAssertEqual(GridKey(event: try event(36)), GridKey(.returnKey))
        XCTAssertEqual(GridKey(event: try event(76)), GridKey(.returnKey), "keypad Enter is Return")
        XCTAssertEqual(GridKey(event: try event(49)), GridKey(.space))
        XCTAssertEqual(GridKey(event: try event(53)), GridKey(.escape))
        XCTAssertEqual(GridKey(event: try event(48, [.shift])), GridKey(.tab, shift: true))
        XCTAssertEqual(GridKey(event: try event(116)), GridKey(.pageUp))
        XCTAssertEqual(GridKey(event: try event(115)), GridKey(.home))
        XCTAssertEqual(GridKey(event: try event(117)), GridKey(.deleteForward))
    }

    /// Letters and digits are nothing to the grid: typing to edit is out of scope, and so are the
    /// menu's own keys, which must reach the responder chain untouched.
    func testLettersDigitsAndCommandKeysAreNotKeys() throws {
        XCTAssertNil(GridKey(event: try event(0)), "a")
        XCTAssertNil(GridKey(event: try event(18)), "1")
        XCTAssertNil(GridKey(event: try event(8, [.command])), "Command-C")
        XCTAssertNil(GridKey(event: try event(0, [.command])), "Command-A")
    }
}
