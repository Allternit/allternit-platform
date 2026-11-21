import SwiftUI
import UIKit

// Settings › Connections › Runtimes (Harness & Bots 2.0): every runtime from
// the registry (`RuntimeRegistryStore`, GET /api/v1/runtimes), in the same
// List · Grid · Deck as the other Connections sections. A row opens the
// runtime's page: its look (RuntimeLookView on a sample session), the brains
// it accepts, the plans it may use, its live web app, and "New chat on …" /
// "New bot on …".

struct RuntimesConnectionsSection: View {
    let layout: OverviewLayout
    let query: String

    @ObservedObject private var registry = RuntimeRegistryStore.shared
    @ObservedObject private var chatRuntime = ChatRuntimeStore.shared

    private var rows: [RuntimeEntry] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return registry.runtimes.filter { r in
            q.isEmpty || [r.name, r.vendor, r.categoryLabel, "runtimes"].joined(separator: " ").lowercased().contains(q)
        }
    }

    var body: some View {
        if !rows.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text("Runtimes")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Spacer()
                    Text("\(rows.filter { ready($0) }.count) ready")
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(.top, 20)
                Text("How a chat or bot works: Allternit, or a coding agent like Claude Code or Codex on your computer.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 2)
                    .padding(.bottom, 8)
                switch registry.status {
                case .error:
                    note("Couldn't load the latest runtimes. Showing the built-in list.")
                case .stale:
                    note("You're offline. Showing the saved list of runtimes.")
                default:
                    EmptyView()
                }
                switch layout {
                case .list:
                    PhoneSettingsCard {
                        ForEach(Array(rows.enumerated()), id: \.element.id) { i, r in
                            if i > 0 { PhoneCardDivider() }
                            NavigationLink { RuntimeDetailView(runtimeId: r.id) } label: { listRow(r) }
                                .buttonStyle(.plain)
                        }
                    }
                case .grid:
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12, alignment: .top),
                                        GridItem(.flexible(), spacing: 12, alignment: .top)], spacing: 12) {
                        ForEach(rows) { r in
                            NavigationLink { RuntimeDetailView(runtimeId: r.id) } label: { card(r, faceSize: 36) }
                                .buttonStyle(.plain)
                        }
                    }
                case .deck:
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(alignment: .top, spacing: 12) {
                            ForEach(rows) { r in
                                NavigationLink { RuntimeDetailView(runtimeId: r.id) } label: { card(r, faceSize: 56) }
                                    .buttonStyle(.plain)
                                    .containerRelativeFrame(.horizontal) { w, _ in w * 0.78 }
                            }
                        }
                        .scrollTargetLayout()
                    }
                    .scrollTargetBehavior(.viewAligned)
                }
            }
            .task {
                registry.refreshIfNeeded()
                chatRuntime.refreshIfNeeded()
            }
            .accessibilityIdentifier("connections-runtimes")
        }
    }

    private func ready(_ r: RuntimeEntry) -> Bool {
        chatRuntime.availability(r, installed: registry.installed) == .ready
    }

    private func status(_ r: RuntimeEntry) -> String {
        switch chatRuntime.availability(r, installed: registry.installed) {
        case .ready: return r.id == "allternit" ? "Runs anywhere" : "Ready"
        case .checking: return "Checking…"
        case .needsComputer: return "Needs a computer"
        case .notInstalled: return "Not installed"
        }
    }

    private func note(_ text: String) -> some View {
        Label(text, systemImage: "info.circle")
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
            .padding(.bottom, 8)
    }

    private func listRow(_ r: RuntimeEntry) -> some View {
        HStack(spacing: 12) {
            ModelProviderMark(providerId: r.id, size: 28)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(r.name)
                    .font(.body)
                    .foregroundColor(Color("TextPrimary"))
                Text("\(r.categoryLabel) · \(r.brainsLabel)")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
            }
            Spacer(minLength: 8)
            Text(status(r))
                .font(.footnote)
                .foregroundColor(Color("TextTertiary"))
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .frame(minHeight: 52)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("runtimeRow-\(r.id)")
    }

    private func card(_ r: RuntimeEntry, faceSize: CGFloat) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ModelProviderMark(providerId: r.id, size: faceSize)
                .accessibilityHidden(true)
            Text(r.name)
                .font(.body.weight(.medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
            Text(r.brainsLabel)
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)
            Text(status(r))
                .font(.caption)
                .foregroundColor(Color("TextTertiary"))
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("runtimeCard-\(r.id)")
    }
}

/// One runtime's page.
struct RuntimeDetailView: View {
    let runtimeId: String

    @ObservedObject private var registry = RuntimeRegistryStore.shared
    @ObservedObject private var chatRuntime = ChatRuntimeStore.shared
    @State private var isNewBotPresented = false
    @State private var isPairingPresented = false
    @State private var look: String?

    private var runtime: RuntimeEntry? { registry.runtime(runtimeId) }

    var body: some View {
        Group {
            if let r = runtime {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        header(r)
                        lookPreview(r)
                        facts(r)
                        availabilityNote(r)
                        actions(r)
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                }
            } else {
                Text("This runtime isn't in the list anymore.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle(runtime?.name ?? "Runtime")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .task {
            registry.refreshIfNeeded()
            chatRuntime.refreshIfNeeded()
        }
        .sheet(isPresented: $isNewBotPresented) {
            NewBotSheet(runtimeId: runtimeId) { outcome in
                AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
                if let botId = outcome.botIds.first { BotModeStore.shared.pendingOpenBotId = botId }
            }
        }
        .sheet(isPresented: $isPairingPresented, onDismiss: { chatRuntime.refreshIfNeeded(force: true) }) {
            RuntimePairingView()
        }
        .accessibilityIdentifier("runtime-detail-\(runtimeId)")
    }

    private func header(_ r: RuntimeEntry) -> some View {
        HStack(spacing: 12) {
            ModelProviderMark(providerId: r.id, size: 44)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(r.name)
                    .font(.title3.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(r.categoryLabel) · \(r.vendor)")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: Look

    private static let sampleMessages: [MessageRecord] = [
        MessageRecord(id: "sample-user", role: "user", content: "Fix the failing login test."),
        MessageRecord(
            id: "sample-reply", role: "assistant",
            content: "Fixed. The token check compared seconds to milliseconds. All 12 tests pass.",
            toolCalls: [
                ToolCallRecord(id: "s1", toolName: "read", state: .done, input: ["path": "src/auth/session.ts"], output: "84 lines"),
                ToolCallRecord(id: "s2", toolName: "edit", state: .done, input: ["path": "src/auth/session.ts"], output: "1 line changed"),
                ToolCallRecord(id: "s3", toolName: "bash", state: .done, input: ["command": "npm test"], output: "12 passed"),
            ]
        ),
    ]

    @ViewBuilder
    private func lookPreview(_ r: RuntimeEntry) -> some View {
        let looks = RuntimeLookTokens.looks(r)
        let current = look ?? r.defaultLook
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Its look")
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                Spacer()
                if looks.count > 1 {
                    Picker("Look", selection: Binding(get: { current }, set: { look = $0 })) {
                        ForEach(looks, id: \.self) { l in Text(l == "desktop" ? "Desktop" : "Terminal").tag(l) }
                    }
                    .pickerStyle(.segmented)
                    .fixedSize()
                }
            }
            if let tokens = RuntimeLookTokens.of(r, look: current) {
                RuntimeLookView(runtime: r, tokens: tokens, messages: Self.sampleMessages, isStreaming: false,
                                onSend: { _ in }, onStop: {})
                    .frame(height: 380)
                    .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle")))
                    .allowsHitTesting(false)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("A sample session drawn the way \(r.name) looks")
            } else {
                Text("\(r.name) uses the Allternit chat look.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
    }

    // MARK: Facts

    private func facts(_ r: RuntimeEntry) -> some View {
        let access = r.access.map(\.label)
        return PhoneSettingsCard {
            fact("Brains it accepts", r.brainsLabel + (access.isEmpty ? "" : " · " + access.joined(separator: ", ")))
            PhoneCardDivider()
            fact("Plans allowed", RuntimeRegistry.plansLabel(r)
                 .map { "\($0). Other plans need an API key here." }
                 ?? "No subscription plans. Use an API key or a local model.")
            PhoneCardDivider()
            fact("Live web app", r.hasLiveApp
                 ? "Yes. Its own web app opens inside Allternit while it runs."
                 : "No. Allternit draws its look from the registry.")
            if let label = r.status?.label {
                PhoneCardDivider()
                fact("Status", label)
            }
        }
    }

    private func fact(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.footnote)
                .foregroundColor(Color("TextTertiary"))
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }

    // MARK: Availability + actions

    @ViewBuilder
    private func availabilityNote(_ r: RuntimeEntry) -> some View {
        let availability = chatRuntime.availability(r, installed: registry.installed)
        if let message = availability.message(r.name) {
            VStack(alignment: .leading, spacing: 8) {
                Label(message, systemImage: availability == .needsComputer ? "desktopcomputer" : "exclamationmark.triangle")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                if availability == .needsComputer {
                    secondaryButton("Pair a computer") { isPairingPresented = true }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        }
    }

    private func actions(_ r: RuntimeEntry) -> some View {
        let ready = chatRuntime.availability(r, installed: registry.installed) == .ready
        return VStack(spacing: 10) {
            Button {
                Haptics.selection()
                chatRuntime.runtimeId = r.id
                NotificationCenter.default.post(name: .shellNewChatOnRuntime, object: nil)
            } label: {
                Text("New chat on \(r.name)")
                    .font(.body.weight(.semibold))
                    .foregroundColor(Color("BgPrimary"))
                    .frame(maxWidth: .infinity, minHeight: 50)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("TextPrimary")))
                    .opacity(ready ? 1 : 0.45)
            }
            .buttonStyle(.plain)
            .disabled(!ready)
            .accessibilityHint(ready ? "Opens a new chat that runs on \(r.name)" : (chatRuntime.availability(r, installed: registry.installed).message(r.name) ?? ""))
            .accessibilityIdentifier("runtime-new-chat")

            Button {
                Haptics.selection()
                isNewBotPresented = true
            } label: {
                Text("New bot on \(r.name)")
                    .font(.body.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, minHeight: 50)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle")))
            }
            .buttonStyle(.plain)
            .accessibilityHint("Opens Create bot with \(r.name) picked")
            .accessibilityIdentifier("runtime-new-bot")
        }
    }

    private func secondaryButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(Capsule().fill(Color("BgPanel")))
                .overlay(Capsule().stroke(Color("BorderSubtle")))
        }
        .buttonStyle(.plain)
    }
}
