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

/// Connect a web subscription, shaped like the web SubscriptionConnect:
/// what Allternit will do with the account, the provider-terms notice (D16:
/// only the person acknowledges), then "Continue to <vendor>". The sign-in
/// itself opens on the vendor's real site in a browser on the user's
/// Sessions computer; the phone watches the login until it is ready.
struct SetupSubscriptionConnect: View {
    let brain: BrainChoice
    @ObservedObject var signals: SetupSignals
    var onReady: () -> Void
    var onCancel: () -> Void

    @State private var agreed = false
    @State private var busy = false
    @State private var error: String?
    @State private var watching: SetupFabricAccount?
    @State private var poll: Task<Void, Never>?

    private var provider: String { brain.subscriptionProvider ?? "" }
    private var name: String { brain.shortName }
    private var computerName: String { signals.subsStatus?.computerName ?? "your Sessions computer" }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            switch signals.subscriptionState(provider) {
            case .noComputer:
                SetupNote(text: "\(name) runs on a Sessions computer, one of your cloud or paired computers. Set one up first in Settings → Computers; it takes a minute.")
                cancelRow
            case .offline:
                SetupNote(text: "Your Sessions computer is offline. Start \(computerName) to open \(name)’s sign-in page.")
                HStack {
                    Button("Check again") { Task { await signals.loadSubscriptions() } }.buttonStyle(SetupInlineButtonStyle())
                    Spacer()
                    Button("Cancel", action: onCancel).buttonStyle(SetupGhostButtonStyle())
                }
            case .error:
                SetupNote(text: signals.subsError ?? "Could not reach your Sessions computer")
                HStack {
                    Button("Retry") { Task { await signals.loadSubscriptions() } }.buttonStyle(SetupInlineButtonStyle())
                    Spacer()
                    Button("Cancel", action: onCancel).buttonStyle(SetupGhostButtonStyle())
                }
            default:
                if let watching {
                    signingIn(watching)
                } else {
                    consent
                }
            }
        }
        .onDisappear { poll?.cancel() }
    }

    private var cancelRow: some View {
        HStack { Spacer(); Button("Not now", action: onCancel).buttonStyle(SetupGhostButtonStyle()) }
    }

    private var consent: some View {
        let terms = signals.terms(for: provider)
        let needsTerms = terms.map { !$0.acknowledged } ?? false
        return VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 8) {
                permission("desktopcomputer", "\(name) runs in its own browser on \(computerName), never on this phone.")
                permission("cursorarrow.click", "A \(name) task starts only when you press send or confirm it, and any question \(name) asks comes to you.")
                permission("lock", "You sign in on \(name)’s own site. Your login stays on that computer; Allternit never sees your password.")
            }
            .padding(12)
            .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 10))

            if needsTerms, let terms {
                DisclosureGroup {
                    Text(terms.text).font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
                        .fixedSize(horizontal: false, vertical: true)
                } label: {
                    Text("About \(name)’s terms").font(.system(size: 12.5, weight: .medium)).foregroundStyle(SetupPalette.ink)
                }
                Toggle(isOn: $agreed) {
                    Text("I’ve read \(name)’s terms notice and want to connect my account")
                        .font(.system(size: 13)).foregroundStyle(SetupPalette.ink)
                }
                .toggleStyle(.switch)
                .tint(SetupPalette.ink)
            }
            if let error { Text(error).font(.system(size: 12.5)).foregroundStyle(SetupPalette.errFg) }
            HStack(spacing: 8) {
                if busy { ProgressView().controlSize(.small) }
                Spacer()
                Button("Cancel") { poll?.cancel(); busy = false; onCancel() }.buttonStyle(SetupGhostButtonStyle())
                Button("Continue to \(name)") { start(needsTerms: needsTerms) }
                    .buttonStyle(SetupInlineButtonStyle())
                    .disabled(busy || (needsTerms && !agreed))
            }
        }
    }

    private func signingIn(_ account: SetupFabricAccount) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                AllternitOrbView(state: .searching, size: 18)
                Text("Sign in to \(name)").font(.system(size: 14, weight: .semibold)).foregroundStyle(SetupPalette.ink)
            }
            SetupNote(text: "The sign-in opened on \(name)’s own site, in a browser on \(computerName). Finish signing in there (Settings → Computers shows that screen). This updates by itself.")
            if let error { Text(error).font(.system(size: 12.5)).foregroundStyle(SetupPalette.errFg) }
            HStack {
                Button("Open the sign-in again") {
                    Task { try? await SetupAPI.openFabricLogin(account.accountId) }
                }
                .buttonStyle(SetupGhostButtonStyle())
                Spacer()
                Button("Cancel") { poll?.cancel(); onCancel() }.buttonStyle(SetupGhostButtonStyle())
            }
        }
    }

    private func permission(_ icon: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).font(.system(size: 13)).foregroundStyle(SetupPalette.ink).frame(width: 18)
            Text(text).font(.system(size: 13)).foregroundStyle(SetupPalette.sub).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func start(needsTerms: Bool) {
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
                signals.upsert(account)
                watching = account
                watch(account)
                if needsTerms { await signals.loadSubscriptions() }
            } catch {
                self.error = SetupSignals.describeSubsError(error)
            }
            busy = false
        }
    }

    /// Re-check the login every 3 s until it can serve chats.
    private func watch(_ account: SetupFabricAccount) {
        poll?.cancel()
        poll = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                if Task.isCancelled { return }
                if let next = try? await SetupAPI.checkFabricAccount(account.accountId) {
                    signals.upsert(next)
                    if next.isReady { onReady(); return }
                }
            }
        }
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
