import SwiftUI
import UIKit

/// The card under the bar: one step at a time (apps, tools, a blank's
/// picker, review, live job, receipt). Theme tokens: white + neutral fill,
/// graphite in dark mode (no tan).
struct IslandBarCard: View {
    @ObservedObject var model: IslandBarModel
    let onClose: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let error = model.errorText {
                errorRow(error)
            }
            switch model.step {
            case .apps: appsList
            case .tools: toolsList
            case .blanks: blanks
            case .review: reviewCard
            case .live: liveCard
            case .done: doneCard
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Theme.bgPrimary))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Theme.borderWarmDefault))
        .shadow(color: .black.opacity(0.25), radius: 20, y: 16)
        .accessibilityIdentifier("islandBarCard")
    }

    // MARK: Apps

    @ViewBuilder private var appsList: some View {
        if model.loading && model.apps.isEmpty {
            loadingRow("Loading your apps")
        } else if model.apps.isEmpty, !model.noComputer {
            Text("No connected apps yet. Connect one under Connections.")
                .font(.footnote).foregroundColor(Theme.textSecondary)
        } else {
            Text("Which app?").font(.subheadline.weight(.semibold)).foregroundColor(Theme.textPrimary)
            ScrollView {
                VStack(spacing: 2) {
                    ForEach(model.matchingApps, id: \.id) { app in
                        row(title: app.name, detail: ConnectionWording.subtitle(app), enabled: app.isRunnable) {
                            Task { await model.pick(app: app) }
                        }
                    }
                }
            }
            .frame(maxHeight: 280)
        }
    }

    // MARK: Tools

    private var toolsList: some View {
        VStack(alignment: .leading, spacing: 2) {
            header("What should it do in \(model.app?.name ?? "it")?")
            ForEach(model.tools, id: \.name) { tool in
                row(title: tool.title, detail: tool.readOnly ? "Reads only" : nil, enabled: true) {
                    Task { await model.pick(tool: tool) }
                }
            }
        }
    }

    // MARK: Blanks

    @ViewBuilder private var blanks: some View {
        if let intent = model.intent,
           let key = model.focusedBlank, let blank = intent.blanks.first(where: { $0.key == key }) {
            header(IslandBarWording.question(blank))
            IslandBlankPicker(blank: blank, recents: intent.recents[key] ?? [],
                              value: model.values[key], appName: intent.app.name) { model.set($0, for: key) }
                .id(key)
        } else if model.intent?.blanks.isEmpty == true {
            Text("Nothing to fill in.").font(.footnote).foregroundColor(Theme.textSecondary)
        }
        HStack {
            Button("Back") { model.back() }.buttonStyle(.bordered)
            Spacer()
            Button {
                Task { await model.goReview() }
            } label: {
                if model.loading { ProgressView() } else { Text("Review") }
            }
            .buttonStyle(.borderedProminent)
            .tint(Theme.textPrimary)
            .disabled(!model.canReview || model.loading)
            .accessibilityIdentifier("islandBarReview")
        }
    }

    // MARK: Review

    @ViewBuilder private var reviewCard: some View {
        if let review = model.review {
            header(model.intent.map { "\($0.toolTitle) in \($0.app.name)" } ?? "Review")
            VStack(alignment: .leading, spacing: 4) {
                ForEach(review.lines, id: \.self) { line in
                    HStack(alignment: .firstTextBaseline) {
                        Text(line.label).font(.footnote).foregroundColor(Theme.textSecondary)
                        Spacer()
                        Text(line.value).font(.footnote.weight(.medium)).foregroundColor(Theme.textPrimary)
                            .multilineTextAlignment(.trailing)
                    }
                }
                if !review.level.isEmpty {
                    HStack {
                        Text("Level").font(.footnote).foregroundColor(Theme.textSecondary)
                        Spacer()
                        Text(ConnectionWording.level(review.level)).font(.footnote.weight(.medium))
                            .foregroundColor(Theme.textPrimary)
                    }
                }
            }
            if review.ruleText != nil || IslandBarWording.approverLine(review) != nil {
                HStack(alignment: .top, spacing: 7) {
                    Image(systemName: "checkmark.shield").font(.footnote).foregroundColor(Theme.textPrimary)
                    VStack(alignment: .leading, spacing: 2) {
                        if let rule = review.ruleText {
                            // Rule text is shown as sent (CONTRACT section 4).
                            Text(rule).font(.caption).foregroundColor(Theme.textPrimary)
                        }
                        if let line = IslandBarWording.approverLine(review) {
                            Text(line).font(.caption).foregroundColor(Theme.textSecondary)
                        }
                    }
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 9).fill(Theme.textPrimary.opacity(0.06)))
            }
            HStack {
                Button("Back") { model.back() }.buttonStyle(.bordered)
                Spacer()
                Button {
                    Haptics.medium()
                    Task { await model.run() }
                } label: {
                    if model.loading { ProgressView() } else {
                        Text(review.approverIsYou || review.approverName == nil ? "Run" : "Send for approval")
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(Theme.textPrimary)
                .disabled(model.loading)
                .accessibilityIdentifier("islandBarRun")
            }
        }
    }

    // MARK: Live

    private var liveCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                IslandStatusIcon(state: model.jobState ?? "working", size: 34)
                VStack(alignment: .leading, spacing: 2) {
                    Text(model.stateLine).font(.subheadline.weight(.semibold)).foregroundColor(Theme.textPrimary)
                    if let line = model.progressLine {
                        Text(line).font(.caption).foregroundColor(Theme.textSecondary).lineLimit(2)
                    } else if model.jobState == "input_required" {
                        Text("Answer it in Tasks.").font(.caption).foregroundColor(Theme.textSecondary)
                    }
                }
            }
            IslandStepLine(state: model.jobState ?? "working")
            HStack {
                Button("Stop", role: .destructive) { Task { await model.stop() } }.buttonStyle(.bordered)
                Spacer()
                Button("Hide") { onClose() }.buttonStyle(.bordered)
            }
        }
    }

    private var doneCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                IslandStatusIcon(state: model.jobState ?? "completed", size: 34)
                Text(model.stateLine)
                    .font(.headline)
                    .foregroundColor(Theme.textPrimary)
                    .accessibilityIdentifier("islandBarReceipt")
            }
            HStack {
                Button("New") { model.reset() }.buttonStyle(.bordered)
                Spacer()
                Button("Done") { model.reset(); onClose() }
                    .buttonStyle(.borderedProminent).tint(Theme.textPrimary)
            }
        }
    }

    // MARK: Pieces

    private func header(_ text: String) -> some View {
        Text(text).font(.subheadline.weight(.semibold)).foregroundColor(Theme.textPrimary)
    }

    private func row(title: String, detail: String?, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.selection()
            action()
        } label: {
            HStack {
                Text(title).font(.subheadline).foregroundColor(enabled ? Theme.textPrimary : Theme.textSecondary)
                Spacer()
                if let detail { Text(detail).font(.caption).foregroundColor(Theme.textSecondary) }
            }
            .padding(.vertical, 8).padding(.horizontal, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled || model.loading)
    }

    private func loadingRow(_ text: String) -> some View {
        HStack(spacing: 8) {
            ProgressView()
            Text(text).font(.footnote).foregroundColor(Theme.textSecondary)
        }
        .accessibilityElement(children: .combine)
    }

    private func errorRow(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(text).font(.footnote.weight(.medium)).foregroundColor(Theme.statusError)
            if model.noComputer {
                Text(CalmCopy.noComputerDetail).font(.caption).foregroundColor(Theme.textSecondary)
            }
        }
    }
}

/// Asked, Approval, Running, Done (the Live Activity's step line).
struct IslandStepLine: View {
    let state: String

    var body: some View {
        let reached = TaskActivityAttributes.ContentState(state: state).stepsReached
        let tint = IslandStatusIcon.color(for: state)
        VStack(spacing: 3) {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(tint.opacity(0.25)).frame(height: 3)
                    Capsule().fill(tint).frame(width: geo.size.width * CGFloat(reached) / 4, height: 3)
                }
            }
            .frame(height: 3)
            HStack {
                ForEach(TaskActivityAttributes.stepLabels, id: \.self) { label in
                    Text(label).font(.caption2).foregroundColor(Theme.textSecondary)
                    if label != TaskActivityAttributes.stepLabels.last { Spacer() }
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(reached) of 4")
    }
}

// MARK: - Blank pickers (one per blank kind)

struct IslandBlankPicker: View {
    let blank: BarBlankInfo
    let recents: [IslandRecent]
    let value: BarValue?
    let appName: String
    let onPick: (BarValue?) -> Void

    @State private var text = ""
    @State private var date = Date()
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !recents.isEmpty, blank.type != "boolean" {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(recents, id: \.self) { r in
                            Button(r.label) { Haptics.selection(); onPick(r.value) }
                                .font(.caption)
                                .padding(.horizontal, 9).padding(.vertical, 4)
                                .background(Capsule().fill(Theme.textPrimary.opacity(value == r.value ? 0.14 : 0.06)))
                                .foregroundColor(Theme.textPrimary)
                                .buttonStyle(.plain)
                        }
                    }
                }
                .accessibilityLabel("Recent values")
            }
            picker
        }
        .onAppear { text = value.map { $0.display } ?? "" }
    }

    @ViewBuilder private var picker: some View {
        if blank.pickerKind == "mcp_app" || blank.type == "details" {
            // The app's own form (MCP App UI) does not run in the bar yet.
            Text("This needs \(appName)'s own form. Open \(appName) on your computer to fill it in.")
                .font(.footnote).foregroundColor(Theme.textSecondary)
        } else {
            switch blank.type {
            case "enum":
                VStack(spacing: 2) {
                    ForEach(blank.options, id: \.self) { option in
                        Button {
                            Haptics.selection()
                            onPick(.text(option.value))
                        } label: {
                            HStack {
                                Text(option.label).font(.subheadline).foregroundColor(Theme.textPrimary)
                                Spacer()
                                if value == .text(option.value) {
                                    Image(systemName: "checkmark").font(.footnote.weight(.semibold))
                                        .foregroundColor(Theme.textPrimary)
                                }
                            }
                            .padding(.vertical, 7).padding(.horizontal, 8)
                            .background(RoundedRectangle(cornerRadius: 8)
                                .fill(value == .text(option.value) ? Theme.textPrimary.opacity(0.06) : .clear))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            case "boolean":
                HStack {
                    ForEach([true, false], id: \.self) { b in
                        Button(b ? "Yes" : "No") { Haptics.selection(); onPick(.bool(b)) }
                            .buttonStyle(.bordered)
                            .tint(value == .bool(b) ? Theme.textPrimary : Theme.textSecondary)
                    }
                }
            case "date":
                DatePicker(blank.label, selection: $date, displayedComponents: .date)
                    .onChange(of: date) { _, d in onPick(.text(Self.isoDate(d))) }
            default:
                HStack {
                    TextField(blank.label, text: $text)
                        .keyboardType(keyboard)
                        .textInputAutocapitalization(blank.type == "text" ? .sentences : .never)
                        .autocorrectionDisabled(blank.type != "text")
                        .focused($focused)
                        .submitLabel(.done)
                        .onSubmit(commit)
                        .onAppear { focused = true }
                    if let unit = blank.unit, !unit.isEmpty {
                        Text(unit).font(.footnote).foregroundColor(Theme.textSecondary)
                    }
                    Button("Set", action: commit).buttonStyle(.bordered)
                        .disabled(text.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding(8)
                .background(RoundedRectangle(cornerRadius: 9).fill(Theme.textPrimary.opacity(0.06)))
            }
        }
    }

    private var keyboard: UIKeyboardType {
        switch blank.type {
        case "number": return .decimalPad
        case "integer": return .numberPad
        case "email": return .emailAddress
        case "uri": return .URL
        default: return .default
        }
    }

    private func commit() {
        if let v = DoWording.parse(text, for: blank) {
            Haptics.selection()
            onPick(v)
        }
    }

    static func isoDate(_ d: Date) -> String {
        let f = DateFormatter()
        f.calendar = Calendar(identifier: .iso8601)
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: d)
    }
}

extension IslandBarWording {
    /// The picker's question: "Which order?", "How much, in EUR?".
    static func question(_ blank: BarBlankInfo) -> String {
        DoWording.ask(blank)
    }
}
