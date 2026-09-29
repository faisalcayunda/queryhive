import AppKit
import SwiftUI

/// The editor's find bar.
///
/// An AppKit view rather than a SwiftUI overlay because it sits inside the editor's own container,
/// above the text view, and pushes the text down the way a native find bar does. It owns no search
/// logic: every keystroke and button press is reported through a closure, and the editor does the
/// searching. That split is what lets the search itself be tested without a window.
final class SQLFindBar: NSView, NSSearchFieldDelegate {
    private let searchField = NSSearchField()
    private let replaceField = NSTextField()
    private let countLabel = NSTextField(labelWithString: "")
    private let caseToggle = NSButton()
    private let wholeWordToggle = NSButton()
    private let previousButton = NSButton()
    private let nextButton = NSButton()
    private let replaceToggle = NSButton()
    private let closeButton = NSButton()
    private let replaceButton = NSButton(title: "Replace", target: nil, action: nil)
    private let replaceAllButton = NSButton(title: "Replace All", target: nil, action: nil)
    private let replaceRow = NSStackView()

    var onQueryChanged: ((String) -> Void)?
    var onFindNext: (() -> Void)?
    var onFindPrevious: (() -> Void)?
    var onReplace: (() -> Void)?
    var onReplaceAll: (() -> Void)?
    var onClose: (() -> Void)?

    /// The search field's text.
    var query: String { searchField.stringValue }
    /// The replacement field's text.
    var replacement: String { replaceField.stringValue }

    /// What the search should ignore.
    var options: FindReplace.Options {
        var options: FindReplace.Options = []
        if caseToggle.state == .on { options.insert(.caseSensitive) }
        if wholeWordToggle.state == .on { options.insert(.wholeWord) }
        return options
    }

    /// Whether the replace row is showing. The container is told to re-lay-out, because a find bar
    /// that is tall enough for one row and then asked to draw two would clip the second.
    var isReplacing = false {
        didSet {
            guard isReplacing != oldValue else { return }
            replaceToggle.state = isReplacing ? .on : .off
            replaceRow.isHidden = !isReplacing
            superview?.needsLayout = true
        }
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("SQLFindBar is only created in code")
    }

    // MARK: Building

    private func build() {
        searchField.placeholderString = "Find"
        searchField.sendsWholeSearchString = false
        searchField.sendsSearchStringImmediately = true
        searchField.delegate = self
        searchField.font = .systemFont(ofSize: 12)
        searchField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        searchField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        searchField.toolTip = "Find in the query (⌘F)"

        replaceField.placeholderString = "Replace with"
        replaceField.font = .systemFont(ofSize: 12)
        replaceField.delegate = self
        replaceField.setContentHuggingPriority(.defaultLow, for: .horizontal)
        replaceField.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        countLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        countLabel.textColor = Tone.readoutNS
        countLabel.alignment = .right
        countLabel.setContentHuggingPriority(.required, for: .horizontal)
        countLabel.setContentCompressionResistancePriority(.required, for: .horizontal)
        countLabel.widthAnchor.constraint(greaterThanOrEqualToConstant: 64).isActive = true

        style(toggle: caseToggle, title: "Aa", help: "Match case")
        style(toggle: wholeWordToggle, title: "W", help: "Whole words only")
        caseToggle.action = #selector(optionsChanged)
        wholeWordToggle.action = #selector(optionsChanged)

        style(symbol: previousButton, "chevron.up", help: "Previous match (⇧⌘G)", action: #selector(previousPressed))
        style(symbol: nextButton, "chevron.down", help: "Next match (⌘G)", action: #selector(nextPressed))
        style(symbol: closeButton, "xmark", help: "Close find (esc)", action: #selector(closePressed))

        replaceToggle.title = "Replace"
        replaceToggle.setButtonType(.toggle)
        replaceToggle.bezelStyle = .recessed
        replaceToggle.font = .systemFont(ofSize: 11)
        replaceToggle.toolTip = "Show replace controls (⌥⌘F)"
        replaceToggle.target = self
        replaceToggle.action = #selector(replaceToggled)

        for button in [replaceButton, replaceAllButton] {
            button.bezelStyle = .rounded
            button.font = .systemFont(ofSize: 11.5)
            button.target = self
        }
        replaceButton.action = #selector(replacePressed)
        replaceAllButton.action = #selector(replaceAllPressed)
        replaceButton.toolTip = "Replace the current match"
        replaceAllButton.toolTip = "Replace every match"

        let row = NSStackView(views: [searchField, countLabel, previousButton, nextButton,
                                      caseToggle, wholeWordToggle, replaceToggle, closeButton])
        row.orientation = .horizontal
        row.spacing = 6
        row.alignment = .centerY
        row.distribution = .fill

        replaceRow.setViews([replaceField, replaceButton, replaceAllButton], in: .leading)
        replaceRow.orientation = .horizontal
        replaceRow.spacing = 6
        replaceRow.alignment = .centerY
        replaceRow.distribution = .fill
        replaceRow.isHidden = true

        let stack = NSStackView(views: [row, replaceRow])
        stack.orientation = .vertical
        stack.spacing = 6
        // `.width` makes both rows span the bar, so the search field's low hugging priority is what
        // decides how much room it takes rather than the row shrinking to fit.
        stack.alignment = .width
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 7),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -7),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
        ])
    }

    private func style(toggle button: NSButton, title: String, help: String) {
        button.title = title
        button.setButtonType(.toggle)
        button.bezelStyle = .recessed
        button.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
        button.toolTip = help
        button.target = self
    }

    private func style(symbol button: NSButton, _ name: String, help: String, action: Selector) {
        button.image = NSImage(systemSymbolName: name, accessibilityDescription: help)
        button.imagePosition = .imageOnly
        button.isBordered = false
        button.bezelStyle = .texturedRounded
        button.contentTintColor = Tone.readoutNS
        button.toolTip = help
        button.target = self
        button.action = action
    }

    // MARK: State

    func setQuery(_ value: String) {
        searchField.stringValue = value
    }

    /// Put the caret in the search field, selected, the way ⌘F does in every app.
    func focusSearch() {
        window?.makeFirstResponder(searchField)
        searchField.currentEditor()?.selectAll(nil)
    }

    /// The count readout. An empty query shows nothing rather than "No matches", because nothing
    /// has been searched for yet.
    func setResult(count: Int, current: Int?) {
        let hasQuery = !query.isEmpty
        countLabel.stringValue = hasQuery ? FindReplace.summary(count: count, index: current) : ""
        countLabel.textColor = count == 0 && hasQuery ? NSColor(Tone.coral) : Tone.readoutNS
    }

    // MARK: Actions

    @objc private func optionsChanged() {
        onQueryChanged?(query)
    }

    @objc private func previousPressed() { onFindPrevious?() }
    @objc private func nextPressed() { onFindNext?() }
    @objc private func closePressed() { onClose?() }

    @objc private func replaceToggled() {
        isReplacing = replaceToggle.state == .on
    }

    @objc private func replacePressed() { onReplace?() }
    @objc private func replaceAllPressed() { onReplaceAll?() }

    // MARK: Delegates

    func controlTextDidChange(_ notification: Notification) {
        if (notification.object as? NSTextField) === searchField {
            onQueryChanged?(query)
        }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) {
            if control === searchField {
                let shift = NSApp.currentEvent?.modifierFlags.contains(.shift) ?? false
                shift ? onFindPrevious?() : onFindNext?()
            } else {
                onReplace?()
            }
            return true
        }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
            onClose?()
            return true
        }
        return false
    }

    // MARK: Drawing

    /// The bar has no fill of its own beyond a wash and a hairline: it sits on the editor box's own
    /// surface, and the container has already pushed the text below it, so nothing shows through.
    override func draw(_ dirtyRect: NSRect) {
        Tone.inkNS(0.03).setFill()
        bounds.fill()
        Tone.inkNS(0.10).setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
    }
}
