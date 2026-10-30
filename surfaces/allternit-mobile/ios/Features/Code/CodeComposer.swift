import SwiftUI
import UIKit

// MARK: - Launch composer

/// The Code prompt on the phone (ChatComposer's Code terminal line, phone.css
/// "Terminal-style composer: one square line, prompt and keys side by
/// side"): the computer on the left and the context meter on the right
/// above it, one line between dashed rules — `+`, the coral `❯` on the
/// text's first line, voice and the coral run key — then one icon row under
/// it (folder · branch · worktree · permission mode, then the model and the
/// Chat/Terminal switch on the right). Sending starts a code thread.
struct CodeLaunchComposer: View {
    @Binding var text: String
    let folder: CodeFolderInfo?
    /// The thread's terminal view: drawn on the dark terminal surface, and
    /// the status row's last item switches back to the chat view.
    var terminal: Bool = false
    /// A turn is running: the run key becomes a stop key.
    var isRunning: Bool = false
    var onStop: (() -> Void)? = nil
    let onSend: (String) -> Void
    let onOpenTerminal: () -> Void
    /// nil hides `+` (the terminal view takes text only).
    var onAttach: (() -> Void)? = nil
    /// The thread's backend session, for the context meter (nil on the
    /// launchpad: the meter then shows the plan window).
    var sessionId: String? = nil
    /// Moves when a turn lands, so the context meter follows the thread.
    var contextRefresh: Int = 0

    @StateObject private var dictation = DictationController()
    @State private var dictationBase = ""
    @FocusState private var isFocused: Bool

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// phone.css: prompt 15px and text 16px, both on a 22px line.
    private static let line: CGFloat = 22
    private static let textFont = UIFont.monospacedSystemFont(ofSize: 16, weight: .regular)

    var body: some View {
        VStack(spacing: 0) {
            CodeDeckRow(folder: folder, terminal: terminal, placement: .above, sessionId: sessionId,
                        contextRefresh: contextRefresh, onOpenTerminal: onOpenTerminal)
            DashedHairline(strong: isFocused, terminal: terminal)
            HStack(alignment: .bottom, spacing: 4) {
                if let onAttach {
                    Button(action: onAttach) {
                        Image(systemName: "plus")
                            .font(.system(size: 18, weight: .semibold))
                            .foregroundColor(ink.secondary)
                            .frame(width: 40, height: 40)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, 6)
                    .accessibilityLabel("Attach")
                }

                HStack(alignment: .top, spacing: 8) {
                    Text("❯")
                        .font(.system(size: 15, weight: .bold, design: .monospaced))
                        .foregroundColor(GizziCodeWordmark.coral)
                        .frame(height: Self.line)
                        .accessibilityHidden(true)
                    TextField("Run a command or describe a task...", text: $text, axis: .vertical)
                        .font(Font(Self.textFont))
                        .foregroundColor(ink.primary)
                        .tint(GizziCodeWordmark.coral)
                        .lineSpacing(Self.line - Self.textFont.lineHeight)
                        .padding(.vertical, (Self.line - Self.textFont.lineHeight) / 2)
                        .lineLimit(1...6)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.send)
                        .focused($isFocused)
                        .onSubmit(send)
                }
                .padding(.top, 9)
                .padding(.bottom, 9)
                .padding(.leading, onAttach == nil ? 8 : 12)
                .padding(.trailing, 4)
                .frame(maxWidth: .infinity, alignment: .leading)

                HStack(spacing: 4) {
                    Button(action: toggleDictation) {
                        Image(systemName: dictation.isRecording ? "waveform.circle.fill" : "waveform")
                            .font(.system(size: 15, weight: .bold))
                            .foregroundColor(ink.secondary)
                            .frame(width: 40, height: 40)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(dictation.isRecording ? "Stop voice input" : "Start voice mode")
                    if isRunning, trimmed.isEmpty, let onStop {
                        Button(action: onStop) {
                            Image(systemName: "stop.fill")
                                .font(.system(size: 12, weight: .bold))
                                .foregroundColor(.white)
                                .frame(width: 40, height: 40)
                                .background(GizziCodeWordmark.coral, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Stop")
                    } else if !trimmed.isEmpty {
                        Button(action: send) {
                            Image(systemName: "arrow.up")
                                .font(.system(size: 14, weight: .bold))
                                .foregroundColor(.white)
                                .frame(width: 40, height: 40)
                                .background(GizziCodeWordmark.coral, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Send")
                        .accessibilityHint("Run (Enter)")
                    }
                }
                .padding(.vertical, 6)
            }
            .padding(.horizontal, 6)
            DashedHairline(strong: isFocused, terminal: terminal)
            CodeDeckRow(folder: folder, terminal: terminal, placement: .below, sessionId: sessionId,
                        contextRefresh: contextRefresh, onOpenTerminal: onOpenTerminal)
        }
        .onChange(of: dictation.transcript) { _, transcript in
            guard dictation.isRecording || !transcript.isEmpty else { return }
            text = dictationBase + transcript
        }
    }

    private var ink: CodeInk { CodeInk(terminal: terminal) }

    private func send() {
        let value = trimmed
        guard !value.isEmpty, !isRunning else { return }
        dictation.stop()
        text = ""
        onSend(value)
    }

    private func toggleDictation() {
        if dictation.isRecording {
            dictation.stop()
        } else {
            dictationBase = text.isEmpty || text.hasSuffix(" ") ? text : text + " "
            Task { await dictation.start() }
        }
    }
}

/// The dashed top/bottom edge of the terminal prompt.
struct DashedHairline: View {
    /// The rules step up while the prompt has focus.
    var strong: Bool = false
    var terminal: Bool = false

    var body: some View {
        GeometryReader { geo in
            Path { path in
                path.move(to: CGPoint(x: 0, y: 0.5))
                path.addLine(to: CGPoint(x: geo.size.width, y: 0.5))
            }
            .stroke(strong ? CodeInk(terminal: terminal).borderStrong : CodeInk(terminal: terminal).border, style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
        }
        .frame(height: 1)
        .accessibilityHidden(true)
    }
}

// MARK: - Ink

/// The Code prompt's colours: the app's tokens, or the terminal palette on
/// the thread's terminal view (dark whatever the appearance).
struct CodeInk {
    let terminal: Bool

    var primary: Color { terminal ? TerminalPalette.foreground : Color("TextPrimary") }
    var secondary: Color { terminal ? TerminalPalette.secondary : Color("TextSecondary") }
    var tertiary: Color { terminal ? TerminalPalette.secondary.opacity(0.7) : Color("TextTertiary") }
    var accent: Color { terminal ? GizziCodeWordmark.coral : Color("AccentPrimary") }
    var fill: Color { terminal ? TerminalPalette.key : Color("FillNeutral") }
    var border: Color { terminal ? TerminalPalette.keyBorder : Color("BorderSubtle") }
    var borderStrong: Color { terminal ? TerminalPalette.pressedBorder : Color("BorderStrong") }
}

// MARK: - Deck row

/// The Code prompt's controls (CodeBottomStatusBar with CodeWorkspaceBar
/// leading), icons only so all eight fit a phone; each keeps its words as
/// its accessibility label. `.above` the prompt: the computer on the left,
/// the context meter on the right. `.below`: working folder · branch ·
/// worktree · permission mode on the left, the model and the Chat/Terminal
/// switch on the right.
struct CodeDeckRow: View {
    enum Placement { case above, below }

    let folder: CodeFolderInfo?
    /// On the terminal surface the switch reads "Chat" (back to the chat view).
    var terminal: Bool = false
    var placement: Placement = .below
    var sessionId: String? = nil
    var contextRefresh: Int = 0
    let onOpenTerminal: () -> Void

    @StateObject private var environmentStore = EnvironmentStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @StateObject private var modelStore = ModelStore.shared
    @ObservedObject private var usage = UsageStore.shared
    @AppStorage(CodePermissionMode.storageKey) private var permissionRaw = CodePermissionMode.manual.rawValue
    @AppStorage(CodeWorktree.storageKey) private var worktree = false

    @State private var sheet: Sheet? = nil
    /// The session's context window (gizzi `/session/:id/context`).
    @State private var context: SessionContextBreakdown? = nil
    @State private var contextSessionId: String? = nil

    enum Sheet: String, Identifiable {
        case environment, folder, permission, model, pairing, usage
        var id: String { rawValue }
    }

    private var ink: CodeInk { CodeInk(terminal: terminal) }

    private var permission: CodePermissionMode { CodePermissionMode(rawValue: permissionRaw) ?? .manual }

    /// Cloud shows the runtime it runs on and "Connecting" until it answers;
    /// local is "This computer".
    private var environmentLabel: String {
        environmentStore.environment == .cloud ? (relay.activeDevice?.name ?? "Cloud") : "This computer"
    }

    private var cloudStatus: String? {
        guard environmentStore.environment == .cloud else { return nil }
        return relay.activeDevice?.isOnline == true ? nil : "Connecting"
    }

    /// ChatComposer `modelPillLabel`: "Allternit Cloud · Kimi K3".
    private var modelLabel: String {
        let name = ComposerModelLabel.label(modelStore)
        guard let provider = modelStore.selectedModel?.provider, !provider.isEmpty else { return name }
        let providerName = CodeProviderName.name(provider)
        return name.lowercased().hasPrefix(providerName.lowercased()) ? name : "\(providerName) · \(name)"
    }

    /// The worktree pill only shows on the default branch (isDefaultCodeBranch).
    private var worktreeVisible: Bool {
        guard let branch = folder?.branch?.lowercased() else { return false }
        return branch == "main" || branch == "master"
    }

    var body: some View {
        HStack(spacing: 2) {
            switch placement {
            case .above:
                environmentButton
                Spacer(minLength: 8)
                contextMeter
            case .below:
                icon("folder", label: "Working folder: \(folder?.name ?? "Select folder")") { sheet = .folder }
                if let branch = folder?.branch, !branch.isEmpty {
                    Image(systemName: "arrow.triangle.branch")
                        .font(.system(size: 15))
                        .foregroundColor(ink.primary)
                        .frame(width: 36, height: 40)
                        .accessibilityElement()
                        .accessibilityLabel("Branch \(branch)")
                }
                if worktreeVisible {
                    icon(worktree ? "square.split.diagonal.2x2.fill" : "square.split.diagonal.2x2",
                         label: "Worktree isolation", tint: worktree ? ink.accent : ink.primary) { worktree.toggle() }
                        .accessibilityValue(worktree ? "On" : "Off")
                }
                icon(permission.icon, label: "Code permission mode: \(permission.label)", tint: ink.accent) { sheet = .permission }
                Spacer(minLength: 8)
                modelButton
                modeSwitch
            }
        }
        .padding(.top, placement == .above ? 2 : 4)
        .padding(.bottom, placement == .above ? 2 : 12)
        .padding(.horizontal, 6)
        .frame(maxWidth: .infinity)
        .task {
            switch placement {
            case .above:
                await relay.refresh()
                usage.fetchUsageIfNeeded()
            case .below:
                modelStore.fetchModelsIfNeeded()
            }
        }
        .task(id: "\(sessionId ?? "")-\(contextRefresh)") {
            guard placement == .above else { return }
            await loadContext()
        }
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .environment:
                CodeEnvironmentSheet(onPair: { self.sheet = .pairing })
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
            case .folder:
                CodeFolderSheet(folder: folder)
                    .presentationDetents([.height(220), .medium])
                    .presentationDragIndicator(.visible)
            case .permission:
                CodePermissionSheet(selection: $permissionRaw)
                    .presentationDetents([.medium])
                    .presentationDragIndicator(.visible)
            case .model:
                ModelPickerSheet(modelStore: modelStore)
            case .pairing:
                RuntimePairingView()
            case .usage:
                NavigationStack {
                    List {
                        if let context { CodeContextRows(breakdown: context) }
                        PlanUsageRows()
                    }
                        .navigationTitle(context == nil ? "Usage" : "Context and usage")
                        .navigationBarTitleDisplayMode(.inline)
                }
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
            }
        }
    }

    // MARK: Items

    /// The computer the thread runs on; a dot while a cloud runtime wakes.
    private var environmentButton: some View {
        icon(environmentStore.environment == .cloud ? "cloud" : "desktopcomputer",
             label: "Environment: \(environmentLabel)\(cloudStatus.map { ", \($0)" } ?? "")") { sheet = .environment }
            .overlay(alignment: .topTrailing) {
                if cloudStatus != nil {
                    Circle().fill(Theme.statusWarning).frame(width: 6, height: 6).offset(x: -8, y: 10)
                        .allowsHitTesting(false)
                }
            }
    }

    /// ComposerUsageMeter: the session's context fill as HUD blocks and a
    /// percent; with no session yet, the plan window's blocks. Hidden when
    /// neither has a figure.
    @ViewBuilder
    private var contextMeter: some View {
        if let context {
            Button(action: { sheet = .usage }) {
                HStack(spacing: 6) {
                    CodeUsageBlocks(fraction: context.utilization, tone: tone(context.level))
                    Text("\(Int((context.utilization * 100).rounded()))%")
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundColor(context.level == .ok ? ink.secondary : tone(context.level))
                }
                .padding(.horizontal, 8)
                .frame(height: 32)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Context: \(Int((context.utilization * 100).rounded()))% used (\(SessionContextBreakdown.format(context.usedTokens)) of \(SessionContextBreakdown.format(context.contextWindow))). Open details.")
        } else if let percent = usage.percentUsed {
            Button(action: { sheet = .usage }) {
                CodeUsageBlocks(fraction: percent / 100)
                    .padding(.horizontal, 8)
                    .frame(height: 32)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Usage: \(usage.percentText ?? "") used. Open details.")
        }
    }

    private func tone(_ level: SessionContextBreakdown.Level) -> Color {
        switch level {
        case .ok: return ink.secondary
        case .approaching: return Theme.statusWarning
        case .reached: return Theme.statusError
        }
    }

    /// The model, as its provider mark.
    private var modelButton: some View {
        Button(action: { sheet = .model }) {
            Image("MatrixLogo")
                .resizable()
                .scaledToFit()
                .frame(width: 16, height: 16)
                .frame(width: 24, height: 24)
                .background(ink.fill, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(ink.border, lineWidth: 1))
                .frame(width: 40, height: 40)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Model: \(modelLabel)")
    }

    /// Chat · Terminal: this thread as a chat or in its terminal look. The
    /// one view switch in Code mode (the shell sits in the sessions sheet).
    private var modeSwitch: some View {
        HStack(spacing: 0) {
            ForEach([false, true], id: \.self) { isTerminal in
                let on = isTerminal == terminal
                Button(action: { if !on { onOpenTerminal() } }) {
                    Image(systemName: isTerminal ? "apple.terminal" : "bubble.left")
                        .font(.system(size: 13, weight: on ? .bold : .regular))
                        .foregroundColor(on ? ink.primary : ink.tertiary)
                        .frame(width: 32, height: 28)
                        .background(
                            RoundedRectangle(cornerRadius: 6, style: .continuous)
                                .fill(on ? (terminal ? TerminalPalette.pressedBorder : Color("BgPanel")) : .clear)
                                .shadow(color: .black.opacity(on && !terminal ? 0.08 : 0), radius: 1, y: 1)
                        )
                }
                .buttonStyle(.plain)
                .accessibilityLabel(isTerminal ? "Terminal" : "Chat")
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(terminal ? TerminalPalette.key : Color("BorderStrong")))
        .padding(.leading, 2)
    }

    private func icon(_ systemName: String, label: String, tint: Color? = nil, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 15))
                .foregroundColor(tint ?? ink.primary)
                .frame(width: 36, height: 40)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    /// The thread's context window; a failed read keeps the last figure
    /// for the same session.
    private func loadContext() async {
        guard let sessionId, !sessionId.hasPrefix("temp-") else {
            context = nil
            contextSessionId = nil
            return
        }
        if contextSessionId != sessionId { context = nil }
        if let breakdown = try? await AgentChatClient().sessionContext(sessionId: sessionId) {
            context = breakdown
            contextSessionId = sessionId
        }
    }
}

/// The composer usage meter's five HUD blocks (ComposerUsageMeter
/// `HudBlocks count=5`): filled by the plan window used, amber from 80%,
/// red at the limit.
struct CodeUsageBlocks: View {
    let fraction: Double
    /// The context meter passes its own level colour.
    var tone: Color? = nil

    private var fill: Color {
        if let tone { return tone }
        if fraction >= 1 { return Theme.statusError }
        if fraction >= 0.8 { return Theme.statusWarning }
        return Color("TextSecondary")
    }

    var body: some View {
        let filled = Int((min(max(fraction, 0), 1) * 5).rounded(.up))
        HStack(spacing: 2) {
            ForEach(0..<5, id: \.self) { index in
                RoundedRectangle(cornerRadius: 1)
                    .fill(index < filled ? fill : Color("BorderStrong"))
                    .frame(width: 5, height: 10)
            }
        }
        .accessibilityHidden(true)
    }
}

/// The context panel's section (ContextUsagePanel): the window's fill, then
/// what fills it, row by row, and who measured it.
struct CodeContextRows: View {
    let breakdown: SessionContextBreakdown

    var body: some View {
        Section {
            HStack {
                Text("\(SessionContextBreakdown.format(breakdown.usedTokens)) of \(SessionContextBreakdown.format(breakdown.contextWindow))")
                    .font(.system(size: 14, weight: .semibold))
                Spacer()
                Text("\(Int((breakdown.utilization * 100).rounded()))%")
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
            }
            ForEach(breakdown.categories.filter { $0.deferred != true }, id: \.id) { row in
                HStack {
                    Text(row.name).font(.system(size: 13))
                    Spacer()
                    Text(SessionContextBreakdown.format(row.tokens))
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        } header: {
            Text("Context window")
        } footer: {
            Text(measuredBy)
        }
    }

    /// ContextUsage.tsx `measuredLabel`, short form.
    private var measuredBy: String {
        if breakdown.source == "brain" { return "Reported by \(breakdown.brainName ?? "the brain")" }
        return breakdown.basis == "estimated" ? "Estimated" : "Measured by the provider"
    }
}

/// Provider display names (provider-registry.ts `name`).
enum CodeProviderName {
    static func name(_ provider: String) -> String {
        switch provider.lowercased() {
        case "allternit": return "Allternit Cloud"
        case "anthropic": return "Anthropic"
        case "claude": return "Claude"
        case "openai": return "OpenAI"
        case "codex": return "Codex"
        case "google": return "Google Gemini"
        case "gemini": return "Gemini"
        case "kimi": return "Kimi"
        case "xai": return "xAI"
        case "grok": return "Grok"
        case "deepseek": return "DeepSeek"
        case "mistral": return "Mistral"
        case "openrouter": return "OpenRouter"
        case "ollama": return "Ollama"
        case "qwen": return "Qwen"
        default: return provider.prefix(1).uppercased() + provider.dropFirst()
        }
    }
}

/// Worktree isolation for the next code thread (CodeCanvas pendingWorktree):
/// a new thread is stamped `metadata.isolation = "worktree"`.
enum CodeWorktree {
    static let storageKey = "allternit-code-pending-worktree"
}

// MARK: - Deck sheets

/// The environment picker (CodeWorkspaceBar's Environment dropdown, a
/// bottom sheet on the phone): This computer, the cloud runtimes it can run
/// on, and "Pair or configure runtime…".
private struct CodeEnvironmentSheet: View {
    let onPair: () -> Void
    @StateObject private var environmentStore = EnvironmentStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                section("This computer")
                row(
                    icon: "desktopcomputer",
                    title: "This computer",
                    subtitle: "Runs in the folder you pick, on this computer",
                    selected: environmentStore.environment == .local
                ) {
                    environmentStore.environment = .local
                    dismiss()
                }

                section("Runs on").padding(.top, 12)
                if relay.isLoading && relay.devices.isEmpty {
                    row(icon: "arrow.clockwise", title: "Loading runtimes…", subtitle: nil, selected: false, action: nil)
                } else {
                    ForEach(relay.devices) { device in
                        row(
                            icon: device.runtimeType == "vps" || device.runtimeType == "hosted" ? "cloud" : "desktopcomputer",
                            title: device.name.isEmpty ? "Allternit runtime" : device.name,
                            subtitle: device.isOnline ? (device.isPreferred ? "Default" : nil) : "Asleep · wakes on first request",
                            selected: environmentStore.environment == .cloud && relay.activeRuntimeId == device.id
                        ) {
                            relay.select(device.id)
                            environmentStore.environment = .cloud
                            dismiss()
                        }
                    }
                }
                Divider().padding(.vertical, 6)
                row(
                    icon: "plus",
                    title: "Pair or configure runtime…",
                    subtitle: relay.devices.isEmpty ? (relay.lastError ?? "Cloud environments need a cloud computer or a paired runtime") : nil,
                    selected: false
                ) {
                    dismiss()
                    onPair()
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 20)
        }
        .background(Color("BgPanel"))
        .task { await relay.refresh() }
    }

    private func section(_ title: String) -> some View {
        Text(title.uppercased())
            .font(.system(size: 11, weight: .semibold))
            .tracking(0.6)
            .foregroundColor(Color("BrandAccent"))
            .padding(.horizontal, 8)
            .padding(.bottom, 4)
    }

    private func row(icon: String, title: String, subtitle: String?, selected: Bool, action: (() -> Void)?) -> some View {
        Button(action: { action?() }) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 18)
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if let subtitle {
                        Text(subtitle)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer()
                if selected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 8)
            .frame(minHeight: 44)
            .background(selected ? Color("FillNeutral") : .clear)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(action == nil)
    }
}

/// The working-folder sheet: the folder the computer's gizzi-code runs in
/// (the web's "Recent" list). Folders are added on the computer itself.
private struct CodeFolderSheet: View {
    let folder: CodeFolderInfo?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("RECENT")
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.6)
                .foregroundColor(Color("BrandAccent"))
                .padding(.horizontal, 8)
                .padding(.bottom, 4)
            if let folder {
                HStack(spacing: 10) {
                    Image(systemName: "folder")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(folder.name)
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                        Text(folder.directory)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer()
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .padding(.horizontal, 8)
                .frame(minHeight: 44)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 10))
            } else {
                Text("Connect a computer or Allternit Cloud to pick a folder.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .frame(minHeight: 44)
            }
            Spacer()
        }
        .padding(.horizontal, 14)
        .padding(.top, 24)
        .background(Color("BgPanel"))
    }
}

/// The permission-mode overlay (CodeBottomStatusBar `ModeSelector`): four
/// modes with descriptions, a check on the active one.
private struct CodePermissionSheet: View {
    @Binding var selection: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            ForEach(CodePermissionMode.allCases) { mode in
                let active = mode.rawValue == selection
                Button(action: {
                    selection = mode.rawValue
                    dismiss()
                }) {
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: mode.icon)
                            .font(.system(size: 14))
                            .foregroundColor(Color("AccentPrimary"))
                            .frame(width: 30, height: 30)
                            .background(Color("AccentPrimary").opacity(0.1))
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(mode.label)
                                .font(.system(size: 13, weight: active ? .bold : .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Text(mode.description)
                                .font(.system(size: 11.5))
                                .foregroundColor(Color("TextSecondary"))
                        }
                        Spacer()
                        if active {
                            Image(systemName: "checkmark")
                                .font(.system(size: 13, weight: .bold))
                                .foregroundColor(Color("AccentPrimary"))
                                .padding(.top, 2)
                        }
                    }
                    .padding(10)
                    .background(active ? Color("AccentPrimary").opacity(0.1) : .clear)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(.horizontal, 10)
        .padding(.top, 24)
        .background(Color("BgPanel"))
    }
}
