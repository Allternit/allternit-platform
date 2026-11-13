import SwiftUI

/// Drives one session on the self-hosted server: events are the single source
/// of truth, de-duplicated by `seq`, fed by the SSE stream.
@MainActor
final class SelfHostedSessionModel: ObservableObject {
    @Published var agentID: String
    @Published var environmentID: String
    @Published private(set) var events: [SHEvent] = []
    @Published private(set) var status: SHSessionStatus = .idle
    @Published private(set) var statusReason: String?
    @Published private(set) var sending = false
    @Published var error: String?

    private var session: SHSession?
    private var streamTask: Task<Void, Never>?
    private let store = SelfHostedServerStore.shared

    init() {
        agentID = store.agents.first?.id ?? ""
        environmentID = (store.environments.first(where: { ($0.workerCount ?? 0) > 0 }) ?? store.environments.first)?.id ?? ""
    }

    var started: Bool { session != nil }
    var selectedEnvironment: SHEnvironment? { store.environments.first { $0.id == environmentID } }
    var waitingForWorker: Bool { status == .waitingForWorker }
    var busy: Bool { sending || status == .running || status == .waitingForWorker }

    func send(_ text: String) async {
        guard let client = store.client(), !agentID.isEmpty, !environmentID.isEmpty else { return }
        sending = true; error = nil
        defer { sending = false }
        do {
            if session == nil {
                let created = try await client.createSession(agentID: agentID, environmentID: environmentID)
                session = created
                startStream(client: client, sessionID: created.id)
            }
            if let id = session?.id { try await client.sendMessage(sessionID: id, content: text) }
        } catch let e as SelfHostedError {
            switch e {
            case .server(_, "session_busy"?, _): error = "The agent is still working on the last message."
            case .server(_, "model_not_mapped"?, _): error = "This agent’s model isn’t set up on the server (model_not_mapped). Pick another agent."
            default: error = e.message
            }
        } catch { self.error = error.localizedDescription }
    }

    func stop() async {
        guard let client = store.client(), let id = session?.id else { return }
        try? await client.interrupt(sessionID: id)
    }

    func leave() { streamTask?.cancel() }

    private func startStream(client: SelfHostedClient, sessionID: String) {
        streamTask?.cancel()
        streamTask = Task { [weak self] in
            do {
                for try await event in client.streamEvents(sessionID: sessionID) {
                    self?.apply(event)
                }
            } catch let e as SelfHostedError {
                self?.error = e.message
            } catch {}
        }
    }

    private func apply(_ event: SHEvent) {
        guard !events.contains(where: { $0.seq == event.seq }) else { return }
        events.append(event)
        if case .status(let s, let reason) = event.payload, let s {
            status = s; statusReason = reason
        }
    }
}

struct SelfHostedSessionView: View {
    @ObservedObject private var store = SelfHostedServerStore.shared
    @StateObject private var model = SelfHostedSessionModel()
    @State private var draft = ""

    var body: some View {
        VStack(spacing: 0) {
            if !model.started { pickers }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        if model.events.isEmpty {
                            Text("Say something to start. Replies and tool calls stream in here.")
                                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                                .padding(.top, 24)
                        }
                        ForEach(model.events) { event in
                            EventRow(event: event).id(event.seq)
                        }
                    }
                    .padding(16)
                }
                .onChange(of: model.events.count) { _, _ in
                    if let last = model.events.last { withAnimation { proxy.scrollTo(last.seq, anchor: .bottom) } }
                }
            }
            if model.waitingForWorker {
                banner("Waiting for a worker", detail: "No worker is attached to this environment yet. Start one on your server and this turn continues by itself.", color: Theme.statusWarning, spinner: true)
            } else if !model.started, let env = model.selectedEnvironment, (env.workerCount ?? 0) == 0 {
                banner("No workers online here", detail: "Model-only replies work. A turn that runs a tool waits for a worker.", color: Theme.statusWarning, spinner: false)
            }
            if let error = model.error { SettingsErrorBanner(message: error).padding(.horizontal, 16).padding(.top, 8) }
            composer
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Session")
        .navigationBarTitleDisplayMode(.inline)
        .onDisappear { model.leave() }
    }

    private var pickers: some View {
        VStack(spacing: 8) {
            Picker("Agent", selection: $model.agentID) {
                ForEach(store.agents) { Text($0.name).tag($0.id) }
            }
            Picker("Environment", selection: $model.environmentID) {
                ForEach(store.environments) { env in
                    Text("\(env.name) (\(env.workerCount ?? 0) online)").tag(env.id)
                }
            }
        }
        .pickerStyle(.menu)
        .padding(.horizontal, 16).padding(.top, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func banner(_ title: String, detail: String, color: Color, spinner: Bool) -> some View {
        HStack(alignment: .top, spacing: 10) {
            if spinner { ProgressView() } else { Image(systemName: "exclamationmark.triangle") }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 14, weight: .semibold))
                Text(detail).font(.system(size: 13)).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .foregroundColor(color)
        .padding(12)
        .background(color.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .padding(.horizontal, 16).padding(.top, 8)
        .accessibilityElement(children: .combine)
    }

    private var composer: some View {
        HStack(spacing: 10) {
            TextField("Message", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .modifier(SHFieldStyle())
            if model.busy && model.started {
                Button("Stop") { Task { await model.stop() } }.buttonStyle(SettingsQuietButtonStyle())
            } else {
                Button("Send") {
                    let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !text.isEmpty else { return }
                    draft = ""
                    Task { await model.send(text) }
                }
                .buttonStyle(SettingsPrimaryButtonStyle())
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || model.sending || model.agentID.isEmpty)
            }
        }
        .padding(16)
    }
}

private struct EventRow: View {
    let event: SHEvent

    var body: some View {
        switch event.payload {
        case .message(let role, let content):
            let mine = role == "user"
            Text(content)
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .textSelection(.enabled)
                .padding(12)
                .background(mine ? Color("FillNeutral") : Color.clear)
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                .frame(maxWidth: .infinity, alignment: mine ? .trailing : .leading)
        case .toolCall(_, let name, let input):
            card(icon: "wrench.and.screwdriver", title: "Tool: \(name)", body: input, tint: Color("TextSecondary"))
        case .toolResult(_, let output, let isError):
            card(icon: isError ? "xmark.octagon" : "checkmark.circle", title: isError ? "Tool failed" : "Tool result",
                 body: output, tint: isError ? Theme.statusError : Color("TextSecondary"))
        case .error(_, let message):
            SettingsErrorBanner(message: message)
        case .status, .unknown:
            EmptyView()
        }
    }

    private func card(icon: String, title: String, body: String, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: icon).font(.system(size: 13, weight: .medium)).foregroundColor(tint)
            if !body.isEmpty {
                Text(String(body.prefix(1200)))
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .textSelection(.enabled)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}
