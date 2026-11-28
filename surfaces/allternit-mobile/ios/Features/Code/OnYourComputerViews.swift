import SwiftUI
import UIKit

// MARK: - Terminal previews

/// The status dot's colour for a terminal's state.
private func toneColor(_ tone: TerminalPreview.Tone) -> Color {
    switch tone {
    case .working: return GizziCodeWordmark.coral
    case .waiting: return Theme.statusWarning
    case .idle: return Theme.statusSuccess
    case .done: return Color("TextTertiary")
    }
}

/// The last screen rows of a terminal, monospace and clipped (web TerminalPreview).
struct TerminalPreviewLines: View {
    let rows: [String]
    var fontSize: CGFloat = 11

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                Text(row.isEmpty ? " " : row)
                    .font(.system(size: fontSize, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .clipped()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(rows.isEmpty ? "" : "Last lines: \(rows.joined(separator: ". "))")
    }
}

/// One Allternit terminal in the Code sessions view: title, folder, status
/// (dot + word read off the screen) and its last rows. `tall` = Grid / Deck card.
struct CodeTerminalPreviewRow: View {
    let terminal: TerminalListItem
    let layout: OverviewLayout
    let onOpen: () -> Void

    var body: some View {
        let status = TerminalPreview.status(terminal)
        let title = TerminalPreview.title(terminal)
        let folder = OnYourComputer.basename(terminal.cwd)
        let detail = [folder, status.agent].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
        let rows = TerminalPreview.rows(terminal.preview, title: terminal.title, max: TerminalPreview.rowCount(layout))
        let card = layout != .list
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: card ? 8 : 6) {
                HStack(spacing: 8) {
                    Image(systemName: "terminal")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(title)
                            .font(.system(size: card ? 13 : 15, weight: card ? .semibold : .regular))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if !detail.isEmpty {
                            Text(detail)
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 4)
                    HStack(spacing: 5) {
                        Circle().fill(toneColor(status.tone)).frame(width: 7, height: 7)
                        Text(status.word)
                            .font(.system(size: 11, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .accessibilityHidden(true)
                }
                if !rows.isEmpty {
                    TerminalPreviewLines(rows: rows)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .background(Color("BgTertiary").opacity(0.6))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                if card { Spacer(minLength: 0) }
            }
            .padding(card ? 10 : 0)
            .padding(.vertical, card ? 0 : 8)
            .frame(maxWidth: .infinity, maxHeight: layout == .deck ? .infinity : nil, alignment: .topLeading)
            .background(card ? Color("BgPanel") : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: card ? 14 : 0))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: card ? 1 : 0))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel([title, detail, status.word].filter { !$0.isEmpty }.joined(separator: ", "))
        .accessibilityHint("Opens this terminal")
        .accessibilityAction(named: "Open terminal", onOpen)
    }
}

// MARK: - On your computer

/// A coding-agent session running outside Allternit (Terminal.app, iTerm2,
/// Ghostty…) with what can be done with it here (web OnThisComputerRows):
/// Move here (always through the confirm sheet on a phone), Open a copy, New
/// session here, and the Move's progress.
struct OnYourComputerRow: View {
    let session: LiveSession
    let store: TerminalSessionsStore
    /// List row, or a Grid / Deck card like the other sessions.
    var layout: OverviewLayout = .list
    /// Opens a terminal by its session id in the Code terminal.
    let onOpenTerminal: (String, String) -> Void

    @ReducedMotion private var reduced
    @State private var confirming = false
    @State private var copiedCommand = false

    private var name: String { OnYourComputer.displayName(session) }

    var body: some View {
        let set = OnYourComputer.rowActions(session.movable, canCopy: session.canCopy)
        let job = store.adoptJobs[session.sessionId]
        let progress = job.flatMap { $0.finished ? nil : OnYourComputer.jobStateText($0.state) }
        let failure = job.flatMap { $0.state == "failed" ? OnYourComputer.moveErrorText(code: $0.code, message: $0.error) : nil }
        let flags = OnYourComputer.flagChips(session.carriedFlags)
        let detail = [OnYourComputer.basename(session.cwd), OnYourComputer.appLine(session),
                      OnYourComputer.statusWord(session.status)].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")

        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 10) {
                Text(OnYourComputer.harnessName(session.harness))
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(Color("BgTertiary"))
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name)
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(detail)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                Circle()
                    .fill(session.status == "busy" ? GizziCodeWordmark.coral : session.status == "idle" ? Theme.statusSuccess : Color("TextTertiary"))
                    .frame(width: 8, height: 8)
                    .padding(.top, 6)
                    .accessibilityHidden(true)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(OnYourComputer.harnessName(session.harness)), \(name), \(detail)")

            if !flags.isEmpty {
                FlowChips(label: "Keeps:", chips: flags)
            }
            if set.openElsewhere {
                Text(OnYourComputer.openElsewhereText(session.holders))
                    .font(.system(size: 13))
                    .foregroundColor(Theme.statusWarning)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if set.noResume {
                Text("This CLI can't resume a session by id.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }

            if let job, let progress {
                progressLine(progress, job: job)
            } else if let failure {
                failureBox(failure)
            } else if !set.actions.isEmpty {
                actionButtons(set.actions)
            }
        }
        .padding(.vertical, layout == .list ? 10 : 0)
        .padding(layout == .list ? 0 : 10)
        .frame(maxWidth: .infinity, maxHeight: layout == .deck ? .infinity : nil, alignment: .topLeading)
        .background(layout == .list ? Color.clear : Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: layout == .list ? 0 : 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: layout == .list ? 0 : 1))
        .animation(reduced ? nil : .easeOut(duration: 0.2), value: job)
        .sheet(isPresented: $confirming) {
            MoveConfirmSheet(session: session, onCancel: { confirming = false }, onMove: {
                confirming = false
                Haptics.light()
                Task {
                    _ = await store.startAdopt(session, action: .move, open: { id in onOpenTerminal(id, name) })
                }
            })
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
    }

    private func actionButtons(_ actions: [OnYourComputer.RowAction]) -> some View {
        let pending = store.pending[session.sessionId]
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(actions, id: \.self) { action in
                    let primary = action == .move || action == .moveWhenIdle
                    Button(action: { run(action) }) {
                        HStack(spacing: 5) {
                            if pending == action {
                                ProgressView().controlSize(.mini)
                            }
                            Text(pending == action ? "Opening…" : action.label)
                                .font(.system(size: 13, weight: .semibold))
                        }
                        .foregroundColor(primary ? Color("BgPrimary") : Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .frame(minHeight: 36)
                        .background(primary ? Color("TextPrimary") : Color("BgTertiary"))
                        .clipShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .disabled(pending != nil)
                    .accessibilityLabel("\(action.label): \(name)")
                }
            }
        }
    }

    private func run(_ action: OnYourComputer.RowAction) {
        switch action {
        case .move, .moveWhenIdle:
            // Eoj's rule: on a phone a Move always goes through the confirm sheet.
            confirming = true
        case .copy, .new:
            Haptics.light()
            Task {
                if let id = await store.startAdopt(session, action: action, open: nil) {
                    onOpenTerminal(id, name)
                }
            }
        }
    }

    private func progressLine(_ text: String, job: AdoptJob) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(text)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                if job.state == "waiting_idle" {
                    Button("Cancel") { Task { await store.cancelAdopt(session) } }
                        .font(.system(size: 13, weight: .semibold))
                        .frame(minHeight: 36)
                        .accessibilityLabel("Cancel moving \(name)")
                }
            }
            if let notice = job.notice {
                Text(notice)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .accessibilityElement(children: .contain)
    }

    private func failureBox(_ failure: OnYourComputer.MoveErrorText) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top) {
                Text(failure.text)
                    .font(.system(size: 13))
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 4)
                Button(action: { store.dismissJob(session.sessionId) }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 32, height: 32)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss")
            }
            if let command = failure.command {
                if let cwd = failure.cwd {
                    Text("In \(cwd):")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
                HStack(alignment: .top, spacing: 8) {
                    Text(command)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button(copiedCommand ? "Copied" : "Copy") {
                        UIPasteboard.general.string = command
                        copiedCommand = true
                        Haptics.light()
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .frame(minHeight: 32)
                    .accessibilityLabel(copiedCommand ? "Copied" : "Copy the command")
                }
                .padding(8)
                .background(Color("BgTertiary"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
        }
    }
}

/// "Keeps: --model opus --dangerously-skip-permissions" as small monospace chips.
struct FlowChips: View {
    let label: String
    let chips: [String]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                Text(label)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
                ForEach(Array(chips.enumerated()), id: \.offset) { _, chip in
                    Text(chip)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 5)
                        .padding(.vertical, 2)
                        .background(Color("BgTertiary"))
                        .clipShape(RoundedRectangle(cornerRadius: 4))
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label) \(chips.joined(separator: " "))")
    }
}

/// The Move confirm (web MoveConfirmSheet): what Move runs, the flags it
/// keeps, and what happens to the old window, with Move / Cancel.
struct MoveConfirmSheet: View {
    let session: LiveSession
    let onCancel: () -> Void
    let onMove: () -> Void

    var body: some View {
        let name = OnYourComputer.displayName(session)
        let holder = session.holders.first
        let holderText = holder.map { OnYourComputer.holderLabel(app: $0.app, pid: $0.pid) } ?? session.pid.map { "pid \($0)" }
        let whenIdle = session.movable == "busy"
        let command = OnYourComputer.moveCommandLine(session.moveCommand)
        let flags = OnYourComputer.flagChips(session.carriedFlags)
        VStack(alignment: .leading, spacing: 14) {
            Text("Move \(name) here?")
                .font(.system(size: 18, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            if let command {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(command)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .padding(10)
                }
                .background(Color("BgTertiary"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .accessibilityLabel("Command: \(command)")
            }
            if !flags.isEmpty {
                FlowChips(label: "Keeps:", chips: flags)
            }
            Text(OnYourComputer.confirmText(app: holder?.app, holder: holderText, whenIdle: whenIdle))
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack(spacing: 10) {
                Button(action: onCancel) {
                    Text("Cancel")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .background(Color("BgTertiary"))
                        .clipShape(Capsule())
                }
                .buttonStyle(.plain)
                Button(action: onMove) {
                    Text(whenIdle ? "Move when idle" : "Move")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .background(Color("TextPrimary"))
                        .clipShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(whenIdle ? "Move \(name) when idle" : "Move \(name) here")
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color("BgPrimary").ignoresSafeArea())
    }
}
