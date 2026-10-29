import SwiftUI

// -----------------------------------------------------------------------------
// Meet your bots → Who leads → a vendor bot (Claude, Duet, ChatGPT, Grok,
// Meta AI), connected right in the step the way the web's useBotSetup vendor
// mode and BotSetupPanels do: the pack's head, "How to connect" profiles,
// "It will be able to", the terms warning, extra fields, the connect state
// and "Make it my main bot". The packs come from the gateway catalog
// (`GET gateway/catalog`); each press is one one-step connect
// (`POST gateway/connect`, OneStepConnect).
// -----------------------------------------------------------------------------

/// One way to connect a vendor (vendor-packs.ts ConnectionProfile).
struct MeetVendorProfile: Identifiable, Sendable {
    let id: String
    let label: String
    let authType: String
    let lane: String
    let recommended: Bool
    let permissionDescription: [String]
    let termsWarning: String?
    let requiresUserOwnedSubscription: Bool

    init?(_ v: JSONValue) {
        guard let id = v.at("id")?.jString else { return nil }
        self.id = id
        label = v.at("label")?.jString ?? id
        authType = v.at("authType")?.jString ?? ""
        lane = v.at("lane")?.jString ?? ""
        recommended = v.at("recommended")?.jBool ?? false
        permissionDescription = (v.at("permissionDescription")?.jArray ?? []).compactMap(\.jString)
        termsWarning = v.at("termsWarning")?.jString
        requiresUserOwnedSubscription = v.at("requiresUserOwnedSubscription")?.jBool ?? false
    }
}

/// A vendor pack (vendor-packs.ts VendorPack), the parts the step shows.
struct MeetVendorPack: Identifiable, Sendable {
    let id: String
    let displayName: String
    let legalName: String
    let profiles: [MeetVendorProfile]

    init?(_ v: JSONValue) {
        guard let id = v.at("id")?.jString else { return nil }
        self.id = id
        displayName = v.at("displayName")?.jString ?? id
        legalName = v.at("legalName")?.jString ?? ""
        profiles = (v.at("profiles")?.jArray ?? []).compactMap(MeetVendorProfile.init)
    }

    /// bot-looks defaultProfileIndex: the recommended profile, else the first.
    var defaultProfileIndex: Int { profiles.firstIndex(where: \.recommended) ?? 0 }

    /// bot-looks vendorSite: where the person signs in.
    var site: String { id == "meta" ? "WhatsApp" : id == "xai" ? "Grok" : displayName }

    /// bot-looks connectStartLabel.
    func startLabel(_ p: MeetVendorProfile) -> String {
        switch p.authType {
        case "browser_session": return "Sign in to \(site)"
        case "channel_oauth": return "Authorize WhatsApp"
        case "desktop_session": return "Connect the \(displayName) app"
        default: return "Connect"
        }
    }
}

/// The step-level state of the vendor connect (useBotSetup vendor mode).
@MainActor
final class MeetVendorSetup: ObservableObject {
    enum Phase { case pick, signing, consent, waiting, checking, needs, failed, connected }

    /// The picked vendor look (nil = another tile is picked).
    @Published private(set) var lookId: String?
    @Published private(set) var packs: [MeetVendorPack]?
    @Published private(set) var loadError: String?
    @Published private var profiles: [String: Int] = [:]
    @Published private var mainToo: [String: Bool] = [:]
    @Published private(set) var connect: OneStepConnect?
    @Published var note: String?
    /// Re-render when the connect model changes.
    private var connectWatch: Task<Void, Never>?

    /// bot-looks: which vendor pack each vendor look connects.
    static let packForLook = ["claude": "anthropic", "duet": "claude-duet", "dots": "openai", "grok": "xai", "muse": "meta"]
    /// bot-looks PROFILE_CONNECT_PRODUCT: the one-step product behind a profile.
    static let profileProduct = [
        "anthropic-browser": "claude", "anthropic-key": "claude-managed-agents", "claude-duet-browser": "claude-duet",
        "openai-chat": "chatgpt", "openai-browser": "chatgpt-dots", "openai-desktop": "chatgpt-dots",
        "xai-browser": "grok-bot", "meta-whatsapp": "muse",
    ]

    var pack: MeetVendorPack? {
        guard let lookId, let packId = Self.packForLook[lookId] else { return nil }
        return packs?.first { $0.id == packId }
    }
    var profileIndex: Int { pack.map { profiles[lookId ?? ""] ?? $0.defaultProfileIndex } ?? 0 }
    var profile: MeetVendorProfile? {
        guard let pack, pack.profiles.indices.contains(profileIndex) else { return nil }
        return pack.profiles[profileIndex]
    }
    var product: String? { profile.flatMap { Self.profileProduct[$0.id] } }
    /// Onboarding: on unless turned off.
    var makeMain: Bool {
        get { mainToo[lookId ?? ""] ?? true }
        set { if let lookId { mainToo[lookId] = newValue } }
    }

    var phase: Phase {
        guard let c = connect else { return .pick }
        if c.busy { return c.result == nil ? .signing : .checking }
        if c.error != nil { return .failed }
        switch c.result?.status {
        case nil: return .pick
        case "ready": return .connected
        case "needs_consent": return .consent
        case "needs_sign_in": return .waiting
        case "needs_key": return .needs
        default: return .failed
        }
    }

    var busy: Bool { phase == .signing || phase == .checking }
    var canCancel: Bool { [.waiting, .signing, .checking, .consent].contains(phase) }

    var primaryLabel: String {
        guard let pack, let profile else { return packs == nil && loadError == nil ? "Loading…" : "Connect" }
        if product == nil { return pack.startLabel(profile) }
        switch phase {
        case .signing: return "Opening sign-in…"
        case .checking: return "Checking…"
        case .consent: return "I understand, connect"
        case .waiting: return profile.authType == "channel_oauth" ? "I’ve authorized" : "I’ve signed in"
        case .needs: return "Connect"
        case .failed: return "Try again"
        default: return pack.startLabel(profile)
        }
    }

    /// MainBotPanel bubbleText for a vendor.
    func bubble(caption: String) -> String {
        "Bring the \(caption) you already use. It keeps its own memory and leads your team."
    }

    func select(_ id: String?) {
        guard id != lookId else { return }
        connect?.stop()
        lookId = id
        note = nil
        resetConnect()
        if id != nil, packs == nil { Task { await load() } }
    }

    func setProfile(_ i: Int) {
        guard let lookId else { return }
        profiles[lookId] = i
        resetConnect()
    }

    func cancel() {
        connect?.stop()
        resetConnect()
    }

    /// The primary button: one connect press (with consent on "I understand").
    func primary() {
        guard let connect else { return }
        connect.run(acceptTerms: phase == .consent)
    }

    func load() async {
        loadError = nil
        do {
            let v = try await ConnectHTTP.call("GET", "gateway/catalog")
            packs = (v.at("packs")?.jArray ?? []).compactMap(MeetVendorPack.init)
            resetConnect()
        } catch {
            loadError = error.localizedDescription
        }
    }

    /// A different vendor or profile starts clean.
    private func resetConnect() {
        connectWatch?.cancel()
        guard let product else {
            connect = nil
            return
        }
        let c = OneStepConnect(product: ConnectProduct.placeholder(product))
        c.onReady = { [weak self] r in Task { await self?.ready(r) } }
        connect = c
        connectWatch = Task { [weak self, weak c] in
            guard let c else { return }
            for await _ in c.objectWillChange.values {
                self?.objectWillChange.send()
            }
        }
    }

    var onConnected: ((_ firstBotId: String?, _ mainBotId: String?, _ summary: String) -> Void)?

    /// useBotSetup onVendorReady: the joined bots, the first made main when asked.
    private func ready(_ r: ConnectResult) async {
        let first = r.bots.first?.botId
        var main: String?
        let name = pack?.displayName ?? "Vendor bot"
        if makeMain, let first {
            do {
                let roster = try await MeetAgentsAPI.roster()
                guard roster.contains(where: { $0.id == first }) else {
                    throw MeetFinishError("the new bot isn’t in your bots list yet. Make it your main bot from its page.")
                }
                try await MeetMainBot.set(first, in: roster)
                main = first
            } catch {
                note = "Not made your main bot: \(error.localizedDescription)"
            }
        }
        AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
        let summary = r.bots.isEmpty
            ? "\(name) connected. No bots found on this account yet."
            : "\(name) connected · \(r.bots.prefix(3).map { "\($0.name) joins your bots" }.joined(separator: " · "))"
        onConnected?(first, main, summary)
    }
}

extension ConnectProduct {
    /// A product known only by id (the vendor step connects by profile).
    static func placeholder(_ product: String) -> ConnectProduct {
        ConnectProduct(.object(["product": .string(product)]))!
    }
}

/// The vendor panel under the picks (BotSetupPanels mode "vendor").
struct MeetVendorPanel: View {
    @ObservedObject var setup: MeetVendorSetup
    let look: MeetLookTile.Vendor
    var onStepByStep: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let error = setup.loadError {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Couldn’t load the vendor list").font(.system(size: 13, weight: .semibold)).foregroundStyle(MeetPalette.ink)
                    Text(error).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub)
                    Button("Try again") { Task { await setup.load() } }
                        .font(.system(size: 13, weight: .medium)).foregroundStyle(MeetPalette.ink)
                }
            } else if let pack = setup.pack {
                head(pack)
                profiles(pack)
                perms(pack)
                fields
                state(pack)
                if setup.product == nil {
                    Text("This one connects step by step: it opens the full setup in its own window.")
                        .font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub)
                }
                Toggle(isOn: Binding(get: { setup.makeMain }, set: { setup.makeMain = $0 })) {
                    Text("Make it my main bot").font(.system(size: 13)).foregroundStyle(MeetPalette.ink)
                }
                .toggleStyle(MeetCheckStyle())
                if let note = setup.note {
                    Text(note).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub).fixedSize(horizontal: false, vertical: true)
                }
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading…").font(.system(size: 13)).foregroundStyle(MeetPalette.sub)
                }
            }
        }
        .padding(.bottom, 12)
    }

    private func head(_ pack: MeetVendorPack) -> some View {
        HStack(spacing: 10) {
            MeetLookTile(id: look.id, size: 36)
            VStack(alignment: .leading, spacing: 1) {
                Text("Connect \(look.label == pack.displayName ? pack.displayName : "\(look.label) · \(pack.displayName)")")
                    .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(MeetPalette.ink).lineLimit(1)
                if !pack.legalName.isEmpty {
                    Text(pack.legalName).font(.system(size: 11.5)).foregroundStyle(MeetPalette.mut).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            Text(setup.profile?.lane == "official" ? "Official API" : "Connected through Allternit")
                .font(.system(size: 10.5, weight: .medium))
                .foregroundStyle(MeetPalette.sub)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(MeetPalette.fill, in: Capsule())
                .lineLimit(1)
        }
    }

    @ViewBuilder
    private func profiles(_ pack: MeetVendorPack) -> some View {
        if setup.phase == .pick || setup.phase == .failed {
            VStack(spacing: 6) {
                ForEach(Array(pack.profiles.enumerated()), id: \.element.id) { i, p in
                    Button { setup.setProfile(i) } label: {
                        HStack(spacing: 10) {
                            Circle()
                                .strokeBorder(i == setup.profileIndex ? MeetPalette.ink : MeetPalette.lineDash, lineWidth: i == setup.profileIndex ? 5 : 1.5)
                                .frame(width: 16, height: 16)
                            Text(p.label).font(.system(size: 13)).foregroundStyle(MeetPalette.ink)
                            Spacer(minLength: 4)
                            if p.recommended {
                                Text("Recommended").font(.system(size: 11)).foregroundStyle(MeetPalette.mut)
                            }
                        }
                        .padding(.horizontal, 12)
                        .frame(minHeight: 44)
                        .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(i == setup.profileIndex ? MeetPalette.ink : MeetPalette.line))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(i == setup.profileIndex ? [.isSelected] : [])
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("How to connect")
        } else if let p = setup.profile {
            Text(p.label).font(.system(size: 13, weight: .medium)).foregroundStyle(MeetPalette.ink)
        }
    }

    @ViewBuilder
    private func perms(_ pack: MeetVendorPack) -> some View {
        if let p = setup.profile {
            VStack(alignment: .leading, spacing: 6) {
                Text("It will be able to").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(MeetPalette.mut)
                ForEach(p.permissionDescription, id: \.self) { t in
                    HStack(alignment: .top, spacing: 8) {
                        Circle().fill(MeetPalette.ink).frame(width: 5, height: 5).padding(.top, 6)
                        Text(t).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub).fixedSize(horizontal: false, vertical: true)
                    }
                }
                if p.requiresUserOwnedSubscription {
                    Text("Uses your own \(pack.site) subscription").font(.system(size: 12, weight: .medium)).foregroundStyle(MeetPalette.ink)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 12))
            if let warning = p.termsWarning {
                Text(warning)
                    .font(.system(size: 12))
                    .foregroundStyle(MeetPalette.ink)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.statusWarning.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private var fields: some View {
        if let c = setup.connect, !c.extras.isEmpty {
            ForEach(Array(c.extras.enumerated()), id: \.offset) { _, f in
                let b = Binding(get: { c.fields[f.key] ?? "" }, set: { c.fields[f.key] = $0 })
                VStack(alignment: .leading, spacing: 4) {
                    Text(f.label).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(MeetPalette.ink)
                    Group {
                        if f.secret { SecureField("", text: b) } else { TextField("", text: b) }
                    }
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .font(.system(size: 14))
                    .padding(.horizontal, 12)
                    .frame(height: 44)
                    .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(MeetPalette.line))
                    .accessibilityLabel(f.label)
                    if f.secret {
                        Text("Allternit stores it encrypted.").font(.system(size: 11.5)).foregroundStyle(MeetPalette.mut)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func state(_ pack: MeetVendorPack) -> some View {
        let r = setup.connect?.result
        switch setup.phase {
        case .signing:
            spinRow("Opening sign-in on your Sessions computer…")
        case .checking:
            spinRow("Checking the connection…")
        case .consent:
            col("Before you connect", setup.connect?.consent ?? r?.message ?? "", steps: [])
        case .waiting:
            col("Waiting for \(setup.profile?.authType == "channel_oauth" ? "authorization" : "sign-in")",
                r.flatMap { $0.message.isEmpty ? nil : $0.message }
                    ?? "Sign in directly on \(pack.site) in the window that opened on your Sessions computer. Allternit never asks you to paste a password or session token.",
                steps: r?.steps ?? [])
        case .failed:
            col(r?.status == "blocked" ? "Blocked by a verification check" : r?.status == "needs_setup" ? "One step left" : "That didn’t finish",
                "\(setup.connect?.error ?? r?.message ?? "") Nothing was connected.", steps: r?.steps ?? [], bad: true)
        default:
            EmptyView()
        }
    }

    private func spinRow(_ text: String) -> some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub)
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.updatesFrequently)
    }

    private func col(_ title: String, _ body: String, steps: [String], bad: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 13, weight: .semibold)).foregroundStyle(bad ? Theme.statusError : MeetPalette.ink)
            Text(body).font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub).fixedSize(horizontal: false, vertical: true)
            ForEach(Array(steps.enumerated()), id: \.offset) { i, s in
                Text("\(i + 1). \(s)").font(.system(size: 12.5)).foregroundStyle(MeetPalette.sub).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 12))
        .accessibilityElement(children: .combine)
    }
}

/// A checkbox row (`bl-chk`).
private struct MeetCheckStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        Button { configuration.isOn.toggle() } label: {
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 5)
                    .fill(configuration.isOn ? MeetPalette.ink : Color.clear)
                    .overlay(RoundedRectangle(cornerRadius: 5).stroke(configuration.isOn ? MeetPalette.ink : MeetPalette.lineDash, lineWidth: 1.5))
                    .overlay(Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)).foregroundStyle(MeetPalette.bg).opacity(configuration.isOn ? 1 : 0))
                    .frame(width: 20, height: 20)
                configuration.label
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(configuration.isOn ? [.isSelected] : [])
    }
}
