import SwiftUI

/// The Settings window, reached with ⌘,.
///
/// One pane, because the app has one thing worth configuring here. The shortcut table is drawn
/// rather than described: a scheme is a promise about which key does what, and a promise the user
/// cannot read is one they cannot check.
struct SettingsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                SectionLabel(text: "Keyboard")
                Text("Choose whose keys this app answers to. Every binding below comes from the "
                     + "scheme's own definitions, so the two never drift apart.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Picker("", selection: $model.shortcutScheme) {
                ForEach(ShortcutScheme.allCases) { scheme in
                    Text(scheme.title).tag(scheme)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            Text(model.shortcutScheme.detail)
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Divider().overlay(.white.opacity(0.08))

            bindingTable
        }
        .padding(20)
        .frame(width: 460)
        .background(Tone.canvas)
    }

    /// Every action the app has, with the key the current scheme gives it.
    ///
    /// An unbound row says so instead of being hidden. A missing shortcut is something the user
    /// needs to know about — it is why "Run" under DBeaver is ⌘↩ and not the ⌘R they are reaching
    /// for — and silently omitting the row would make the scheme look complete when it is not.
    private var bindingTable: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(ShortcutAction.allCases) { action in
                HStack(spacing: 8) {
                    Text(action.title)
                        .font(.system(size: 11.5))
                        .foregroundStyle(.white.opacity(0.9))
                    Spacer(minLength: 12)
                    if let shortcut = model.shortcutScheme.shortcut(for: action) {
                        Text(shortcut.display)
                            .font(.system(size: 11, weight: .medium, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.85))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Color.white.opacity(0.07),
                                        in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    } else {
                        Text("not bound")
                            .font(.system(size: 10.5))
                            .foregroundStyle(Tone.secondary.opacity(0.7))
                    }
                }
                .padding(.vertical, 3)
            }
        }
    }
}
