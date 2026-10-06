import AppKit
import SwiftUI

// The object tree as an `NSOutlineView` (W9-T3, blueprint §6): view-based rows, so the keyboard
// (arrows, type-select, Return) and VoiceOver (`AXOutline`, `AXOutlineRow`, level, disclosing) come
// from AppKit instead of being rebuilt on top of SwiftUI rows.
//
// The model stays the source of truth. Expansion is `TreeNode.expanded`, selection is
// `AppModel.selectedNodeID`, and the outline only reflects them: `OutlineSnapshot` flattens what is
// visible into plain values inside a SwiftUI `body`, so Observation re-runs that body when any of it
// changes, and `updateNSView` diffs the values against what is on screen.

// MARK: - Snapshot

/// One visible row, as a value.
struct OutlineRow: Equatable {
    enum Content: Equatable {
        case node(TreeNode.Kind)
        case loading
        case empty
        case error
    }

    var id: String
    var parentID: String?
    var depth: Int
    var content: Content
    /// The node's title, or the message text for a message row.
    var title: String
    var color: ConnectionColor
    var driver: ConnectionKind
    /// Whether the row's children are shown. Not the same as `TreeNode.expanded`: a filter opens
    /// every level it can see, and a load or a failure shows its message under a collapsed node.
    var open: Bool
    var expandable: Bool
    /// VoiceOver's value: a connection's state, otherwise what kind of object the row is.
    var value: String
    var help: String

    var nodeKind: TreeNode.Kind? {
        if case let .node(kind) = content { return kind }
        return nil
    }
}

struct OutlineSnapshot {
    var rows: [OutlineRow] = []
    var nodes: [String: TreeNode] = [:]
    var selectedID: String?

    /// Flattens the visible tree. `visible` is the filter's id set, `nil` for no filter. Cost is
    /// proportional to the open rows, not the whole tree: a collapsed node contributes one row.
    static func make(model: AppModel, visible: Set<String>?) -> OutlineSnapshot {
        var snap = OutlineSnapshot(selectedID: model.selectedNodeID)
        let filtering = visible != nil

        func message(_ content: OutlineRow.Content, _ text: String, under parent: TreeNode, depth: Int) {
            snap.rows.append(OutlineRow(
                id: "msg:\(parent.id):\(content)", parentID: parent.id, depth: depth, content: content,
                title: text, color: parent.color, driver: parent.connectionKind, open: false,
                expandable: false, value: "", help: ""))
        }

        func walk(_ node: TreeNode, depth: Int, parent: TreeNode?) {
            if let visible, !visible.contains(node.id) { return }
            let open = node.isExpandable && (filtering || node.expanded || node.loading || node.error != nil)
            let value: String
            if node.kind == .connection {
                value = model.connectionState(for: node.connectionID).label
            } else {
                value = node.kind.noun
            }
            snap.nodes[node.id] = node
            snap.rows.append(OutlineRow(
                id: node.id, parentID: parent?.id, depth: depth, content: .node(node.kind),
                title: node.title, color: node.color, driver: node.connectionKind, open: open,
                expandable: node.isExpandable, value: value, help: node.helpText))
            guard open else { return }
            if node.loading {
                message(.loading, "Loading…", under: node, depth: depth + 1)
            } else if let error = node.error {
                message(.error, error, under: node, depth: depth + 1)
            } else if let children = node.children {
                if children.isEmpty {
                    message(.empty, "Empty", under: node, depth: depth + 1)
                } else {
                    for child in children { walk(child, depth: depth + 1, parent: node) }
                }
            }
        }
        for node in model.tree { walk(node, depth: 0, parent: nil) }
        return snap
    }
}

// MARK: - SwiftUI side

/// The object list of the sidebar. A `View` rather than the representable itself so the snapshot is
/// built inside a `body`, where Observation is certain to track what it reads.
struct SchemaOutline: View {
    @Environment(AppModel.self) private var model
    /// Ids the filter keeps, or `nil` for none.
    let visible: Set<String>?

    var body: some View {
        OutlineRepresentable(snapshot: .make(model: model, visible: visible), model: model)
    }
}

private struct OutlineRepresentable: NSViewRepresentable {
    let snapshot: OutlineSnapshot
    let model: AppModel

    func makeCoordinator() -> OutlineCoordinator { OutlineCoordinator(model: model) }

    func makeNSView(context: Context) -> NSScrollView {
        context.coordinator.makeScrollView()
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        context.coordinator.apply(snapshot)
    }
}

// MARK: - AppKit side

final class OutlineItem: NSObject {
    let id: String
    var row: OutlineRow
    init(_ row: OutlineRow) {
        id = row.id
        self.row = row
    }
}

/// The private pasteboard type a dragged table carries beside its text: the node's id.
extension NSPasteboard.PasteboardType {
    static let treeNodeID = NSPasteboard.PasteboardType("com.queryhive.tree-node-id")
}

final class OutlineCoordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    let model: AppModel
    let scrollView = NSScrollView()
    let outline = OutlineView()
    private(set) var rows: [OutlineRow] = []
    private(set) var nodes: [String: TreeNode] = [:]
    private var items: [String: OutlineItem] = [:]
    private var childIDs: [String: [String]] = [:]
    private static let rootKey = ""
    /// True while the coordinator itself expands, collapses or selects, so the delegate callbacks
    /// that follow do not write back to the model they were just read from.
    private var syncing = false
    private var announced = Set<String>()

    init(model: AppModel) {
        self.model = model
        super.init()
    }

    func makeScrollView() -> NSScrollView {
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("tree"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .plain
        outline.backgroundColor = .clear
        outline.rowHeight = Metrics.treeRow
        outline.intercellSpacing = NSSize(width: 0, height: 1)
        outline.indentationPerLevel = Metrics.treeIndent
        outline.indentationMarkerFollowsCell = true
        outline.selectionHighlightStyle = .regular
        outline.allowsMultipleSelection = false
        outline.allowsEmptySelection = true
        outline.allowsTypeSelect = true
        outline.focusRingType = .none
        outline.autoresizesOutlineColumn = false
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        outline.setAccessibilityLabel("Object tree")
        outline.setDraggingSourceOperationMask(.copy, forLocal: false)
        outline.setDraggingSourceOperationMask(.copy, forLocal: true)
        outline.dataSource = self
        outline.delegate = self
        outline.target = self
        outline.doubleAction = #selector(doubleClicked)
        outline.menuProvider = { [weak self] row in self?.menu(forRow: row) }
        outline.openSelection = { [weak self] in self?.openSelected() }

        scrollView.documentView = outline
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.verticalScrollElasticity = .allowed
        scrollView.automaticallyAdjustsContentInsets = false
        scrollView.contentInsets = NSEdgeInsets(top: 5, left: 0, bottom: 5, right: 0)
        return scrollView
    }

    // MARK: Sync

    func apply(_ snapshot: OutlineSnapshot) {
        nodes = snapshot.nodes
        if snapshot.rows != rows {
            rows = snapshot.rows
            rebuildItems()
            syncing = true
            let origin = scrollView.contentView.bounds.origin
            outline.reloadData()
            // Pre-order, so a parent is open before its children are asked to be.
            for row in rows where row.open && row.expandable {
                if let item = items[row.id] { outline.expandItem(item) }
            }
            scrollView.contentView.scroll(to: origin)
            syncing = false
            announceNewErrors()
        }
        syncSelection(snapshot.selectedID)
    }

    private func rebuildItems() {
        var fresh: [String: OutlineItem] = [:]
        childIDs = [:]
        for row in rows {
            // Identity is kept by id: `NSOutlineView` compares items by object.
            let item = items[row.id] ?? OutlineItem(row)
            item.row = row
            fresh[row.id] = item
            childIDs[row.parentID ?? Self.rootKey, default: []].append(row.id)
        }
        items = fresh
    }

    private func syncSelection(_ id: String?) {
        let current = outline.selectedRow >= 0 ? (outline.item(atRow: outline.selectedRow) as? OutlineItem)?.id : nil
        guard current != id else { return }
        syncing = true
        defer { syncing = false }
        if let id, let item = items[id], item.row.nodeKind != nil {
            let row = outline.row(forItem: item)
            if row >= 0 {
                outline.selectRowIndexes([row], byExtendingSelection: false)
                outline.scrollRowToVisible(row)
                return
            }
        }
        outline.deselectAll(nil)
    }

    /// A failed load is announced once, when its row appears, rather than left for VoiceOver to find.
    private func announceNewErrors() {
        let errors = rows.filter { $0.content == .error }
        for row in errors where announced.insert(row.id).inserted {
            NSAccessibility.post(element: outline, notification: .announcementRequested,
                                 userInfo: [.announcement: row.title, .priority: NSAccessibilityPriorityLevel.high])
        }
        announced.formIntersection(errors.map(\.id))
    }

    // MARK: Data source

    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        childIDs[(item as? OutlineItem)?.id ?? Self.rootKey]?.count ?? 0
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        items[childIDs[(item as? OutlineItem)?.id ?? Self.rootKey]![index]]!
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        (item as? OutlineItem)?.row.expandable ?? false
    }

    /// Only tables can be dragged. The text is the qualified, driver-quoted name, which
    /// `NSTextView` drops at the pointer by itself, so the editor needs no code for it.
    func outlineView(_ outlineView: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
        guard let item = item as? OutlineItem, let node = nodes[item.id], let text = node.insertableText
        else { return nil }
        let board = NSPasteboardItem()
        board.setString(text, forType: .string)
        board.setString(node.id, forType: .treeNodeID)
        return board
    }

    // MARK: Delegate

    func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
        guard let item = item as? OutlineItem else { return nil }
        let id = NSUserInterfaceItemIdentifier("cell")
        let cell = outlineView.makeView(withIdentifier: id, owner: self) as? OutlineCellView
            ?? { let c = OutlineCellView(); c.identifier = id; return c }()
        cell.configure(item.row, actions: item.row.nodeKind == nil ? nil : OutlineCellView.Actions(
            open: { [weak self] in if let node = self?.nodes[item.id] { self?.model.openNode(node) } },
            refresh: { [weak self] in if let node = self?.nodes[item.id] { self?.model.refresh(node) } }))
        return cell
    }

    func outlineView(_ outlineView: NSOutlineView, rowViewForItem item: Any) -> NSTableRowView? {
        let id = NSUserInterfaceItemIdentifier("row")
        return outlineView.makeView(withIdentifier: id, owner: self) as? OutlineRowView
            ?? { let r = OutlineRowView(); r.identifier = id; return r }()
    }

    func outlineView(_ outlineView: NSOutlineView, heightOfRowByItem item: Any) -> CGFloat {
        guard let row = (item as? OutlineItem)?.row else { return Metrics.treeRow }
        switch row.content {
        case .node: return Metrics.treeRow
        case .loading, .empty: return 18
        case .error:
            // Two lines at most, like the SwiftUI row: sized from the text so a long reason wraps
            // instead of being clipped. Measured at reload, so a resize waits for the next one.
            let width = max(60, outlineView.bounds.width - CGFloat(row.depth + 1) * Metrics.treeIndent - 40)
            let bounds = (row.title as NSString).boundingRect(
                with: NSSize(width: width, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin], attributes: [.font: OutlineCellView.font(11)])
            return min(2, max(1, ceil(bounds.height / 14))) * 14 + 4
        }
    }

    func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        (item as? OutlineItem)?.row.nodeKind != nil
    }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !syncing else { return }
        let row = outline.selectedRow
        model.selectedNodeID = row >= 0 ? (outline.item(atRow: row) as? OutlineItem)?.id : nil
    }

    func outlineViewItemDidExpand(_ notification: Notification) {
        outline.syncChevrons()
        guard !syncing, let item = notification.userInfo?["NSObject"] as? OutlineItem,
              let node = nodes[item.id] else { return }
        model.expand(node)
    }

    func outlineViewItemDidCollapse(_ notification: Notification) {
        outline.syncChevrons()
        guard !syncing, let item = notification.userInfo?["NSObject"] as? OutlineItem,
              let node = nodes[item.id] else { return }
        node.expanded = false
    }

    func outlineView(_ outlineView: NSOutlineView, typeSelectStringFor tableColumn: NSTableColumn?,
                     item: Any) -> String? {
        guard let row = (item as? OutlineItem)?.row, row.nodeKind != nil else { return nil }
        return row.title
    }

    // MARK: Actions

    @objc private func doubleClicked() {
        let row = outline.clickedRow
        guard row >= 0, let item = outline.item(atRow: row) as? OutlineItem,
              let node = nodes[item.id] else { return }
        model.openNode(node)
    }

    /// Return does what a double-click does, on the selected row.
    private func openSelected() {
        let row = outline.selectedRow
        guard row >= 0, let item = outline.item(atRow: row) as? OutlineItem,
              let node = nodes[item.id] else { return }
        model.openNode(node)
    }

    /// The context menu for the row under the pointer. It does not move the selection, as the
    /// SwiftUI menu did not.
    private func menu(forRow row: Int) -> NSMenu? {
        guard row >= 0, let item = outline.item(atRow: row) as? OutlineItem,
              let node = nodes[item.id] else { return nil }
        return Self.menu(from: TreeMenu.items(for: node, in: model))
    }

    static func menu(from items: [TreeMenuItem]) -> NSMenu {
        let result = NSMenu()
        result.autoenablesItems = false
        for item in items {
            if item.isSeparator { result.addItem(.separator()); continue }
            let entry = MenuAction(title: item.title, handler: item.action)
            entry.state = item.checked ? .on : .off
            if let sub = item.submenu { entry.submenu = menu(from: sub) }
            result.addItem(entry)
        }
        return result
    }
}

/// A menu item that runs a closure, so a menu is built from `TreeMenuItem` data and nothing else.
final class MenuAction: NSMenuItem {
    private let handler: (() -> Void)?

    init(title: String, handler: (() -> Void)?) {
        self.handler = handler
        super.init(title: title, action: handler == nil ? nil : #selector(fire), keyEquivalent: "")
        target = self
    }

    required init(coder: NSCoder) { fatalError("not decoded") }

    @objc private func fire() { handler?() }
}

// MARK: - Outline view

final class OutlineView: NSOutlineView {
    var menuProvider: ((Int) -> NSMenu?)?
    var openSelection: (() -> Void)?

    /// The pill and the content both sit 6 pt in from the column's edges, as the SwiftUI rows did.
    static let gutter: CGFloat = 6

    override func keyDown(with event: NSEvent) {
        // Return and keypad Enter. ⌘/⌥-modified Return is not ours.
        if (event.keyCode == 36 || event.keyCode == 76),
           event.modifierFlags.intersection([.command, .option, .control]).isEmpty {
            openSelection?()
            return
        }
        super.keyDown(with: event)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        menuProvider?(row(at: convert(event.locationInWindow, from: nil)))
    }

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        redrawSelection()
        return ok
    }

    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        redrawSelection()
        return ok
    }

    private func redrawSelection() {
        for row in selectedRowIndexes { rowView(atRow: row, makeIfNecessary: false)?.needsDisplay = true }
    }

    /// A chevron of the same size and weight the SwiftUI tree drew, in place of the system triangle.
    override func makeView(withIdentifier identifier: NSUserInterfaceItemIdentifier,
                           owner: Any?) -> NSView? {
        if identifier == NSOutlineView.disclosureButtonIdentifier {
            let button = ChevronButton()
            button.target = self
            button.action = #selector(chevronClicked(_:))
            return button
        }
        return super.makeView(withIdentifier: identifier, owner: owner)
    }

    /// AppKit hands the custom button back untouched (no target, state off), so the outline wires it:
    /// a click opens or closes the row, and `syncChevrons` keeps its state true.
    @objc private func chevronClicked(_ sender: NSButton) {
        let row = self.row(for: sender)
        guard row >= 0, let item = item(atRow: row) else { return }
        if isItemExpanded(item) { collapseItem(item) } else { expandItem(item) }
        syncChevrons()
    }

    func syncChevrons() {
        enumerateAvailableRowViews { rowView, _ in (rowView as? OutlineRowView)?.syncChevron() }
    }

    override func frameOfOutlineCell(atRow row: Int) -> NSRect {
        var frame = super.frameOfOutlineCell(atRow: row)
        frame.size = NSSize(width: 11, height: 11)
        frame.origin.x = CGFloat(level(forRow: row)) * indentationPerLevel + Self.gutter
        frame.origin.y += (rect(ofRow: row).height - 11) / 2
        return frame
    }

    override func frameOfCell(atColumn column: Int, row: Int) -> NSRect {
        var frame = super.frameOfCell(atColumn: column, row: row)
        // Chevron box 11 + 5 spacing, after the level's indentation and the gutter.
        let x = CGFloat(level(forRow: row)) * indentationPerLevel + Self.gutter + 16
        frame.size.width = max(0, frame.maxX - x - Self.gutter)
        frame.origin.x = x
        return frame
    }
}

final class ChevronButton: NSButton {
    private static let config = NSImage.SymbolConfiguration(pointSize: 8, weight: .bold)
    private static let closed = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)?
        .withSymbolConfiguration(config)
    private static let opened = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)?
        .withSymbolConfiguration(config)

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: 11, height: 11))
        isBordered = false
        imagePosition = .imageOnly
        image = Self.closed
        contentTintColor = NSColor(Tone.secondary)
        focusRingType = .none
        setAccessibilityLabel("Disclose")
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    func setOpen(_ open: Bool) {
        state = open ? .on : .off
        image = open ? Self.opened : Self.closed
    }
}

// MARK: - Row view

/// Draws the selection and hover pills itself, so the look is the one the SwiftUI rows had: a
/// 6 pt rounded fill, ink 0.13 selected and 0.06 under the pointer. The one visible change (V-5):
/// while the outline is the first responder, the selected pill also carries a 1.5 pt ring.
final class OutlineRowView: NSTableRowView {
    private var hovering = false { didSet { if hovering != oldValue { needsDisplay = true } } }

    /// The chevron points down while the row is open. AppKit does not set the state of a custom
    /// disclosure button, so the row does it whenever it lays out.
    func syncChevron() {
        guard let outline = superview as? NSOutlineView,
              let button = subviews.first(where: { $0 is ChevronButton }) as? NSButton else { return }
        let row = outline.row(for: self)
        guard row >= 0, let item = outline.item(atRow: row) else { return }
        (button as? ChevronButton)?.setOpen(outline.isItemExpanded(item))
    }

    override func layout() {
        super.layout()
        syncChevron()
    }

    override var isEmphasized: Bool {
        get { false }
        set {}
    }

    /// Text stays ink on the pill instead of flipping to white.
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }

    private var pill: NSBezierPath {
        NSBezierPath(roundedRect: bounds.insetBy(dx: OutlineView.gutter, dy: 0.5), xRadius: 6, yRadius: 6)
    }

    override func drawBackground(in dirtyRect: NSRect) {
        guard hovering, !isSelected else { return }
        NSColor(Tone.ink).withAlphaComponent(0.06).setFill()
        pill.fill()
    }

    override func drawSelection(in dirtyRect: NSRect) {
        NSColor(Tone.ink).withAlphaComponent(0.13).setFill()
        pill.fill()
        guard let window, let outline = superview as? NSOutlineView,
              window.firstResponder === outline else { return }
        let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: OutlineView.gutter + 1, dy: 1.5),
                                xRadius: 5, yRadius: 5)
        ring.lineWidth = 1.5
        // ponytail: the accent stands in for `Tone.focusRing` until W9-T1 lands that token.
        NSColor(Tone.accent).setStroke()
        ring.stroke()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow],
                                       owner: self, userInfo: nil))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }
    override func prepareForReuse() {
        super.prepareForReuse()
        hovering = false
    }
}

// MARK: - Cell view

final class OutlineCellView: NSTableCellView {
    struct Actions {
        var open: () -> Void
        var refresh: () -> Void
    }

    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    private let spinner = NSProgressIndicator()
    private var tile: NSHostingView<AnyView>?
    private let stack = NSStackView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        icon.imageScaling = .scaleProportionallyDown
        label.lineBreakMode = .byTruncatingMiddle
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        spinner.style = .spinning
        spinner.controlSize = .mini
        spinner.isIndeterminate = true
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 5
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(icon)
        stack.addArrangedSubview(spinner)
        stack.addArrangedSubview(label)
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
        ])
        textField = label
    }

    required init?(coder: NSCoder) { fatalError("not decoded") }

    /// The chrome's UI font at a size, honouring the family the user picked in Settings.
    static func font(_ size: CGFloat, weight: Font.Weight = .regular) -> NSFont {
        let family = ThemeStore.shared.uiFontFamily
        if !family.isEmpty, let font = FontChoice.resolve(family, size: size, weight: weight) { return font }
        let ns: NSFont.Weight = weight == .semibold ? .semibold : .regular
        return .systemFont(ofSize: size, weight: ns)
    }

    func configure(_ row: OutlineRow, actions: Actions?) {
        tile?.removeFromSuperview()
        tile = nil
        spinner.stopAnimation(nil)
        spinner.isHidden = true
        icon.isHidden = false
        toolTip = row.help.isEmpty ? nil : row.help

        switch row.content {
        case let .node(kind):
            label.stringValue = row.title
            label.font = Self.font(12, weight: kind == .connection ? .semibold : .regular)
            label.textColor = NSColor(Tone.ink)
            label.maximumNumberOfLines = 1
            label.lineBreakMode = .byTruncatingMiddle
            if kind == .connection {
                icon.isHidden = true
                let host = NSHostingView(rootView: AnyView(
                    connectionTile(colour: row.color, kind: row.driver, size: 16)))
                host.translatesAutoresizingMaskIntoConstraints = false
                host.widthAnchor.constraint(equalToConstant: 16).isActive = true
                host.heightAnchor.constraint(equalToConstant: 16).isActive = true
                stack.insertArrangedSubview(host, at: 0)
                tile = host
            } else {
                icon.image = symbol(kind.symbol, 10.5)
                icon.contentTintColor = Self.tint(kind)
            }
            setAccessibilityLabel(row.title)
            setAccessibilityValue(row.value)
            setAccessibilityHelp(row.help)
            if let actions {
                setAccessibilityCustomActions([
                    NSAccessibilityCustomAction(name: "Open", handler: { actions.open(); return true }),
                    NSAccessibilityCustomAction(name: "Refresh", handler: { actions.refresh(); return true }),
                ])
            }
        case .loading:
            label.stringValue = row.title
            icon.isHidden = true
            spinner.isHidden = false
            spinner.startAnimation(nil)
            messageStyle(tint: NSColor(Tone.secondary), lines: 1)
            setAccessibilityLabel(row.title)
        case .empty:
            label.stringValue = row.title
            icon.image = symbol("tray", 9)
            icon.contentTintColor = NSColor(Tone.ink).withAlphaComponent(0.3)
            messageStyle(tint: NSColor(Tone.ink).withAlphaComponent(0.3), lines: 1)
            setAccessibilityLabel(row.title)
        case .error:
            label.stringValue = row.title
            icon.image = symbol("exclamationmark.triangle.fill", 9)
            icon.contentTintColor = NSColor(Tone.coral)
            messageStyle(tint: NSColor(Tone.coral), lines: 2)
            setAccessibilityLabel(row.title)
        }
    }

    private func messageStyle(tint: NSColor, lines: Int) {
        label.font = Self.font(11)
        label.textColor = tint
        label.lineBreakMode = .byWordWrapping
        label.maximumNumberOfLines = lines
        label.setAccessibilityValue(nil)
        setAccessibilityValue(nil)
        setAccessibilityHelp(nil)
        setAccessibilityCustomActions([])
    }

    private func symbol(_ name: String, _ size: CGFloat) -> NSImage? {
        NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: size, weight: .medium))
    }

    private static func tint(_ kind: TreeNode.Kind) -> NSColor {
        switch kind {
        case .group, .connection, .table: NSColor(Tone.secondary)
        case .catalog: NSColor(Tone.violet)
        case .database: NSColor(Tone.ice)
        case .schema: NSColor(Tone.amber)
        }
    }
}
