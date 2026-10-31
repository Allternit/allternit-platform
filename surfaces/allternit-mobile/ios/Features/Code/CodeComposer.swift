import SwiftUI
import UIKit

// MARK: - Launch composer

/// The Code prompt on the phone (ChatComposer's Code terminal line): one line
/// in a dashed frame — `+`, the coral `❯` on the text's first line, voice and
/// the coral run key — with the cache state and the model on the frame's top
/// edge. Under it, one icon row (folder · branch · worktree · permission mode,
/// then the context meter and the Chat/Terminal switch) and the last run's
/// usage. The context bar sits in the icon row or above the frame
/// (`CodeContextBarPosition`). Sending starts a code thread.
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
    /// The newest run's reported usage, for the usage line and cache state.
    var lastRunUsage: CodeRunUsage? = nil
    /// Moves when a turn lands, so the context meter follows the thread.
    var contextRefresh: Int = 0

    @StateObject private var contextState = CodeContextState()
    @AppStorage(CodeContextBarPosition.storageKey) private var contextBarRaw = CodeContextBarPosition.bottom.rawValue
    @Environment(\.horizontalSizeClass) private var sizeClass
    @StateObject private var dictation = DictationController()
    @State private var dictationBase = ""
    @FocusState private var isFocused: Bool

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// phone.css: prompt 15px and text 16px, both on a 22px line.
    private static let line: CGFloat = 22
    private static let textFont = UIFont.monospacedSystemFont(ofSize: 16, weight: .regular)

    var body: some View {
        VStack(spacing: 0) {
            if contextBarRaw == CodeContextBarPosition.above.rawValue {
                CodeContextBar(state: contextState, terminal: terminal)
            }
            CodePromptEdge(strong: isFocused, terminal: terminal, isRunning: isRunning, usage: lastRunUsage)
                .zIndex(1)
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
                    TextField("Describe a task...", text: $text, prompt: Text("Describe a task...").foregroundColor(ink.tertiary), axis: .vertical)
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
                    WaitingGamesPlayButton(busy: isRunning)
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
            .padding(.top, 14)
            .padding(.horizontal, 6)
            .overlay {
                CodePromptSides()
                    .stroke(isFocused ? ink.borderStrong : ink.border, style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                    .accessibilityHidden(true)
            }
            CodeDeckRow(folder: folder, terminal: terminal, context: contextState,
                        showsMeter: contextBarRaw != CodeContextBarPosition.above.rawValue, onOpenTerminal: onOpenTerminal)
            if let line = lastRunUsage?.line(narrow: sizeClass != .regular) {
                Text(line)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(ink.tertiary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .padding(.horizontal, 8)
                    .padding(.bottom, 10)
            }
        }
        // The frame needs a gutter: its sides would sit on the screen edge.
        .padding(.horizontal, 10)
        .task(id: "\(sessionId ?? "")-\(contextRefresh)") {
            await contextState.load(sessionId: sessionId)
        }
        .sheet(isPresented: $contextState.showsUsage) {
            NavigationStack {
                List {
                    if let context = contextState.breakdown { CodeContextRows(breakdown: context) }
                    PlanUsageRows()
                    Section("Context bar") {
                        Picker("Position", selection: $contextBarRaw) {
                            Text("Bottom").tag(CodeContextBarPosition.bottom.rawValue)
                            Text("Above").tag(CodeContextBarPosition.above.rawValue)
                        }
                        .pickerStyle(.segmented)
                    }
                }
                .navigationTitle(contextState.breakdown == nil ? "Usage" : "Context and usage")
                .navigationBarTitleDisplayMode(.inline)
            }
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
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

// MARK: - Prompt frame

/// The frame's sides and bottom; `CodePromptEdge` draws its top.
private struct CodePromptSides: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.minX + 0.5, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.minX + 0.5, y: rect.maxY - 0.5))
        path.addLine(to: CGPoint(x: rect.maxX - 0.5, y: rect.maxY - 0.5))
        path.addLine(to: CGPoint(x: rect.maxX - 0.5, y: rect.minY))
        return path
    }
}

/// The frame's top edge, drawn in segments so a background shows through: the
/// prompt cache's state on the left, the model and the run dot on the right.
private struct CodePromptEdge: View {
    var strong: Bool
    var terminal: Bool
    var isRunning: Bool
    var usage: CodeRunUsage?

    @StateObject private var modelStore = ModelStore.shared
    @State private var showsModels = false

    private var ink: CodeInk { CodeInk(terminal: terminal) }

    /// ChatComposer `modelPillLabel`: "Allternit Cloud · Kimi K3".
    private var modelLabel: String {
        let name = ComposerModelLabel.label(modelStore)
        guard let provider = modelStore.selectedModel?.provider, !provider.isEmpty else { return name }
        let providerName = CodeProviderName.name(provider)
        return name.lowercased().hasPrefix(providerName.lowercased()) ? name : "\(providerName) · \(name)"
    }

    var body: some View {
        HStack(spacing: 6) {
            DashedHairline(strong: strong, terminal: terminal).frame(width: 10)
            if usage?.hasCacheEstimate == true {
                TimelineView(.periodic(from: .now, by: 30)) { timeline in
                    if let state = usage?.cacheState(now: timeline.date) {
                        Text("cache \(state)")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(ink.tertiary)
                            .fixedSize()
                    }
                }
            }
            DashedHairline(strong: strong, terminal: terminal).frame(minWidth: 8)
            Button(action: { showsModels = true }) {
                HStack(spacing: 5) {
                    Image("MatrixLogo")
                        .resizable()
                        .scaledToFit()
                        .frame(width: 14, height: 14)
                    Text(modelLabel)
                        .font(.system(size: 11, weight: .medium, design: .monospaced))
                        .foregroundColor(ink.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundColor(ink.secondary)
                }
                .frame(minHeight: 36)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Model: \(modelLabel)")
            Circle()
                .fill(isRunning ? GizziCodeWordmark.coral : Color(hex: "#8EAB84"))
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
            DashedHairline(strong: strong, terminal: terminal).frame(width: 10)
        }
        .frame(height: 36)
        .padding(.bottom, -18)
        .task { modelStore.fetchModelsIfNeeded() }
        .sheet(isPresented: $showsModels) { ModelPickerSheet(modelStore: modelStore) }
    }
}

// MARK: - Run usage

/// `usage` on the chat stream's finish frame (ChatFinishUsage). Absent is not
/// zero: a provider that reports nothing shows nothing.
struct CodeRunUsage: Decodable, Equatable, Sendable {
    var inputTokens: Double?
    var outputTokens: Double?
    var cacheReadTokens: Double?
    var cacheWriteTokens: Double?
    var cacheTtlSeconds: Double?
    /// Estimated expiry in epoch milliseconds.
    var cacheExpiresAt: Double?
    var cost: Double?
    var estimated: Bool?

    /// `52.8k in · 152 out · 88% cached · $0.03`; nil when nothing was
    /// reported (code-composer-status.tsx `codeRunUsageLine`).
    func line(narrow: Bool) -> String? {
        let approx = estimated == true ? "~" : ""
        let input = inputTokens ?? 0
        let output = outputTokens ?? 0
        let total = input + (cacheReadTokens ?? 0) + (cacheWriteTokens ?? 0)
        var parts: [String] = []
        if input + output > 0 {
            parts.append("\(approx)\(Self.compact(input)) in")
            if !narrow { parts.append("\(approx)\(Self.compact(output)) out") }
        }
        if let read = cacheReadTokens, total > 0 {
            parts.append("\(Int((read / total * 100).rounded()))% cached")
        }
        if let cost, cost > 0 {
            parts.append(cost < 0.01 ? String(format: "$%.4f", cost) : String(format: "$%.2f", cost))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    var hasCacheEstimate: Bool { (cacheTtlSeconds ?? 0) > 0 && cacheExpiresAt != nil }

    /// `warm`, `expiring` or `cold` (cache-countdown.ts); nil with no estimate.
    func cacheState(now: Date) -> String? {
        guard let ttl = cacheTtlSeconds, ttl > 0, let expiresAt = cacheExpiresAt else { return nil }
        let left = max(0, min(ttl, expiresAt / 1000 - now.timeIntervalSince1970))
        if left == 0 { return "cold" }
        return left / ttl < 0.2 ? "expiring" : "warm"
    }

    /// RunTelemetry.tsx `compact`.
    private static func compact(_ n: Double) -> String {
        if n >= 1_000_000 { return String(format: n >= 10_000_000 ? "%.0fM" : "%.1fM", n / 1_000_000) }
        if n >= 1_000 { return String(format: n >= 10_000 ? "%.0fk" : "%.1fk", n / 1_000) }
        return String(Int(n))
    }
}

// MARK: - Context

/// Where the context bar sits; the usage sheet sets it.
enum CodeContextBarPosition: String {
    case bottom, above
    static let storageKey = "allternit-code-composer-context-bar"
}

/// The session's context window (gizzi `/session/:id/context`), shared by the
/// meter and the bar, and whether the usage sheet is open.
@MainActor
final class CodeContextState: ObservableObject {
    @Published var breakdown: SessionContextBreakdown? = nil
    @Published var showsUsage = false
    private var loadedSessionId: String? = nil

    /// A failed read keeps the last figure for the same session.
    func load(sessionId: String?) async {
        guard let sessionId, !sessionId.hasPrefix("temp-") else {
            breakdown = nil
            loadedSessionId = nil
            return
        }
        if loadedSessionId != sessionId { breakdown = nil }
        if let next = try? await AgentChatClient().sessionContext(sessionId: sessionId) {
            breakdown = next
            loadedSessionId = sessionId
        }
    }
}

/// ComposerContextBar: a thin bar on its own line above the prompt. With no
/// session yet it shows the plan window, like the meter. A tap opens the usage
/// sheet, which also moves it back.
private struct CodeContextBar: View {
    @ObservedObject var state: CodeContextState
    var terminal: Bool
    @ObservedObject private var usage = UsageStore.shared

    private var ink: CodeInk { CodeInk(terminal: terminal) }
    private var fraction: Double { min(max(state.breakdown?.utilization ?? (usage.percentUsed ?? 0) / 100, 0), 1) }
    private var tint: Color {
        switch state.breakdown?.level {
        case .approaching: return Theme.statusWarning
        case .reached: return Theme.statusError
        default: return GizziCodeWordmark.coral
        }
    }

    var body: some View {
        Button(action: { state.showsUsage = true }) {
            HStack(spacing: 8) {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(ink.border)
                        Capsule().fill(tint).frame(width: geo.size.width * fraction)
                    }
                }
                .frame(height: 2)
                if let breakdown = state.breakdown {
                    Text("\(Int((breakdown.utilization * 100).rounded()))%")
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(ink.tertiary)
                }
            }
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(state.breakdown.map { "Context: \(Int(($0.utilization * 100).rounded()))% used (\(SessionContextBreakdown.format($0.usedTokens)) of \(SessionContextBreakdown.format($0.contextWindow))). Open details." } ?? "Context and usage. Open details.")
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
/// leading), icons only so all eight fit one row under the prompt; each
/// keeps its words as its accessibility label. Computer · working folder ·
/// branch · worktree · permission mode on the left; the context meter and the
/// Chat/Terminal toggle on the right.
struct CodeDeckRow: View {
    let folder: CodeFolderInfo?
    /// On the terminal surface the toggle flips back to the chat view.
    var terminal: Bool = false
    @ObservedObject var context: CodeContextState
    /// False while the context bar sits above the prompt.
    var showsMeter: Bool = true
    let onOpenTerminal: () -> Void

    @StateObject private var environmentStore = EnvironmentStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var usage = UsageStore.shared
    @AppStorage(CodePermissionMode.storageKey) private var permissionRaw = CodePermissionMode.manual.rawValue
    @AppStorage(CodeWorktree.storageKey) private var worktree = false

    @State private var sheet: Sheet? = nil

    enum Sheet: String, Identifiable {
        case environment, folder, permission, pairing
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

    /// The worktree pill only shows on the default branch (isDefaultCodeBranch).
    private var worktreeVisible: Bool {
        guard let branch = folder?.branch?.lowercased() else { return false }
        return branch == "main" || branch == "master"
    }

    var body: some View {
        HStack(spacing: 2) {
            environmentButton
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
            Spacer(minLength: 4)
            if showsMeter { contextMeter }
            modeToggle
        }
        .padding(.top, 4)
        .padding(.bottom, 12)
        .padding(.horizontal, 6)
        .frame(maxWidth: .infinity)
        .task {
            usage.fetchUsageIfNeeded()
            await relay.refresh()
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
            case .pairing:
                RuntimePairingView()
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
    /// percent; with no session yet, the plan window's blocks; before either
    /// has a figure, empty blocks (a tap still opens the usage sheet).
    @ViewBuilder
    private var contextMeter: some View {
        if let context = context.breakdown {
            Button(action: { self.context.showsUsage = true }) {
                HStack(spacing: 4) {
                    CodeUsageBlocks(fraction: context.utilization, tone: tone(context.level), terminal: terminal)
                    Text("\(Int((context.utilization * 100).rounded()))%")
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundColor(context.level == .ok ? ink.secondary : tone(context.level))
                }
                .padding(.horizontal, 4)
                .frame(height: 40)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Context: \(Int((context.utilization * 100).rounded()))% used (\(SessionContextBreakdown.format(context.usedTokens)) of \(SessionContextBreakdown.format(context.contextWindow))). Open details.")
        } else {
            Button(action: { self.context.showsUsage = true }) {
                CodeUsageBlocks(fraction: (usage.percentUsed ?? 0) / 100, terminal: terminal)
                    .padding(.horizontal, 6)
                    .frame(height: 40)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(usage.percentText.map { "Usage: \($0) used. Open details." } ?? "Usage. Open details.")
        }
    }

    private func tone(_ level: SessionContextBreakdown.Level) -> Color {
        switch level {
        case .ok: return ink.secondary
        case .approaching: return Theme.statusWarning
        case .reached: return Theme.statusError
        }
    }

    /// The view switch: Code · Terminal · Shell. Its icon is the current
    /// view; the pick goes to Code mode (`.codePickView`).
    private var modeToggle: some View {
        CodeViewMenu(current: terminal ? .terminal : .code, tint: ink.primary)
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
}

/// The composer usage meter's five HUD blocks (ComposerUsageMeter
/// `HudBlocks count=5`): filled by the plan window used, amber from 80%,
/// red at the limit.
struct CodeUsageBlocks: View {
    let fraction: Double
    /// The context meter passes its own level colour.
    var tone: Color? = nil
    /// Drawn on the terminal surface (dark empty blocks).
    var terminal: Bool = false

    private var fill: Color {
        if let tone { return tone }
        if fraction >= 1 { return Theme.statusError }
        if fraction >= 0.8 { return Theme.statusWarning }
        return terminal ? TerminalPalette.secondary : Color("TextSecondary")
    }

    var body: some View {
        let filled = Int((min(max(fraction, 0), 1) * 5).rounded(.up))
        HStack(spacing: 2) {
            ForEach(0..<5, id: \.self) { index in
                RoundedRectangle(cornerRadius: 1)
                    .fill(index < filled ? fill : (terminal ? TerminalPalette.keyBorder : Color("BorderStrong")))
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
