import SwiftUI
import UIKit

// The real connect flows under the result card — the phone versions of the
// web's ServerPairPanel, SubscriptionConnect and ModesStep (phone mode).

// MARK: - Your own server (one-time pairing code)

/// "Your own server" on the phone: pair the machine like any computer you
/// own. A one-time code from the cloud, a command to run on the server, then
/// the account's paired computers are watched until the new one shows up.
/// The result card only says Paired once the cloud lists it.
struct SetupServerPairPanel: View {
    var onOutcome: (StepOutcome) -> Void

    private enum Phase: Equatable {
        case idle, creating
        case waiting(SetupPairingCode, Set<String>)
        case paired(name: String, os: String?)
        case expired
        case error(String)
    }

    @State private var phase: Phase = .idle
    @State private var copied = false
    @State private var poll: Task<Void, Never>?

    var body: some View {
        Group {
            switch phase {
            case .paired(let name, _):
                SetupNote(text: "Paired \(name). It shows up in Settings → Computers.")
            case .waiting(let code, _):
                waiting(code)
            default:
                VStack(alignment: .leading, spacing: 8) {
                    SetupNote(text: introText)
                    Button(buttonTitle) { start() }
                        .buttonStyle(SetupInlineButtonStyle())
                        .disabled(phase == .creating)
                }
            }
        }
        .onAppear { report() }
        .onChange(of: phase) { _, _ in report() }
        .onDisappear { poll?.cancel() }
    }

    private var introText: String {
        switch phase {
        case .expired: return "That code expired before the server paired. Make a new one and run it on the server."
        case .error(let message): return "Couldn’t make a pairing code: \(message)"
        default: return "Pair a machine you own (VPS, NAS or a spare computer). You get a one-time command to run on it."
        }
    }

    private var buttonTitle: String {
        switch phase {
        case .creating: return "Making a code …"
        case .idle: return "Get a pairing code"
        default: return "Get a new code"
        }
    }

    private func waiting(_ code: SetupPairingCode) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            SetupNote(text: "1. On your server, install the Allternit CLI.\n2. Run this command there:")
            HStack(spacing: 8) {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(code.command)
                        .font(SetupPalette.mono(12))
                        .foregroundStyle(SetupPalette.ink)
                        .textSelection(.enabled)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                }
                .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 8))
                Button(copied ? "Copied" : "Copy") {
                    UIPasteboard.general.string = code.command
                    copied = true
                }
                .buttonStyle(SetupInlineButtonStyle())
            }
            (Text("Code ") + Text(code.code).font(SetupPalette.mono(12.5))
                + Text(" works once, until \(code.expiresAt.formatted(date: .omitted, time: .shortened)). Waiting for your server to pair …"))
                .font(.system(size: 12.5))
                .foregroundStyle(SetupPalette.sub)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func start() {
        phase = .creating
        copied = false
        Task {
            do {
                // Snapshot what is already paired so only a new machine counts.
                let before = (try? await SetupAPI.pairedComputers()) ?? []
                let code = try await SetupAPI.createPairingCode()
                phase = .waiting(code, Set(before.map(\.id)))
                watch(code, known: Set(before.map(\.id)))
            } catch {
                phase = .error(error.localizedDescription)
            }
        }
    }

    /// Watch for the server to pair (every 4 s) until the code expires.
    private func watch(_ code: SetupPairingCode, known: Set<String>) {
        poll?.cancel()
        poll = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                if Task.isCancelled { return }
                if code.expiresAt < Date() { phase = .expired; return }
                if let fresh = (try? await SetupAPI.pairedComputers())?.first(where: { !known.contains($0.id) }) {
                    phase = .paired(name: fresh.name, os: fresh.os)
                    return
                }
            }
        }
    }

    private func report() {
        let label = "Your own server"
        switch phase {
        case .idle: onOutcome(StepOutcome(status: .idle, label: label, detail: ["Pair it with a one-time code"]))
        case .creating: onOutcome(StepOutcome(status: .working, label: label, detail: ["Making a pairing code …"]))
        case .waiting: onOutcome(StepOutcome(status: .waiting, label: label, detail: ["Run the pairing command on your server", "Waiting for it to pair …"]))
        case .paired(let name, let os):
            onOutcome(StepOutcome(status: .paired, label: name,
                                  detail: ["Paired \(name)"] + (os.map { ["OS: \($0)"] } ?? []) + ["Manage it in Settings → Computers"]))
        case .expired: onOutcome(StepOutcome(status: .error, label: label, detail: ["The pairing code expired before the server paired"]))
        case .error(let message): onOutcome(StepOutcome(status: .error, label: label, detail: [message]))
        }
    }
}

// MARK: - Subscription connect (ChatGPT / Claude / Kimi plans)

/// Connect a web subscription during setup (web SubscriptionConnect inside the
/// setup panel): the vendor's "connect an app" consent, then the sign-in on
/// the vendor's real site in a browser on the Sessions computer, shown live
/// here (LoginViewer). Allternit never renders a vendor login form.
struct SetupSubscriptionConnect: View {
    let brain: BrainChoice
    @ObservedObject var signals: SetupSignals
    var onReady: () -> Void
    var onCancel: () -> Void

    private enum Step: Equatable { case consent, signin(SetupFabricAccount), done(SetupFabricAccount) }

    @State private var step: Step = .consent
    @State private var agreed = false
    @State private var busy = false
    @State private var error: String?
    @State private var computer: ComputerRecord?
    @State private var attempt = 0

    private var provider: String { brain.subscriptionProvider ?? "" }
    private var name: String { brain.shortName }
    private var computerName: String { signals.subsStatus?.computerName ?? "your Sessions computer" }
    private var brand: Color { SetupSubscriptionBrand.color(provider) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            switch (signals.subscriptionState(provider), step) {
            case (_, .done(let account)): done(account)
            case (_, .signin(let account)): signingIn(account)
            case (.noComputer, _): noComputer
            case (.offline, _): offline
            default: consent
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 16))
        .overlay(alignment: .top) {
            UnevenRoundedRectangle(topLeadingRadius: 16, topTrailingRadius: 16).fill(brand).frame(height: 3)
        }
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(SetupPalette.line))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Connect \(name)")
        .task(id: signals.subsStatus?.computerId) { await loadComputer() }
    }

    private func header(_ title: String, _ subtitle: String) -> some View {
        VStack(spacing: 12) {
            HStack(spacing: 10) {
                AllternitOrbView(state: .idle, size: 26)
                    .frame(width: 44, height: 44)
                    .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.black.opacity(0.05)))
                Image(systemName: "arrow.left.arrow.right").font(.system(size: 14)).foregroundStyle(SetupPalette.mut)
                SetupSubscriptionLogo(provider: provider, size: 44)
            }
            .accessibilityHidden(true)
            VStack(spacing: 4) {
                Text(title).font(.system(size: 16, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                Text(subtitle).font(.system(size: 13)).foregroundStyle(SetupPalette.sub)
            }
            .multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity)
    }

    private var noComputer: some View {
        VStack(spacing: 16) {
            header("Connect \(name) to Allternit", "\(name) runs on a Sessions computer, one of your cloud or paired computers. Set one up first; it takes a minute.")
            SetupNote(text: "Set it up in Settings → Computers after setup.")
                .multilineTextAlignment(.center)
            HStack(spacing: 8) {
                Button("Not now", action: onCancel).buttonStyle(SetupGhostButtonStyle())
                Button("Check again") { Task { await signals.loadSubscriptions() } }
                    .buttonStyle(SetupBrandButtonStyle(color: brand))
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var offline: some View {
        VStack(spacing: 16) {
            header("Sessions computer is offline", "Start \(computerName) to open \(name)’s sign-in page.")
            if let error { SetupErrorLine(text: error) }
            HStack(spacing: 8) {
                Spacer()
                Button("Cancel", action: onCancel).buttonStyle(SetupGhostButtonStyle())
                Button("Start Sessions computer") { startComputer() }
                    .buttonStyle(SetupInlineButtonStyle())
                    .disabled(busy || computer == nil)
            }
        }
    }

    private var consent: some View {
        let terms = signals.terms(for: provider)
        let needsTerms = terms.map { !$0.acknowledged } ?? false
        return VStack(alignment: .leading, spacing: 16) {
            header("Connect \(name) to Allternit", "Use your \(name) subscription as a model in Allternit.")
            VStack(alignment: .leading, spacing: 10) {
                permission("desktopcomputer", Text("\(name) runs in its own browser on ") + Text(computerName).foregroundColor(SetupPalette.ink).fontWeight(.medium) + Text(", never on this phone."))
                permission("cursorarrow.click", Text("A \(name) task starts only when you press send or confirm it, and any question \(name) asks comes to you."))
                permission("lock", Text("You sign in on \(name)’s own site. Your login stays on that computer; Allternit never sees your password."))
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 12))

            if needsTerms, let terms {
                VStack(alignment: .leading, spacing: 8) {
                    DisclosureGroup {
                        Text(terms.text).font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
                            .lineSpacing(2)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.top, 8)
                    } label: {
                        Text("About \(name)’s terms").font(.system(size: 12, weight: .medium)).foregroundStyle(SetupPalette.ink)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(SetupPalette.line))
                    Button { agreed.toggle() } label: {
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: agreed ? "checkmark.square.fill" : "square")
                                .font(.system(size: 16)).foregroundStyle(agreed ? SetupPalette.ink : SetupPalette.mut)
                            Text("I’ve read \(name)’s terms notice and want to connect my account")
                                .font(.system(size: 13)).foregroundStyle(SetupPalette.ink)
                                .multilineTextAlignment(.leading)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(agreed ? .isSelected : [])
                }
            }
            if let error {
                SetupErrorLine(text: error) {
                    Button("Retry") { Task { await signals.loadSubscriptions() } }.buttonStyle(SetupGhostButtonStyle())
                }
            }
            HStack(spacing: 8) {
                if busy { SetupWorking(label: "Opening the sign-in…") }
                Spacer(minLength: 0)
                Button("Cancel") { cancel() }.buttonStyle(SetupGhostButtonStyle())
                Button("Continue to \(name)") { start(needsTerms: needsTerms) }
                    .buttonStyle(SetupBrandButtonStyle(color: brand))
                    .disabled(busy || (needsTerms && !agreed))
            }
        }
    }

    @ViewBuilder
    private func signingIn(_ account: SetupFabricAccount) -> some View {
        HStack(spacing: 12) {
            SetupSubscriptionLogo(provider: provider, size: 32)
            VStack(alignment: .leading, spacing: 1) {
                Text("Sign in to \(name)").font(.system(size: 14, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                Text("On \(name)’s own site, in a browser on \(computerName).").font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
            }
        }
        if let computer {
            SetupLoginViewer(computer: computer, account: account, providerName: name,
                             onChecked: { next in
                                 signals.upsert(next)
                                 if next.sessionHealth == "ready" { step = .done(next) }
                             },
                             onClose: { cancel() })
        } else {
            SetupErrorLine(text: "The \(name) sign-in opened on \(computerName), but that computer can’t be shown here. Open it from Settings → Computers to finish.")
        }
    }

    private func done(_ account: SetupFabricAccount) -> some View {
        let who = account.identity ?? account.label
        return VStack(spacing: 16) {
            ZStack(alignment: .bottomTrailing) {
                SetupSubscriptionLogo(provider: provider, size: 52)
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 22))
                    .foregroundStyle(SetupPalette.ok)
                    .background(Circle().fill(Color.white))
                    .offset(x: 6, y: 6)
            }
            VStack(spacing: 4) {
                Text("\(name) is connected").font(.system(size: 16, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                Text("\(who.isEmpty ? "" : "Signed in as \(who). ")Pick \(name) in any chat’s model menu.")
                    .font(.system(size: 13)).foregroundStyle(SetupPalette.sub)
            }
            .multilineTextAlignment(.center)
            Button("Use it as my default brain") {
                Task { await signals.loadSubscriptions() }
                if account.isReady { onReady() }
            }
            .buttonStyle(SetupBrandButtonStyle(color: brand))
        }
        .frame(maxWidth: .infinity)
    }

    private func permission(_ icon: String, _ text: Text) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).font(.system(size: 14)).foregroundStyle(SetupPalette.ink).frame(width: 18).padding(.top, 1)
            text.font(.system(size: 13)).foregroundStyle(SetupPalette.sub).lineSpacing(1).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func loadComputer() async {
        guard let id = signals.subsStatus?.computerId else { computer = nil; return }
        computer = (try? await BotModeClient.shared.listComputers())?.first { $0.id == id }
    }

    /// Cancel always works, even while a request to a stuck computer is in flight.
    private func cancel() {
        attempt += 1
        busy = false
        onCancel()
    }

    private func startComputer() {
        guard let computer else { return }
        busy = true
        Task {
            do {
                try await BotModeClient.shared.startComputer(id: computer.id)
                error = nil
                await signals.loadSubscriptions()
                await loadComputer()
            } catch {
                self.error = SetupSignals.describeSubsError(error)
            }
            busy = false
        }
    }

    private func start(needsTerms: Bool) {
        attempt += 1
        let mine = attempt
        busy = true
        error = nil
        Task {
            do {
                if needsTerms, let version = signals.subsStatus?.disclosureVersion {
                    try await SetupAPI.acknowledgeDisclosure(provider: provider, version: version)
                }
                // Sign an existing login back in, or add a new one.
                let existing = signals.subsAccounts.first { $0.provider == provider && $0.sessionHealth == "auth_required" }
                let account: SetupFabricAccount
                if let existing {
                    let id = existing.accountId
                    try await setupWithTimeout(45) { try await SetupAPI.openFabricLogin(id) }
                    account = existing
                } else {
                    let id = provider
                    account = try await setupWithTimeout(45) { try await SetupAPI.createFabricAccount(provider: id) }
                }
                guard mine == attempt else { return }
                signals.upsert(account)
                if computer == nil { await loadComputer() }
                step = .signin(account)
                if needsTerms { await signals.loadSubscriptions() }
            } catch {
                if mine == attempt { self.error = SetupSignals.describeSubsError(error) }
            }
            if mine == attempt { busy = false }
        }
    }
}

/// The login window on the Sessions computer, shown live (web LoginViewer).
/// The person started it, so it opens with them in control when nobody else
/// holds the computer. Newer gateways notice the sign-in by themselves
/// (polled every 2 s); older ones need "I’ve signed in".
struct SetupLoginViewer: View {
    let computer: ComputerRecord
    let account: SetupFabricAccount
    let providerName: String
    var onChecked: (SetupFabricAccount) -> Void
    var onClose: () -> Void

    @State private var lease: ComputerControlLease?
    @State private var wsPath: String?
    @State private var autoDetect: Bool?
    @State private var waitingOn: String?
    @State private var checking = false
    @State private var error: String?
    @State private var finished = false
    @State private var fullScreen = false

    private var mine: Bool {
        guard let lease else { return false }
        return lease.holder.kind == "user" && lease.holder.deviceId == BotModeClient.deviceId
    }

    private var controlSummary: String {
        guard let lease else { return "Nobody is controlling it" }
        return mine ? "You’re in control" : "\(lease.holder.displayLabel) is in control"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Sign in to \(providerName) on \(computer.name)")
                        .font(.system(size: 13, weight: .medium)).foregroundStyle(SetupPalette.ink)
                    Text("\(autoDetect == false ? "Sign in below, then choose “I’ve signed in”." : "Sign in below. Allternit notices when you’re done.") You type your password on that computer; Allternit doesn’t see or store it. \(controlSummary).")
                        .font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
                Button("Cancel") { close() }.buttonStyle(SetupControlButtonStyle())
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    if mine {
                        Button("Release control") { release() }.buttonStyle(SetupControlButtonStyle())
                    } else {
                        Button("Take control") { take() }.buttonStyle(SetupControlButtonStyle())
                    }
                    Button { fullScreen = true } label: {
                        Label("Open larger", systemImage: "desktopcomputer")
                    }
                    .buttonStyle(SetupControlButtonStyle())
                    .disabled(wsPath == nil)
                    if autoDetect == false {
                        Button("I’ve signed in") { check() }.buttonStyle(SetupControlButtonStyle()).disabled(checking)
                    }
                    if autoDetect != false && error == nil {
                        SetupWorking(label: waitingOn.map { "\($0)…" } ?? "Waiting for you to sign in…")
                    }
                    if checking { SetupWorking(label: "Checking the session…") }
                }
            }
            if let error { SetupErrorLine(text: error) }
            ZStack {
                Color.black
                if let wsPath {
                    ComputerScreenCanvas(wsPath: wsPath, controls: mine).id(wsPath)
                } else {
                    ProgressView().tint(.white)
                }
            }
            .aspectRatio(16 / 9, contentMode: .fit)
            .frame(minHeight: 200)
            .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .padding(12)
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Log in to \(providerName)")
        .fullScreenCover(isPresented: $fullScreen) {
            if let wsPath {
                NavigationStack {
                    ComputerScreenCanvas(wsPath: wsPath, controls: mine)
                        .ignoresSafeArea(edges: .bottom)
                        .navigationTitle(computer.name)
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar { ToolbarItem(placement: .topBarLeading) { Button("Close") { fullScreen = false } } }
                }
            }
        }
        .task { await openInControl() }
        .task(id: account.accountId) { await watchLogin() }
    }

    /// Open already in control: the person asked for this login.
    private func openInControl() async {
        lease = try? await BotModeClient.shared.controlState(computerId: computer.id).control
        if lease == nil {
            try? await BotModeClient.shared.takeControl(computerId: computer.id)
            lease = try? await BotModeClient.shared.controlState(computerId: computer.id).control
        }
        await connect()
    }

    private func connect() async {
        do {
            let status = try await BotModeClient.shared.viewStatus(computerId: computer.id)
            let token: String
            do {
                token = try await BotModeClient.shared.vncToken(computerId: computer.id, readOnly: !mine)
            } catch where mine {
                token = try await BotModeClient.shared.vncToken(computerId: computer.id, readOnly: true)
            }
            let separator = status.ws.vnc.contains("?") ? "&" : "?"
            wsPath = "\(status.ws.vnc)\(separator)token=\(BotModeClient.query(token))"
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func take() {
        Task {
            do {
                try await BotModeClient.shared.takeControl(computerId: computer.id)
                lease = try? await BotModeClient.shared.controlState(computerId: computer.id).control
                await connect()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private func release() {
        Task {
            try? await BotModeClient.shared.releaseControl(computerId: computer.id)
            lease = try? await BotModeClient.shared.controlState(computerId: computer.id).control
            await connect()
        }
    }

    private func finish(_ next: SetupFabricAccount) {
        guard !finished else { return }
        finished = true
        if mine { Task { try? await BotModeClient.shared.releaseControl(computerId: computer.id) } }
        onChecked(next)
    }

    /// signed_in / failed / closed end it. The account itself is also read every
    /// third tick: after a bot check the login tracker can stay "waiting" although
    /// the account is already ready.
    private func watchLogin() async {
        var ticks = 0
        while !Task.isCancelled && !finished {
            ticks += 1
            if ticks % 3 == 0,
               let row = (try? await SetupAPI.fabricAccounts())?.first(where: { $0.accountId == account.accountId }),
               row.sessionHealth == "ready" {
                finish(row)
                return
            }
            if autoDetect != false {
                do {
                    let st = try await SetupAPI.fabricLoginState(account.accountId)
                    autoDetect = true
                    waitingOn = st.state == .waiting ? st.detail : nil
                    switch st.state {
                    case .signedIn: if let a = st.account { finish(a); return }
                    case .failed: error = st.detail ?? "Signing in to \(providerName) didn’t finish."
                    case .closed: onClose(); return
                    default: break
                    }
                } catch let e as ConnectError where e.status == 404 || e.status == 405 || e.code == "not_found" {
                    // Older gateway: no login-state route. Fall back to the manual check.
                    autoDetect = false
                } catch {}
            }
            try? await Task.sleep(nanoseconds: 2_000_000_000)
        }
    }

    private func check() {
        checking = true
        error = nil
        Task {
            do {
                if let next = try await SetupAPI.checkFabricAccount(account.accountId) {
                    if next.sessionHealth == "ready" { finish(next) } else { onChecked(next) }
                }
            } catch {
                self.error = SetupSignals.describeSubsError(error)
            }
            checking = false
        }
    }

    private func close() {
        let id = account.accountId
        Task { try? await SetupAPI.cancelFabricLogin(id) }
        if mine { Task { try? await BotModeClient.shared.releaseControl(computerId: computer.id) } }
        onClose()
    }
}

/// Vendor brand colours (web provider-registry SUBSCRIPTION_BRANDS).
enum SetupSubscriptionBrand {
    static func color(_ provider: String) -> Color {
        switch provider {
        case "chatgpt": return Color(hex: "#10A37F")
        case "claude": return Color(hex: "#D97757")
        case "kimi": return Color(hex: "#1783FF")
        case "gemini", "google": return Color(hex: "#1B72E8")
        case "copilot", "microsoft": return Color(hex: "#0F6CBD")
        default: return SetupPalette.ink
        }
    }

    static let logo: [String: String] = ["chatgpt": "SetupLogoChatGPT", "claude": "SetupLogoClaude", "kimi": "SetupLogoKimi"]
}

/// The vendor's own logo on a white tile (web SubscriptionLogo).
struct SetupSubscriptionLogo: View {
    let provider: String
    var size: CGFloat = 36

    var body: some View {
        let radius = max(6, (size / 4).rounded())
        ZStack {
            Color.white
            if let asset = SetupSubscriptionBrand.logo[provider] {
                Image(asset).resizable().scaledToFit().frame(width: size * 0.62, height: size * 0.62)
            } else {
                Text(String((provider.first ?? "?")).uppercased())
                    .font(.system(size: size * 0.38, weight: .bold))
                    .foregroundStyle(SetupSubscriptionBrand.color(provider))
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: radius))
        .overlay(RoundedRectangle(cornerRadius: radius).stroke(Color.black.opacity(0.05)))
        .accessibilityHidden(true)
    }
}

/// Orb + label status line (web `Working`).
struct SetupWorking: View {
    var label: String
    var body: some View {
        HStack(spacing: 6) {
            AllternitOrbView(state: .working, size: 14)
            Text(label).font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
        }
        .accessibilityElement(children: .combine)
    }
}

/// Error line on the neutral fill (web `ErrorLine`).
struct SetupErrorLine<Trailing: View>: View {
    var text: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 13))
            Text(text).font(.system(size: 12)).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            trailing
        }
        .foregroundStyle(SetupPalette.errFg)
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 8))
        .accessibilityAddTraits(.isStaticText)
    }
}

extension SetupErrorLine where Trailing == EmptyView {
    init(text: String) { self.init(text: text) { EmptyView() } }
}

/// Small outlined control button (web CONTROL_BUTTON).
struct SetupControlButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12, weight: .medium))
            .labelStyle(.titleAndIcon)
            .foregroundStyle(SetupPalette.ink)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(configuration.isPressed ? SetupPalette.fill : SetupPalette.card, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(SetupPalette.line))
            .contentShape(Rectangle().inset(by: -7))
            .opacity(enabled ? 1 : 0.5)
    }
}

/// The vendor-coloured primary button ("Continue to ChatGPT").
struct SetupBrandButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    var color: Color
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(.white)
            .padding(.horizontal, 14)
            .frame(height: 36)
            .background(color, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle().inset(by: -4))
            .opacity(enabled ? (configuration.isPressed ? 0.9 : 1) : 0.5)
    }
}

// MARK: - Brain connect (phone ModesStep)

/// The phone's provider connect for one brain row: a provider the paired
/// runtime already authenticates connects with one tap; otherwise an API key
/// is validated and stored by the runtime. CLI sign-ins and local model
/// downloads run on the computer, so the phone points to Desktop for them
/// (web ModesStep `phone`).
struct SetupBrainConnect: View {
    let brain: BrainChoice
    @ObservedObject var signals: SetupSignals
    /// Reports the new default (provider, model).
    var onDefault: (_ provider: String, _ model: String?) -> Void

    private struct Cloud { let id: String; let name: String; let placeholder: String; let hint: String }
    private static let clouds: [BrainChoice: Cloud] = [
        .claude: Cloud(id: "anthropic", name: "Anthropic", placeholder: "sk-ant-…", hint: "Get your key at console.anthropic.com"),
        .chatgpt: Cloud(id: "openai", name: "OpenAI", placeholder: "sk-…", hint: "Get your key at platform.openai.com"),
        .gemini: Cloud(id: "google", name: "Google AI", placeholder: "AIza…", hint: "Get your key at aistudio.google.com"),
    ]
    /// CLI providers that already count as this brain when the runtime has them.
    private static let cliIds: [BrainChoice: String] = [.claude: "claude-cli", .chatgpt: "codex-cli", .gemini: "antigravity"]

    @State private var key = ""
    @State private var reveal = false
    @State private var status: Status = .idle
    private enum Status: Equatable { case idle, checking, valid(String), invalid(String) }

    var body: some View {
        if let cloud = Self.clouds[brain] {
            cloudPanel(cloud)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                SetupNote(text: brain == .local
                          ? "Local models download and run on your computer. Open Allternit Desktop there to pick one, or finish later in Settings → Models."
                          : "Qwen, Kimi, Z.ai and command-line brains sign in on your computer. Connect them from Allternit Desktop, or later in Settings → Models.")
            }
        }
    }

    @ViewBuilder
    private func cloudPanel(_ cloud: Cloud) -> some View {
        let already = signals.authed.contains(cloud.id) || Self.cliIds[brain].map(signals.authed.contains) == true
        VStack(alignment: .leading, spacing: 10) {
            if already {
                SetupNote(text: "\(cloud.name) is already signed in on your computer.")
                Button(status == .checking ? "Connecting …" : "Use \(brain.label)") { useConfigured(cloud) }
                    .buttonStyle(SetupInlineButtonStyle())
                    .disabled(status == .checking)
            } else {
                Text("\(cloud.name) API key").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                HStack(spacing: 8) {
                    Group {
                        if reveal { TextField(cloud.placeholder, text: $key) } else { SecureField(cloud.placeholder, text: $key) }
                    }
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(SetupPalette.mono(13))
                    .padding(.horizontal, 10)
                    .frame(height: 38)
                    .background(SetupPalette.bg, in: RoundedRectangle(cornerRadius: 9))
                    .overlay(RoundedRectangle(cornerRadius: 9).stroke(SetupPalette.line))
                    .submitLabel(.done)
                    .onSubmit { validate(cloud) }
                    Button { reveal.toggle() } label: {
                        Image(systemName: reveal ? "eye.slash" : "eye").foregroundStyle(SetupPalette.sub).frame(width: 36, height: 38)
                    }
                    .accessibilityLabel(reveal ? "Hide key" : "Show key")
                }
                Text(cloud.hint).font(.system(size: 11.5)).foregroundStyle(SetupPalette.mut)
                Button(status == .checking ? "Checking …" : "Connect") { validate(cloud) }
                    .buttonStyle(SetupInlineButtonStyle())
                    .disabled(status == .checking || key.trimmingCharacters(in: .whitespaces).count < 10)
            }
            switch status {
            case .valid(let message): Text(message).font(.system(size: 12.5)).foregroundStyle(SetupPalette.okFg)
            case .invalid(let message): Text(message).font(.system(size: 12.5)).foregroundStyle(SetupPalette.errFg)
            default: EmptyView()
            }
        }
    }

    /// One-tap connect for a provider the runtime already authenticates.
    private func useConfigured(_ cloud: Cloud) {
        status = .checking
        Task {
            let models = (try? await SetupAPI.discoverProviderModels(cloud.id)) ?? []
            try? await SetupAPI.saveProvider(provider: cloud.id, apiKey: nil, models: models)
            onDefault(cloud.id, models.first?.id)
            status = .valid("Connected \(cloud.name)")
        }
    }

    private func validate(_ cloud: Cloud) {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 10 else { return }
        status = .checking
        Task {
            do {
                let result = try await SetupAPI.validateKey(provider: cloud.id, key: trimmed)
                guard result.valid else { status = .invalid(result.error ?? "Key rejected"); return }
                // The plaintext key goes straight to the runtime's keychain and is dropped here.
                try? await SetupAPI.saveProvider(provider: cloud.id, apiKey: trimmed, models: result.models)
                key = ""
                onDefault(cloud.id, result.models.first?.id)
                status = .valid("Connected \(cloud.name)")
            } catch {
                status = .invalid("Could not reach your computer to check the key")
            }
        }
    }
}
