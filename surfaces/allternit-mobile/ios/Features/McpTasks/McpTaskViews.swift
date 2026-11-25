import SwiftUI
import SafariServices

/// Task card, "orb and beam": the thinking orb, a status line and a beam that
/// fills with progress (or sweeps while the length is unknown). Mirrors the
/// web McpTaskCard. Stop cancels the task on its server.
struct McpTaskCardView: View {
    let taskId: String
    @ObservedObject private var store = McpTasksStore.shared
    @ReducedMotion private var reduceMotion
    @State private var stopping = false
    @State private var stopError: String? = nil

    var body: some View {
        if let task = store.tasks[taskId] {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    leading(task)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(task.displayTitle)
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(2)
                        statusLine(task)
                    }
                    Spacer(minLength: 8)
                    if !task.isFinished {
                        Button {
                            stop(task)
                        } label: {
                            if stopping { ProgressView().controlSize(.small) } else { Text("Stop") }
                        }
                        .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                        .disabled(stopping)
                        .accessibilityLabel("Stop \(task.displayTitle)")
                    }
                }
                if task.isRunning || task.isInputNeeded {
                    Beam(fraction: task.progress?.fraction, paused: task.isInputNeeded, reduced: reduceMotion)
                        .frame(height: 3)
                        .accessibilityHidden(true)
                }
                if let label = task.progress?.label, task.isRunning {
                    Text(label).font(.caption).foregroundColor(Color("TextSecondary"))
                }
                if let source = task.source, !source.isEmpty {
                    Text("Runs on \(source)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                if task.isFailed, let text = task.errorText, !text.isEmpty {
                    Text(text).font(.caption).foregroundColor(.red)
                }
                if let stopError {
                    Text(stopError).font(.caption).foregroundColor(.red)
                }
                // The question this task is waiting on, in place.
                ForEach(openForms(for: task)) { form in
                    McpElicitationFormView(request: form)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
            .animation(Motion.pick(Motion.deck, reduced: reduceMotion), value: task.status)
            .accessibilityElement(children: .contain)
        }
    }

    private func openForms(for task: McpTaskPart) -> [McpElicitationPart] {
        store.elicitations.values
            .filter { $0.taskId == task.taskId && !store.isResolved($0.id) }
            .sorted { $0.id < $1.id }
    }

    @ViewBuilder
    private func leading(_ task: McpTaskPart) -> some View {
        if task.isDone {
            Image(systemName: "checkmark.circle.fill").font(.title3).foregroundColor(Color("TextSecondary"))
        } else if task.isFailed {
            Image(systemName: "exclamationmark.circle.fill").font(.title3).foregroundColor(.red)
        } else if task.isCancelled {
            Image(systemName: "stop.circle").font(.title3).foregroundColor(Color("TextSecondary"))
        } else {
            ThinkingOrb(state: task.isInputNeeded ? .listening : .working, size: .inline, paused: task.isInputNeeded)
                .frame(width: 24, height: 24)
        }
    }

    private func statusLine(_ task: McpTaskPart) -> some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            HStack(spacing: 6) {
                Text(statusText(task))
                if let elapsed = elapsed(task, now: context.date) {
                    Text("·")
                    Text(elapsed).monospacedDigit()
                }
            }
            .font(.caption)
            .foregroundColor(task.isInputNeeded ? Color("TextPrimary") : Color("TextSecondary"))
        }
    }

    private func statusText(_ task: McpTaskPart) -> String {
        if task.isInputNeeded { return "Needs your input" }
        if task.isDone { return "Done" }
        if task.isFailed { return "Failed" }
        if task.isCancelled { return "Stopped" }
        if let message = task.statusMessage, !message.isEmpty { return message }
        return "Running"
    }

    private func elapsed(_ task: McpTaskPart, now: Date) -> String? {
        guard let start = task.startedAt else { return nil }
        let end = task.isFinished ? (task.updatedAt ?? now.timeIntervalSince1970 * 1000) : now.timeIntervalSince1970 * 1000
        let seconds = max(0, Int((end - start) / 1000))
        return seconds >= 3600
            ? String(format: "%d:%02d:%02d", seconds / 3600, (seconds % 3600) / 60, seconds % 60)
            : String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    private func stop(_ task: McpTaskPart) {
        Haptics.light()
        stopping = true
        stopError = nil
        Task { @MainActor in
            do {
                try await McpTasksClient.shared.cancelTask(id: task.taskId)
            } catch {
                stopError = "Could not stop this task. Try again."
                Haptics.error()
            }
            stopping = false
        }
    }
}

/// Thin progress beam: fills to `fraction`, or a soft segment sweeps when the
/// length is unknown. Static when motion is reduced or the task is waiting.
private struct Beam: View {
    let fraction: Double?
    let paused: Bool
    let reduced: Bool
    @State private var sweep = false

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Color("FillNeutral"))
                if let fraction {
                    Capsule().fill(Color("TextPrimary").opacity(0.75))
                        .frame(width: geo.size.width * fraction)
                        .animation(Motion.pick(Motion.deck, reduced: reduced), value: fraction)
                } else {
                    let width = geo.size.width * 0.3
                    Capsule().fill(Color("TextPrimary").opacity(paused ? 0.25 : 0.6))
                        .frame(width: width)
                        .offset(x: reduced || paused ? 0 : (sweep ? geo.size.width - width : 0))
                        .animation(reduced || paused ? nil : .easeInOut(duration: 1.2).repeatForever(autoreverses: true), value: sweep)
                }
            }
            .clipShape(Capsule())
        }
        .onAppear { sweep = true }
    }
}

// MARK: - Form

/// A server's question, answered in place. Form mode shows the requested
/// fields; URL mode opens the page in a Safari sheet. Form mode never
/// collects secrets — a secret-looking field turns the card into a pointer
/// to the server's own page and leaves only Decline.
struct McpElicitationFormView: View {
    let request: McpElicitationPart
    @ObservedObject private var store = McpTasksStore.shared
    @ReducedMotion private var reduceMotion
    @State private var text: [String: String] = [:]
    @State private var flags: [String: Bool] = [:]
    @State private var busy: String? = nil
    @State private var error: String? = nil
    @State private var page: IdentifiableURL? = nil

    var body: some View {
        if !store.isResolved(request.id) {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 6) {
                    Image(systemName: "questionmark.bubble.fill").font(.caption)
                    Text(request.source.map { "\($0) is asking" } ?? "The server is asking")
                        .font(.caption.weight(.semibold))
                }
                .foregroundColor(Color("TextSecondary"))
                if !request.message.isEmpty {
                    Text(request.message)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                if request.mode == .url { urlBody } else { formBody }
                if let error {
                    Text(error).font(.caption).foregroundColor(.red)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgPrimary"))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
            .sheet(item: $page) { item in
                SafariView(url: item.url).ignoresSafeArea()
            }
            .onAppear(perform: seedDefaults)
        }
    }

    // URL mode

    @ViewBuilder
    private var urlBody: some View {
        HStack(spacing: 6) {
            Button("Decline") { send("decline") }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
            if let url = request.url {
                Text(url.host ?? "").font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(1)
                Spacer(minLength: 0)
                Button("Open page") {
                    Haptics.light()
                    page = IdentifiableURL(url: url)
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                Button("Done") { send("accept") }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
            }
        }
        .disabled(busy != nil)
    }

    // Form mode

    @ViewBuilder
    private var formBody: some View {
        if request.hasSecretField {
            Text("This asks for a secret. Allternit never collects secrets in a form. Use the server's own sign-in page or a connector instead.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Button("Decline") { send("decline") }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                .disabled(busy != nil)
        } else {
            ForEach(request.fields) { field in
                fieldView(field)
            }
            HStack(spacing: 6) {
                Button("Decline") { send("decline") }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                Spacer(minLength: 0)
                Button {
                    send("accept")
                } label: {
                    if busy == "accept" { ProgressView().controlSize(.small) } else { Text("Send") }
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                .disabled(!canSubmit)
            }
            .disabled(busy != nil)
        }
    }

    @ViewBuilder
    private func fieldView(_ field: McpFormField) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(field.required ? field.title : "\(field.title) (optional)")
                .font(.caption.weight(.medium))
                .foregroundColor(Color("TextSecondary"))
            switch field.kind {
            case .boolean:
                Toggle(field.detail ?? field.title, isOn: Binding(
                    get: { flags[field.key] ?? false },
                    set: { flags[field.key] = $0; Haptics.selection() }
                ))
                .font(.subheadline)
            case .choice:
                Picker(field.title, selection: Binding(
                    get: { text[field.key] ?? "" },
                    set: { text[field.key] = $0; Haptics.selection() }
                )) {
                    if !field.required { Text("None").tag("") }
                    ForEach(field.options, id: \.value) { Text($0.label).tag($0.value) }
                }
                .pickerStyle(.menu)
                .labelsHidden()
            default:
                TextField(field.detail ?? field.title, text: Binding(
                    get: { text[field.key] ?? "" },
                    set: { text[field.key] = $0 }
                ), axis: field.multiline ? .vertical : .horizontal)
                .keyboardType(field.kind == .integer ? .numberPad : (field.kind == .number ? .decimalPad : .default))
                .textFieldStyle(.roundedBorder)
                .lineLimit(field.multiline ? 3...6 : 1...1)
            }
            if field.kind != .boolean, let detail = field.detail, !detail.isEmpty {
                Text(detail).font(.caption2).foregroundColor(Color("TextSecondary"))
            }
        }
    }

    private var canSubmit: Bool {
        request.fields.allSatisfy { field in
            guard field.required else { return true }
            if field.kind == .boolean { return true }
            return !(text[field.key] ?? "").trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    private func seedDefaults() {
        for field in request.fields where text[field.key] == nil && flags[field.key] == nil {
            if field.kind == .boolean { flags[field.key] = field.defaultText == "true" }
            else if let value = field.defaultText { text[field.key] = value }
            else if field.kind == .choice, field.required, let first = field.options.first { text[field.key] = first.value }
        }
    }

    private func content() -> [String: McpJSON] {
        var out: [String: McpJSON] = [:]
        for field in request.fields {
            switch field.kind {
            case .boolean: out[field.key] = .bool(flags[field.key] ?? false)
            case .number, .integer:
                if let raw = text[field.key], let n = Double(raw) { out[field.key] = .number(n) }
            default:
                if let raw = text[field.key], !raw.isEmpty { out[field.key] = .string(raw) }
            }
        }
        return out
    }

    private func send(_ action: String) {
        busy = action
        error = nil
        Haptics.light()
        let body = (action == "accept" && request.mode == .form) ? content() : nil
        Task { @MainActor in
            do {
                try await McpTasksClient.shared.replyElicitation(id: request.id, action: action, content: body)
                withAnimation(Motion.pick(Motion.fade, reduced: reduceMotion)) {
                    store.markResolved(id: request.id, action: action)
                }
                Haptics.success()
            } catch {
                self.error = "Could not send your answer. Try again."
                Haptics.error()
            }
            busy = nil
        }
    }
}
