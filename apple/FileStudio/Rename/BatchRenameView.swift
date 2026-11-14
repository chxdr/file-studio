// BatchRenameView.swift — the "Batch Rename N Items…" sheet: the steps on
// top, the live preview below, presets in the header, Cancel / Rename at
// the bottom. Keyboard: ⌃⌘R opens it; Tab moves between fields; ⌥⌘1…⌥⌘7
// add a step; ⏎ renames; ⎋ cancels.

import SwiftUI

struct BatchRenameView: View {
    @Bindable var model: BatchRenameModel
    let cancel: () -> Void
    let rename: () -> Void
    let savePreset: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.horizontal, 20)
                .padding(.top, 18)
                .padding(.bottom, 10)
            stepsList
                .frame(height: stepsHeight)
            Divider()
            RenamePreviewTable(session: model.session, summary: model.summary)
                .frame(minHeight: 200)
            Divider()
            footer
                .padding(.horizontal, 20)
                .padding(.vertical, 14)
        }
        .frame(minWidth: 720, idealWidth: 820, minHeight: 560, idealHeight: 640)
        .background(addStepShortcuts)
        .onDisappear { model.stopPolling() }
    }

    /// Tall enough for the steps (two-line steps are taller), up to a limit;
    /// the preview gets the rest.
    private var stepsHeight: CGFloat {
        let h = model.steps.reduce(12) { $0 + ([.replace, .sequence, .insert].contains($1.kind) ? 92 : 62) }
        return min(300, max(84, CGFloat(h)))
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.itemCount == 1 ? "Rename 1 Item" : "Rename \(model.itemCount) Items")
                    .font(.headline)
                Text("Steps run from top to bottom. The preview updates as you type.")
                    .font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            Menu {
                if model.presets.isEmpty {
                    Text("No Saved Presets")
                }
                ForEach(model.presets, id: \.name) { p in
                    Button(p.name) { model.load(p) }
                }
                Divider()
                Button("Save as Preset…", action: savePreset)
                if !model.presets.isEmpty {
                    Menu("Delete Preset") {
                        ForEach(model.presets, id: \.name) { p in
                            Button(p.name) { model.deletePreset(p.name) }
                        }
                    }
                }
            } label: {
                Label("Presets", systemImage: "square.stack")
            }
            .fixedSize()
            .accessibilityLabel("Presets")
            addStepMenu
        }
    }

    private var addStepMenu: some View {
        Menu {
            ForEach(Array(RenameStepKind.allCases.enumerated()), id: \.element) { i, k in
                Button { model.add(k) } label: { Label(k.title, systemImage: k.symbol) }
                    .keyboardShortcut(KeyEquivalent(Character("\(i + 1)")), modifiers: [.command, .option])
            }
        } label: {
            Label("Add Step", systemImage: "plus")
        }
        .fixedSize()
        .accessibilityLabel("Add Step")
    }

    /// ⌥⌘1…⌥⌘7 work while the menu is closed too.
    private var addStepShortcuts: some View {
        ZStack {
            ForEach(Array(RenameStepKind.allCases.enumerated()), id: \.element) { i, k in
                Button("") { model.add(k) }
                    .keyboardShortcut(KeyEquivalent(Character("\(i + 1)")), modifiers: [.command, .option])
            }
        }
        .opacity(0)
        .accessibilityHidden(true)
    }

    // MARK: Steps

    private var stepsList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 8) {
                    ForEach(Array(model.steps.enumerated()), id: \.element.id) { i, s in
                        StepCard(model: model, id: s.id, index: i, count: model.steps.count,
                                 error: model.summary.errorStep.map(Int.init) == i ? model.summary.error : nil)
                            .id(s.id)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 6)
            }
            .onChange(of: model.focusStep) { _, id in
                if let id { withAnimation { proxy.scrollTo(id, anchor: .bottom) } }
            }
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                if let e = model.summary.error {
                    Label(e, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                } else {
                    Text(model.status).foregroundStyle(.secondary).monospacedDigit()
                }
                if model.summary.exifLoading {
                    HStack(spacing: 6) {
                        ProgressView(value: Double(model.summary.exifDone),
                                     total: Double(max(1, model.summary.exifTotal)))
                            .frame(width: 120)
                        Text("Reading EXIF \(model.summary.exifDone) of \(model.summary.exifTotal)…")
                            .foregroundStyle(.secondary)
                    }
                }
                if let n = model.notice {
                    Label(n, systemImage: "exclamationmark.circle").foregroundStyle(.red)
                }
            }
            .font(.callout)
            .lineLimit(2)
            Spacer()
            Button("Cancel", action: cancel).keyboardShortcut(.cancelAction)
            Button(model.applyTitle, action: rename)
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canApply)
        }
    }
}

// MARK: - One step

private struct StepCard: View {
    @Bindable var model: BatchRenameModel
    /// The step, by id: an index could be stale for a moment after a removal.
    let id: UUID
    let index: Int
    let count: Int
    let error: String?
    @FocusState private var focused: Bool

    private var current: RenameStep {
        model.steps.first { $0.id == id } ?? RenameStep(kind: .template)
    }

    private var step: Binding<RenameStep> {
        Binding(
            get: { current },
            set: { new in
                if let i = model.steps.firstIndex(where: { $0.id == id }) { model.steps[i] = new }
            })
    }

    var body: some View {
        let s = current
        HStack(alignment: .top, spacing: 10) {
            Toggle("", isOn: step.enabled)
                .toggleStyle(.checkbox)
                .labelsHidden()
                .help(s.enabled ? "Turn this step off" : "Turn this step on")
                .padding(.top, 4)
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Picker("Step", selection: step.kind) {
                        ForEach(RenameStepKind.allCases) { k in
                            Label(k.title, systemImage: k.symbol).tag(k)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                    fields(s)
                }
                options(s)
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                }
            }
            .disabled(!s.enabled)
            .opacity(s.enabled ? 1 : 0.55)
            Spacer(minLength: 0)
            ControlGroup {
                Button { model.move(s.id, by: -1) } label: { Image(systemName: "chevron.up") }
                    .disabled(index == 0)
                    .help("Move Up")
                Button { model.move(s.id, by: 1) } label: { Image(systemName: "chevron.down") }
                    .disabled(index == count - 1)
                    .help("Move Down")
                Button { model.remove(s.id) } label: { Image(systemName: "minus") }
                    .help("Remove Step")
            }
            .controlSize(.small)
            .fixedSize()
        }
        .padding(10)
        .background(.background.secondary, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(error == nil ? Color.primary.opacity(0.06) : .orange.opacity(0.6))
        )
        .onAppear { if model.focusStep == s.id { focused = true } }
        .onChange(of: model.focusStep) { _, id in if id == s.id { focused = true } }
    }

    // The main fields, on the first line.
    @ViewBuilder private func fields(_ s: RenameStep) -> some View {
        switch s.kind {
        case .template:
            TextField("Template", text: step.template, prompt: Text("{name} {n:3}"))
                .focused($focused)
                .font(.body.monospaced())
            tokenMenu { step.wrappedValue.template += $0 }
        case .replace:
            TextField("Find", text: step.find, prompt: Text("Find"))
                .focused($focused)
            Image(systemName: "arrow.right").foregroundStyle(.secondary)
            TextField("Replace with", text: step.replace, prompt: Text(s.regex ? "Replace ($1 for groups)" : "Replace with"))
        case .changeCase:
            Picker("Case", selection: step.caseKind) {
                ForEach(RenameCase.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            .labelsHidden().fixedSize()
            .focused($focused)
            targetPicker
        case .sequence:
            LabeledField("Start", value: step.start).focused($focused)
            LabeledField("Step", value: step.step)
            Stepper("Digits: \(s.padding)", value: step.padding, in: 0...12).fixedSize()
        case .trim:
            Picker("Remove", selection: step.trimMode) {
                ForEach(RenameTrimMode.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            .labelsHidden().fixedSize()
            .focused($focused)
            switch s.trimMode {
            case .start, .end:
                Stepper("\(s.count)", value: step.count, in: 0...255).fixedSize()
            case .characters:
                TextField("Characters", text: step.chars, prompt: Text("_-"))
                    .frame(width: 90)
            case .whitespace:
                EmptyView()
            }
            targetPicker
        case .insert:
            TextField("Text", text: step.text, prompt: Text("Text or {tokens}"))
                .focused($focused)
            tokenMenu { step.wrappedValue.text += $0 }
        case .ext:
            Picker("Extension", selection: step.extChange) {
                ForEach(RenameExtChange.allCases, id: \.self) { Text($0.title).tag($0) }
            }
            .labelsHidden().fixedSize()
            .focused($focused)
            if s.extChange == .set {
                TextField("New extension", text: step.ext, prompt: Text("jpg"))
                    .frame(width: 90)
            }
        }
    }

    // Options, on a second line where a step has them.
    @ViewBuilder private func options(_ s: RenameStep) -> some View {
        switch s.kind {
        case .replace:
            HStack(spacing: 14) {
                Toggle("Regular expression", isOn: step.regex)
                Toggle("Ignore case", isOn: step.ignoreCase)
                Toggle("First match only", isOn: step.firstOnly)
                targetPicker
            }
            .toggleStyle(.checkbox)
            .controlSize(.small)
        case .sequence:
            HStack(spacing: 10) {
                positionPicker
                TextField("Separator", text: step.separator, prompt: Text("Separator"))
                    .frame(width: 80)
                    .help("Between the number and the name")
            }
            .controlSize(.small)
        case .insert:
            HStack(spacing: 10) {
                positionPicker
                targetPicker
            }
            .controlSize(.small)
        default:
            EmptyView()
        }
    }

    private var targetPicker: some View {
        Picker("In", selection: step.target) {
            ForEach(RenameTarget.allCases, id: \.self) { Text($0.title).tag($0) }
        }
        .fixedSize()
    }

    @ViewBuilder private var positionPicker: some View {
        Picker("Where", selection: step.position) {
            ForEach(RenamePosition.allCases, id: \.self) { Text($0.title).tag($0) }
        }
        .fixedSize()
        if step.wrappedValue.position == .index {
            Stepper("\(step.wrappedValue.index)", value: step.index, in: 0...255).fixedSize()
        }
    }

    private func tokenMenu(_ insert: @escaping (String) -> Void) -> some View {
        Menu {
            ForEach(model.tokens, id: \.token) { t in
                Button { insert(t.token) } label: {
                    Text(t.token).font(.body.monospaced()) + Text("  \(t.title)").foregroundStyle(.secondary)
                }
            }
        } label: {
            Image(systemName: "curlybraces.square")
        }
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Insert Token")
        .accessibilityLabel("Insert Token")
    }
}

/// "Start [ 1 ]": a label and a small number field.
private struct LabeledField: View {
    let title: String
    @Binding var value: Int64

    init(_ title: String, value: Binding<Int64>) {
        self.title = title
        _value = value
    }

    var body: some View {
        HStack(spacing: 4) {
            Text(title).foregroundStyle(.secondary)
            TextField(title, value: $value, format: .number.grouping(.never))
                .frame(width: 60)
                .multilineTextAlignment(.trailing)
        }
    }
}
