import SwiftUI

/// Answer an agent's question in place — port of the web ChatQuestionCard /
/// QuestionPrompt / DecisionTree (allternit-ai src/views/chat/components,
/// src/components/agent-elements/question). One question at a time with
/// Back / Skip / Next; the last one sends every answer in one
/// `POST /questions/:id/reply`. A decision tree's follow-up opens under the
/// picked branch.
struct QuestionCardView: View {
    let request: AgentQuestionRequest
    /// Who is asking ("Researcher", the bot's name). Nil reads "Agent question".
    var agentName: String? = nil
    /// Called once the answer (or skip) reached the agent.
    var onResolved: () -> Void = {}

    @ObservedObject private var store = AgentAskStore.shared

    private struct Draft: Equatable {
        var selected: [Int] = []
        var followUp: Int? = nil
        var text: String = ""
        var skipped = false
    }

    @State private var index = 0
    @State private var drafts: [Int: Draft] = [:]
    @State private var isSending = false
    @State private var sendError: String? = nil
    @State private var sentLabel: String? = nil
    @FocusState private var textFocused: Bool

    private var questions: [AgentQuestionInfo] { request.questions }
    private var current: AgentQuestionInfo { questions[min(index, questions.count - 1)] }
    private var draft: Draft { drafts[index] ?? Draft() }

    var body: some View {
        if questions.isEmpty || store.isResolved(request.id) && sentLabel == nil {
            EmptyView()
        } else {
            card
        }
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 12) {
            header

            if let sentLabel {
                Label(sentLabel, systemImage: "checkmark.circle.fill")
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
                    .transition(.opacity)
            } else {
                questionBody
                footer
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
        .accessibilityLabel(agentName.map { "\($0) asks" } ?? "Agent question")
    }

    // MARK: - Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: current.isTree ? "arrow.triangle.branch" : "questionmark.bubble")
                    .font(.caption.weight(.semibold))
                Text(agentName.map { "\($0) asks" } ?? "Agent question")
                    .font(.caption.weight(.semibold))
                if let risk = current.risk {
                    riskBadge(risk)
                }
                Spacer(minLength: 0)
                if questions.count > 1 {
                    Text("\(index + 1) of \(questions.count)")
                        .font(.caption.monospacedDigit())
                }
            }
            .foregroundColor(Color("TextSecondary"))

            Text(current.title)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)

            if let subtitle = current.subtitle {
                Text(subtitle)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let context = current.context, !context.isEmpty {
                Text(context)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("BgPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
        }
    }

    private func riskBadge(_ risk: String) -> some View {
        Text(risk.capitalized + " risk")
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .foregroundColor(risk == "high" ? .red : (risk == "medium" ? .orange : Color("TextSecondary")))
            .background(Capsule().fill((risk == "high" ? Color.red : (risk == "medium" ? Color.orange : Color.gray)).opacity(0.12)))
    }

    // MARK: - Body

    @ViewBuilder
    private var questionBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(current.options.enumerated()), id: \.offset) { optionIndex, option in
                optionRow(option, at: optionIndex)
                if current.isTree, draft.selected.first == optionIndex, let followUp = option.then {
                    followUpBlock(followUp)
                }
            }

            if current.options.isEmpty || current.custom {
                TextField(current.options.isEmpty ? "Type your answer…" : "Something else…", text: textBinding, axis: .vertical)
                    .lineLimit(1...4)
                    .font(.subheadline)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(Color("BgPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .stroke(Color("BorderSubtle"), lineWidth: 1)
                    )
                    .focused($textFocused)
                    .accessibilityLabel(current.options.isEmpty ? "Your answer" : "Your own answer")
            }

            if let sendError {
                Text(sendError)
                    .font(.caption)
                    .foregroundColor(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func optionRow(_ option: AgentQuestionOption, at optionIndex: Int) -> some View {
        let isSelected = draft.selected.contains(optionIndex)
        return Button {
            toggle(optionIndex)
        } label: {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: current.multiple
                      ? (isSelected ? "checkmark.square.fill" : "square")
                      : (isSelected ? "largecircle.fill.circle" : "circle"))
                    .font(.body)
                    .foregroundColor(isSelected ? Color("AccentPrimary") : Color("TextSecondary"))
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(option.label)
                            .font(.subheadline.weight(.medium))
                            .foregroundColor(Color("TextPrimary"))
                        if option.recommended {
                            Text("Recommended")
                                .font(.caption2.weight(.semibold))
                                .foregroundColor(Color("AccentPrimary"))
                        }
                    }
                    if let description = option.description, !description.isEmpty {
                        Text(description)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let caution = option.caution, !caution.isEmpty {
                        Text(caution)
                            .font(.caption)
                            .foregroundColor(.orange)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
                if option.then != nil {
                    Image(systemName: "chevron.down")
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                        .rotationEffect(.degrees(isSelected ? 0 : -90))
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(isSelected ? Color("AccentPrimary").opacity(0.10) : Color("BgPrimary"))
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .stroke(isSelected ? Color("AccentPrimary").opacity(0.5) : Color("BorderSubtle"), lineWidth: 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(isSending)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    private func followUpBlock(_ followUp: AgentQuestionFollowUp) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(followUp.question)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            ForEach(Array(followUp.options.enumerated()), id: \.offset) { leafIndex, leaf in
                let picked = draft.followUp == leafIndex
                Button {
                    var d = draft
                    d.followUp = picked ? nil : leafIndex
                    drafts[index] = d
                } label: {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: picked ? "largecircle.fill.circle" : "circle")
                            .foregroundColor(picked ? Color("AccentPrimary") : Color("TextSecondary"))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(leaf.label)
                                .font(.footnote.weight(.medium))
                                .foregroundColor(Color("TextPrimary"))
                            if let description = leaf.description, !description.isEmpty {
                                Text(description)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            if let caution = leaf.caution, !caution.isEmpty {
                                Text(caution).font(.caption).foregroundColor(.orange)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 6)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(isSending)
            }
        }
        .padding(.leading, 30)
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(Color("BorderSubtle"))
                .frame(width: 1.5)
                .padding(.leading, 20)
        }
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: 8) {
            if index > 0 {
                Button("Back") {
                    sendError = nil
                    index -= 1
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                .disabled(isSending)
            }
            Spacer(minLength: 0)
            Button("Skip") { skipCurrent() }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                .disabled(isSending)
            Button {
                advance()
            } label: {
                if isSending {
                    ProgressView().controlSize(.small)
                } else {
                    Text(index < questions.count - 1 ? "Next" : "Send")
                }
            }
            .buttonStyle(QuestionCardButtonStyle(kind: .primary))
            .disabled(isSending || !hasAnswer)
        }
    }

    // MARK: - State

    private var textBinding: Binding<String> {
        Binding(
            get: { drafts[index]?.text ?? "" },
            set: { value in
                var d = drafts[index] ?? Draft()
                d.text = value
                d.skipped = false
                drafts[index] = d
            }
        )
    }

    private var hasAnswer: Bool {
        !draft.selected.isEmpty || !draft.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func toggle(_ optionIndex: Int) {
        var d = draft
        d.skipped = false
        if current.multiple {
            if let at = d.selected.firstIndex(of: optionIndex) { d.selected.remove(at: at) } else { d.selected.append(optionIndex) }
        } else {
            d.selected = d.selected == [optionIndex] ? [] : [optionIndex]
            d.followUp = nil
        }
        drafts[index] = d
        sendError = nil
    }

    private func skipCurrent() {
        var d = draft
        d.skipped = true
        drafts[index] = d
        if index < questions.count - 1 {
            index += 1
        } else {
            submit()
        }
    }

    private func advance() {
        textFocused = false
        if index < questions.count - 1 {
            index += 1
        } else {
            submit()
        }
    }

    /// Answers → the reply payload (web `buildQuestionReply`): labels, a
    /// tree path, or typed text; skipped questions send "".
    private func replyAnswers() -> [AgentAsksClient.ReplyAnswer] {
        questions.enumerated().map { qIndex, question in
            let d = drafts[qIndex] ?? Draft()
            let custom = d.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if d.skipped || (d.selected.isEmpty && custom.isEmpty) {
                return .init(questionIndex: qIndex, answer: .one(""))
            }
            if question.options.isEmpty {
                return .init(questionIndex: qIndex, answer: .one(custom))
            }
            let labels = d.selected.compactMap { question.options.indices.contains($0) ? question.options[$0].label : nil }
            if question.isTree, let branchIndex = d.selected.first, custom.isEmpty {
                let branch = question.options[branchIndex]
                var path = [branch.label]
                if let leaf = d.followUp, let leaves = branch.then?.options, leaves.indices.contains(leaf) {
                    path.append(leaves[leaf].label)
                }
                return .init(questionIndex: qIndex, answer: .many(path))
            }
            if question.multiple {
                return .init(questionIndex: qIndex, answer: .many(custom.isEmpty ? labels : labels + [custom]))
            }
            return .init(questionIndex: qIndex, answer: .one(custom.isEmpty ? (labels.first ?? "") : custom))
        }
    }

    private func submit() {
        let answers = replyAnswers()
        let allSkipped = answers.allSatisfy { $0.answer == .one("") }
        isSending = true
        sendError = nil
        Task {
            do {
                if allSkipped {
                    try await store.skip(request)
                    withAnimation(.easeOut(duration: 0.2)) { sentLabel = "Skipped" }
                } else {
                    try await store.answer(request, answers: answers)
                    withAnimation(.easeOut(duration: 0.2)) { sentLabel = agentName.map { "Sent to \($0)" } ?? "Sent" }
                }
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                isSending = false
                onResolved()
            } catch {
                isSending = false
                sendError = "Couldn't send your answer. \(FriendlyErrorMessage.from(error.localizedDescription))"
                UINotificationFeedbackGenerator().notificationOccurred(.error)
            }
        }
    }
}

/// Buttons on the question / approval cards: 44pt targets, token colors.
struct QuestionCardButtonStyle: ButtonStyle {
    enum Kind { case primary, quiet, destructive }
    var kind: Kind

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.semibold))
            .padding(.horizontal, 14)
            .frame(minHeight: 36)
            .foregroundColor(foreground)
            .background(background.opacity(configuration.isPressed ? 0.75 : 1))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(kind == .quiet ? Color("BorderSubtle") : Color.clear, lineWidth: 1))
            .contentShape(Capsule())
            .padding(.vertical, 4)
    }

    private var foreground: Color {
        switch kind {
        case .primary: return Color("BgPrimary")
        case .quiet: return Color("TextPrimary")
        case .destructive: return .red
        }
    }

    private var background: Color {
        switch kind {
        case .primary: return Color("TextPrimary")
        case .quiet: return Color("BgPrimary")
        case .destructive: return Color.red.opacity(0.10)
        }
    }
}
