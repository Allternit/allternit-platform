import SwiftUI

// MARK: - Launch composer

/// The Code launch prompt on the phone (phone.css "Terminal-style composer:
/// one square line, prompt and keys side by side"): a dashed hairline, `+`,
/// the coral `❯` prompt with a mono field, the voice key, then the deck row
/// under it (computer · folder · branch · permission mode · model ·
/// Terminal). Sending starts a code thread with the text.
struct CodeLaunchComposer: View {
    @Binding var text: String
    let folder: CodeFolderInfo?
    let onSend: (String) -> Void
    let onOpenTerminal: () -> Void
    let onAttach: () -> Void

    @StateObject private var dictation = DictationController()
    @State private var dictationBase = ""
    @FocusState private var isFocused: Bool

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(spacing: 0) {
            DashedHairline()
            HStack(alignment: .center, spacing: 2) {
                Button(action: onAttach) {
                    Image(systemName: "plus")
                        .font(.system(size: 18, weight: .regular))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Attach")

                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("❯")
                        .font(.system(size: 15, weight: .semibold, design: .monospaced))
                        .foregroundColor(GizziCodeWordmark.coral)
                        .accessibilityHidden(true)
                    TextField("Run a command or describe a task...", text: $text, axis: .vertical)
                        .font(.system(size: 16, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1...6)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.send)
                        .focused($isFocused)
                        .onSubmit(send)
                }
                .padding(.leading, 12)
                .padding(.vertical, 9)
                .frame(maxWidth: .infinity, alignment: .leading)

                if trimmed.isEmpty {
                    Button(action: toggleDictation) {
                        Image(systemName: dictation.isRecording ? "waveform.circle.fill" : "waveform")
                            .font(.system(size: 17))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(width: 36, height: 36)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(dictation.isRecording ? "Stop voice input" : "Start voice mode")
                } else {
                    Button(action: send) {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 14, weight: .bold))
                            .foregroundColor(Color("OnAccent"))
                            .frame(width: 30, height: 30)
                            .background(Color("AccentPrimary"))
                            .clipShape(Circle())
                            .frame(width: 36, height: 36)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Send")
                }
            }
            .padding(.trailing, 6)
            DashedHairline()
            CodeDeckRow(folder: folder, onOpenTerminal: onOpenTerminal)
        }
        .onChange(of: dictation.transcript) { _, transcript in
            guard dictation.isRecording || !transcript.isEmpty else { return }
            text = dictationBase + transcript
        }
    }

    private func send() {
        let value = trimmed
        guard !value.isEmpty else { return }
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
    var body: some View {
        GeometryReader { geo in
            Path { path in
                path.move(to: CGPoint(x: 0, y: 0.5))
                path.addLine(to: CGPoint(x: geo.size.width, y: 0.5))
            }
            .stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
        }
        .frame(height: 1)
        .accessibilityHidden(true)
    }
}

// MARK: - Deck row

/// The quiet row under the Code prompt (CodeWorkspaceBar + CodeBottomStatusBar
/// on the phone): one line that scrolls sideways — Environment, Working
/// folder, branch, permission mode, model, and the Terminal key.
struct CodeDeckRow: View {
    let folder: CodeFolderInfo?
    let onOpenTerminal: () -> Void

    @StateObject private var environmentStore = EnvironmentStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @StateObject private var modelStore = ModelStore.shared
    @AppStorage(CodePermissionMode.storageKey) private var permissionRaw = CodePermissionMode.manual.rawValue

    @State private var sheet: Sheet? = nil

    enum Sheet: String, Identifiable {
        case environment, folder, permission, model, pairing
        var id: String { rawValue }
    }

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

    private var modelLabel: String {
        guard let model = modelStore.selectedModel else { return modelStore.pillLabel }
        return "\(model.provider) · \(model.name)"
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                pill(label: "Environment", action: { sheet = .environment }) {
                    Image(systemName: environmentStore.environment == .cloud ? "cloud" : "desktopcomputer")
                        .font(.system(size: 13))
                    Text(environmentLabel).lineLimit(1)
                    if let cloudStatus {
                        Text(cloudStatus)
                            .font(.system(size: 11))
                            .foregroundColor(Color("BrandAccent"))
                    }
                    caret
                }
                pill(label: "Working folder", action: { sheet = .folder }) {
                    Image(systemName: "folder").font(.system(size: 13))
                    Text(folder?.name ?? "Select folder").lineLimit(1).frame(maxWidth: 150, alignment: .leading)
                    caret
                }
                if let branch = folder?.branch, !branch.isEmpty {
                    pill(label: "Branch \(branch)", action: nil) {
                        Image(systemName: "arrow.triangle.branch").font(.system(size: 12))
                        Text(branch).lineLimit(1).frame(maxWidth: 150, alignment: .leading)
                    }
                }
                Button(action: { sheet = .permission }) {
                    HStack(spacing: 5) {
                        Image(systemName: permission.icon).font(.system(size: 12)).opacity(0.85)
                        Text(permission.label)
                    }
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("AccentPrimary"))
                    .padding(.horizontal, 8)
                    .frame(height: 30)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Code permission mode")
                pill(label: "Model", action: { sheet = .model }) {
                    Text(modelLabel).lineLimit(1)
                }
                Button(action: onOpenTerminal) {
                    HStack(spacing: 5) {
                        Image(systemName: "apple.terminal").font(.system(size: 13))
                        Text("Terminal")
                    }
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .frame(height: 30)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Open terminal")
            }
            .padding(.top, 6)
            .padding(.bottom, 14)
            .padding(.horizontal, 6)
        }
        .task { await relay.refresh() }
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
            }
        }
    }

    private var caret: some View {
        Image(systemName: "chevron.down")
            .font(.system(size: 9, weight: .semibold))
            .foregroundColor(Color("TextTertiary"))
    }

    @ViewBuilder
    private func pill<Content: View>(label: String, action: (() -> Void)?, @ViewBuilder content: () -> Content) -> some View {
        let chip = HStack(spacing: 6) { content() }
            .font(.system(size: 13))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 8)
            .frame(height: 36)
        if let action {
            Button(action: action) { chip }
                .buttonStyle(.plain)
                .accessibilityLabel(label)
        } else {
            chip.accessibilityElement(children: .combine).accessibilityLabel(label)
        }
    }
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
