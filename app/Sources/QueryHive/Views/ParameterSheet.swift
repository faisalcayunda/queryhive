import SwiftUI

/// The values a `:name` statement needs, asked once per run.
///
/// It shows the statement that will run, with the values written in, because that text is the thing
/// being approved: the point of asking is that it is visible before it goes to the server. Nothing
/// is stored past the tab's own memory, so a value cannot be reused without being seen again.
struct ParameterSheet: View {
    @Environment(AppModel.self) private var model
    let prompt: ParameterPrompt

    @State private var entries: [String: ParameterEntry]

    init(prompt: ParameterPrompt) {
        self.prompt = prompt
        _entries = State(initialValue: prompt.initial)
    }

    /// What the values currently make. The Run button is the only thing that reads a failure, and
    /// the preview is where the reason is shown.
    private var rendered: Result<String, ParameterError> {
        ParameterRender.statement(prompt.template, entries: entries, driver: prompt.kind)
    }

    private var isReady: Bool {
        if case .success = rendered { return true }
        return false
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Query parameters")
                    .font(.ui(14, weight: .semibold))
                    .foregroundStyle(Tone.ink)
                Text("Each value is written into the statement as a checked literal. Nothing is "
                     + "bound — no driver can bind the statement this is for — so the text below is "
                     + "what will run, and it is the thing worth reading before it does.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 8) {
                ForEach(prompt.names, id: \.self) { name in
                    row(name)
                }
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("Statement")
                    .font(.ui(11, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                preview
            }

            HStack(spacing: 8) {
                Spacer(minLength: 0)
                PillButton(title: "Cancel", role: .quiet) { model.cancelParameters() }
                PillButton(title: "Run", symbol: "play.fill") { model.confirmParameters(entries) }
                    .disabled(!isReady)
            }
        }
        .padding(18)
        .frame(width: 620)
        .background(Tone.canvas)
    }

    /// One parameter: its name, what it is, and what it is.
    ///
    /// The type is chosen rather than inferred. A parameter has no column behind it, and guessing
    /// would be guessing at what the statement means.
    private func row(_ name: String) -> some View {
        let entry = entries[name] ?? .empty
        return HStack(spacing: 8) {
            Text(":\(name)")
                .font(.code(12))
                .foregroundStyle(Tone.ink)
                .frame(width: 128, alignment: .leading)
            Picker("", selection: Binding(
                get: { entry.kind },
                set: { entries[name] = ParameterEntry(kind: $0, text: entry.text) }
            )) {
                ForEach(ParameterKind.allCases) { kind in
                    Text(kind.title).tag(kind)
                }
            }
            .labelsHidden()
            .frame(width: 124)
            TextField("", text: Binding(
                get: { entry.text },
                set: { entries[name] = ParameterEntry(kind: entry.kind, text: $0) }
            ))
            .textFieldStyle(.plain)
            .font(.code(12))
            .foregroundStyle(Tone.ink)
            .disabled(entry.kind == .null)
            .opacity(entry.kind == .null ? 0.4 : 1)
        }
    }

    @ViewBuilder
    private var preview: some View {
        switch rendered {
        case .success(let sql):
            ScrollView {
                Text(sql)
                    .font(.code(11))
                    .foregroundStyle(Tone.ink.opacity(0.9))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .frame(maxHeight: 150)
            .background(Tone.recess.opacity(0.25), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        case .failure(let error):
            Text(error.message)
                .font(.ui(11))
                .foregroundStyle(Tone.coral)
                .fixedSize(horizontal: false, vertical: true)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Tone.coral.opacity(0.12),
                            in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
    }
}
