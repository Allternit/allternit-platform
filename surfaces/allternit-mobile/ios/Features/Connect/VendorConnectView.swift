import SwiftUI

// -----------------------------------------------------------------------------
// Connect a vendor bot (ChatGPT, Claude, Gemini, Grok Bot, Muse, …) with the
// web's one-step connect (allternit-ai src/components/gateway/QuickConnect.tsx
// `useOneStepConnect`): each press is one `POST gateway/connect`. Website
// products open their sign-in window on the person's computer (the gateway's
// machine), so while it says "needs_sign_in" the phone re-asks every 3 s for
// at most ~5 minutes until they've signed in there.
// -----------------------------------------------------------------------------

@MainActor
final class OneStepConnect: ObservableObject {
    let product: ConnectProduct
    @Published private(set) var result: ConnectResult?
    @Published private(set) var busy = false
    @Published private(set) var waiting = false
    @Published var error: String?
    @Published var fields: [String: String] = [:]

    private var polls = 0
    private var timer: Task<Void, Never>?
    var onReady: ((ConnectResult) -> Void)?

    init(product: ConnectProduct) { self.product = product }

    var extras: [(key: String, label: String, secret: Bool)] { VendorConnectAPI.extraFields(product: product.product, result: result) }
    var consent: String? { result?.status == "needs_consent" ? (result?.terms ?? product.terms) : nil }
    var bots: [ConnectedBot] { result?.bots ?? product.bots }
    var connected: Bool { (result?.status ?? (product.isConnected ? "ready" : nil)) == "ready" }

    func run(poll: Bool = false, acceptTerms: Bool = false) {
        stop()
        if !poll { polls = 0 }
        busy = true
        error = nil
        Task {
            do {
                let r = try await VendorConnectAPI.connect(product: product.product, fields: fields, acceptTerms: acceptTerms)
                result = r
                if r.status == "ready" { onReady?(r) }
                if r.status == "needs_sign_in" && polls < 100 {
                    polls += 1
                    waiting = true
                    timer = Task { [weak self] in
                        try? await Task.sleep(nanoseconds: 3_000_000_000)
                        guard !Task.isCancelled else { return }
                        self?.run(poll: true)
                    }
                }
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }

    func stop() {
        timer?.cancel()
        timer = nil
        waiting = false
    }
}

/// The products list (Quick connect) with each product's own flow one tap away.
struct VendorConnectView: View {
    /// Offer "Make it my main bot" on the connected bot (Meet your bots → who leads).
    var offerMain = false
    /// A vendor bot was connected; `mainBotId` is set when it was made the main bot.
    var onConnected: ((_ firstBotId: String?, _ mainBotId: String?) -> Void)? = nil

    @ObservedObject private var auth = AuthManager.shared
    @State private var products: [ConnectProduct]?
    @State private var unsupported = false
    @State private var loadError: String?
    @State private var open: ConnectProduct?

    var body: some View {
        List {
            if !auth.isSignedIn {
                FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to connect a bot",
                                  message: "Vendor bots connect to your Allternit account.",
                                  actionTitle: "Sign in", action: { auth.isPresentingAuth = true })
                    .listRowBackground(Color.clear)
            } else if unsupported {
                FriendlyStateView(style: .empty, icon: "arrow.down.circle", title: "Update Allternit on your computer",
                                  message: "One-step connect needs a newer Allternit on your computer. Update it, then try again.")
                    .listRowBackground(Color.clear)
            } else if let loadError {
                FriendlyStateView(style: .offline, icon: "desktopcomputer.trianglebadge.exclamationmark", title: "Couldn’t reach your computer",
                                  message: "Vendor bots connect through it. \(loadError)", actionTitle: "Retry", action: { Task { await load() } })
                    .listRowBackground(Color.clear)
            } else if let products {
                Section {
                    ForEach(products) { p in
                        Button { open = p } label: {
                            HStack(spacing: 12) {
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(p.name).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                    Text(p.offers).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2)
                                    Text(p.isConnected ? "Connected · \(p.bots.count) bot\(p.bots.count == 1 ? "" : "s")" : p.signInText)
                                        .font(.caption2.weight(.semibold))
                                        .foregroundColor(p.isConnected ? Theme.statusSuccess : Color("TextSecondary"))
                                }
                                Spacer(minLength: 8)
                                Image(systemName: "chevron.right").font(.caption).foregroundColor(Color("TextSecondary"))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                } footer: {
                    Text("Use a subscription you already pay for. Connect once and every chat, GPT, Project, Gem or bot you have there becomes an Allternit bot.")
                }
            } else {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading…").foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .navigationTitle("Connect a vendor bot")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task { if products == nil { await load() } }
        .sheet(item: $open) { p in
            NavigationStack {
                VendorProductView(product: p, offerMain: offerMain) { first, main in
                    onConnected?(first, main)
                }
            }
        }
    }

    private func load() async {
        guard auth.isSignedIn else { return }
        loadError = nil
        do {
            if let list = try await VendorConnectAPI.catalog() {
                products = list
            } else {
                unsupported = true
            }
        } catch {
            loadError = error.localizedDescription
        }
    }
}

/// One product's one-step connect: status, terms, extra fields, the bots it made.
struct VendorProductView: View {
    let product: ConnectProduct
    var offerMain = false
    var onConnected: (_ firstBotId: String?, _ mainBotId: String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @StateObject private var model: OneStepConnect
    @State private var makeMain: Bool
    @State private var mainNote: String?
    @State private var reported = false

    init(product: ConnectProduct, offerMain: Bool, onConnected: @escaping (_ firstBotId: String?, _ mainBotId: String?) -> Void) {
        self.product = product
        self.offerMain = offerMain
        self.onConnected = onConnected
        _model = StateObject(wrappedValue: OneStepConnect(product: product))
        _makeMain = State(initialValue: offerMain)
    }

    var body: some View {
        List {
            Section {
                Text(product.offers).font(.subheadline).foregroundColor(Color("TextSecondary"))
                if !product.verifiedLive {
                    Label("Not yet verified live — no real account has been run through this connection yet.", systemImage: "info.circle")
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
            if let r = model.result {
                Section("Status") {
                    HStack(spacing: 8) {
                        if model.waiting { ProgressView().controlSize(.small) }
                        Text(r.statusText).font(.subheadline.weight(.semibold))
                            .foregroundColor(r.status == "ready" ? Theme.statusSuccess : r.status == "blocked" ? Theme.statusError : Color("TextPrimary"))
                    }
                    if !r.message.isEmpty {
                        Text(r.status == "needs_sign_in"
                             ? "A \(product.name) window is open on your computer. Sign in there; this checks again on its own."
                             : r.message)
                            .font(.footnote).foregroundColor(Color("TextSecondary"))
                    }
                    if r.status != "ready" {
                        ForEach(Array(r.steps.enumerated()), id: \.offset) { i, s in
                            Text("\(i + 1). \(s)").font(.footnote).foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
            }
            if let consent = model.consent {
                Section("Before you connect") {
                    Text(consent).font(.footnote).foregroundColor(Color("TextSecondary"))
                }
            }
            if !model.extras.isEmpty {
                Section("Details") {
                    ForEach(Array(model.extras.enumerated()), id: \.offset) { _, f in
                        let b = Binding(get: { model.fields[f.key] ?? "" }, set: { model.fields[f.key] = $0 })
                        Group {
                            if f.secret { SecureField(f.label, text: b) } else { TextField(f.label, text: b) }
                        }
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    }
                }
            }
            if !model.bots.isEmpty {
                Section("Bots") {
                    ForEach(model.bots, id: \.botId) { b in Label(b.name, systemImage: "person.crop.circle") }
                    if let mainNote { Text(mainNote).font(.footnote).foregroundColor(Color("TextSecondary")) }
                }
            }
            if offerMain && !model.connected {
                Section {
                    Toggle("Make it my main bot", isOn: $makeMain).tint(Color("AccentPrimary"))
                }
            }
            if let error = model.error {
                Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
            }
        }
        .navigationTitle(product.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Close") { model.stop(); dismiss() } }
        }
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 12) {
                if model.waiting {
                    Button("Stop waiting") { model.stop() }
                }
                Spacer()
                Button {
                    if model.connected && reported { dismiss(); return }
                    model.run(acceptTerms: model.consent != nil)
                } label: {
                    Text(buttonTitle).font(.headline).padding(.horizontal, 6).frame(minHeight: 40)
                }
                .buttonStyle(.borderedProminent)
                .tint(Color("AccentPrimary"))
                .disabled(model.busy && !model.waiting)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .background(.bar)
        }
        .onAppear {
            model.onReady = { r in Task { await ready(r) } }
        }
        .onDisappear { model.stop() }
    }

    private var buttonTitle: String {
        if model.connected && reported { return "Done" }
        if model.busy && !model.waiting { return "Connecting…" }
        if model.consent != nil { return "I understand, connect" }
        if model.waiting { return "I’ve signed in" }
        if model.connected { return "Refresh bots" }
        return model.result == nil ? "Connect" : "Try again"
    }

    /// useBotSetup onVendorReady: optionally make the first joined bot the main bot.
    private func ready(_ r: ConnectResult) async {
        let first = r.bots.first?.botId
        var main: String?
        if makeMain && offerMain, let first {
            do {
                let roster = try await MeetAgentsAPI.roster()
                guard roster.contains(where: { $0.id == first }) else {
                    throw MeetFinishError("the new bot isn’t in your bots list yet. Make it your main bot from its page.")
                }
                try await MeetMainBot.set(first, in: roster)
                main = first
                mainNote = "\(r.bots.first?.name ?? "It") is your main bot."
            } catch {
                mainNote = "Not made your main bot: \(error.localizedDescription)"
            }
        }
        if r.bots.isEmpty { mainNote = "Connected. No bots found on this account yet." }
        AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
        reported = true
        onConnected(first, main)
    }
}
