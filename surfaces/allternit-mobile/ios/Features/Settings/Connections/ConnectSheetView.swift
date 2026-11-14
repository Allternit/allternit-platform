import SwiftUI
import UIKit

/// What the connect sheet is for.
enum ConnSheet: Identifiable, Equatable {
    /// Paste a key. `only` = one provider (Gemini, or a "Use a key" fallback).
    case keys(only: String?)
    /// A sign-in that runs on a computer (ChatGPT, Codex, Claude Code…).
    case login(String)
    /// Something that is set up on the computer itself (local brain).
    case onComputer(String)
    /// Sign in with ChatGPT on this iPhone (contract §9.9).
    case chatgptDevice

    var id: String {
        switch self {
        case .keys(let only): return "keys-\(only ?? "all")"
        case .login(let id): return "login-\(id)"
        case .onComputer(let id): return "computer-\(id)"
        case .chatgptDevice: return "chatgpt-device"
        }
    }
}

/// The steps a connection takes, shown up front (web `StepStrip`).
struct ConnStepStrip: View {
    let steps: [String]
    let at: Int

    var body: some View {
        HStack(spacing: 4) {
            ForEach(Array(steps.enumerated()), id: \.offset) { i, label in
                HStack(spacing: 4) {
                    Text(i < at ? "✓" : "\(i + 1)").fontWeight(.semibold)
                    Text(label).lineLimit(1)
                }
                .font(.system(size: 11.5))
                .padding(.horizontal, 8)
                .frame(height: 24)
                .foregroundColor(i < at ? Color(hex: "#2B7A40") : i == at ? Color("BgPrimary") : Color("TextTertiary"))
                .background(Capsule().fill(i < at ? Color(hex: "#3FAA5A").opacity(0.14) : i == at ? Color("TextPrimary") : Color("TextPrimary").opacity(0.07)))
                if i < steps.count - 1 {
                    Text("›").font(.system(size: 12)).foregroundColor(Color("TextTertiary")).accessibilityHidden(true)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Step \(min(at + 1, steps.count)) of \(steps.count): \(steps[min(at, steps.count - 1)])")
    }
}

/// The connect sheet (web `ConnectSheet`): Allternit ⇄ vendor, the steps,
/// then the vendor step and a live check, ending on "Connected · …".
struct ConnectSheetView: View {
    let sheet: ConnSheet
    @ObservedObject var store: ConnectionsStore
    /// Called with the id that turned on (setup uses it to pick the brain).
    var onConnected: ((String) -> Void)? = nil
    var onFallback: ((String) -> Void)? = nil

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    content
                }
                .padding(EdgeInsets(top: 8, leading: 16, bottom: 24, trailing: 16))
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
        // The ChatGPT sign-in page needs the full height.
        .presentationDetents(sheet == .chatgptDevice ? [.large] : [.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private var title: String {
        switch sheet {
        case .keys(let only):
            if let only, let p = ConnectionsCatalog.keyProvider(only) { return "Connect \(p.name)" }
            return "Connect a key"
        case .login(let id), .onComputer(let id):
            let name = ConnectionsCatalog.entry(id)?.name ?? id
            return store.state(id) == .needs ? "Reconnect \(name)" : "Connect \(name)"
        case .chatgptDevice:
            return store.state("chatgpt") == .needs ? "Reconnect ChatGPT" : "Connect ChatGPT"
        }
    }

    @ViewBuilder
    private var content: some View {
        switch sheet {
        case .keys(let only):
            let providers = only.flatMap(ConnectionsCatalog.keyProvider).map { [$0] } ?? ConnectionsCatalog.keyProviders
            Text("Open the key page, copy a key, then come back and press Paste. We check it with the vendor before it's saved, and it works on any device.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            ConnStepStrip(steps: ["Open the key page", "Paste", "Done"], at: 0)
            VStack(spacing: 10) {
                ForEach(providers) { provider in
                    ConnKeyCard(provider: provider, store: store, startOpen: providers.count == 1) { id in
                        onConnected?(id)
                    }
                }
            }
        case .login(let id):
            ConnLoginStep(id: id, store: store, onConnected: { onConnected?(id) }, onFallback: { f in
                dismiss()
                onFallback?(f)
            })
        case .chatgptDevice:
            ChatGPTSignInStep(store: store, onConnected: { onConnected?("chatgpt") }, onFallback: { f in
                dismiss()
                onFallback?(f)
            })
        case .onComputer(let id):
            let name = ConnectionsCatalog.entry(id)?.name ?? id
            Text("\(name) runs on your own computer. Open Allternit on your Mac or PC and set it up in Settings › Connections there. It shows here as soon as it's connected.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if !store.computerOnline {
                Button("Add a computer") {
                    dismiss()
                    onFallback?("this-mac")
                }
                .buttonStyle(ConnPrimaryButtonStyle())
            }
        }
    }
}

/// One key provider: Open the key page › Paste › Done (contract §6 "Keys").
private struct ConnKeyCard: View {
    let provider: ConnKeyProvider
    @ObservedObject var store: ConnectionsStore
    var startOpen = false
    let onConnected: (String) -> Void

    @State private var safariURL: IdentifiableURL?
    @State private var expanded = false
    @State private var typing = false
    @State private var draft = ""
    @State private var checking = false
    @State private var error: String?

    var body: some View {
        let row = store.row(provider.id)
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                ConnLogoView(logo: provider.logo, name: provider.name, size: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(provider.name).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    Text(statusLine(row))
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if !expanded && !checking {
                    Button(row.state == .on ? "Replace" : row.state == .needs ? "Fix" : "Open the key page") {
                        error = nil
                        safariURL = URL(string: provider.keyPage).map { IdentifiableURL(url: $0) }
                    }
                    .buttonStyle(row.state == .needs ? AnyButtonStyle(ConnPrimaryButtonStyle()) : AnyButtonStyle(ConnQuietButtonStyle()))
                }
            }
            if expanded || checking {
                pasteArea
            }
            if let error {
                SettingsErrorBanner(message: error)
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityIdentifier("conn-key-\(provider.id)")
        .onAppear { if startOpen && row.state != .on { expanded = true } }
        .sheet(item: $safariURL, onDismiss: { expanded = true }) { item in
            SafariView(url: item.url).ignoresSafeArea()
        }
    }

    private func statusLine(_ row: ConnectionWire) -> String {
        switch row.state {
        case .on: return row.found.map { "Connected · \($0)" } ?? "Connected"
        case .needs: return "Needs you · \(row.reason ?? ConnectionsCatalog.reasonText(row.reasonCode, name: provider.name) ?? "The key was rejected")"
        case .connecting: return "Connecting…"
        case .off: return "Not connected"
        }
    }

    @ViewBuilder
    private var pasteArea: some View {
        if checking {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Checking your key with \(provider.name)…").font(.system(size: 13.5)).foregroundColor(Color("TextSecondary"))
            }
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    // System paste button: no "Allow Paste" prompt.
                    PasteButton(payloadType: String.self) { strings in
                        let pasted = strings.first ?? ""
                        Task { @MainActor in await submit(pasted) }
                    }
                    .buttonBorderShape(.capsule)
                    .labelStyle(.titleAndIcon)
                    .tint(Color("TextPrimary"))
                    if !typing {
                        Button("Type it instead") { typing = true }.buttonStyle(ConnQuietButtonStyle())
                    }
                    Spacer(minLength: 0)
                    Button("Open the key page") {
                        safariURL = URL(string: provider.keyPage).map { IdentifiableURL(url: $0) }
                    }
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                }
                if typing {
                    HStack(spacing: 8) {
                        SecureField("Your \(provider.name) key", text: $draft)
                            .font(.system(size: 14))
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                            .padding(.horizontal, 12)
                            .frame(minHeight: 36)
                            .background(Color("BgPrimary"))
                            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .onSubmit { Task { await submit(draft) } }
                        Button("Save") { Task { await submit(draft) } }
                            .buttonStyle(ConnPrimaryButtonStyle())
                            .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
            }
        }
    }

    private func submit(_ raw: String) async {
        let key = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            error = "The clipboard has no text. Copy your \(provider.name) key and try again."
            return
        }
        error = nil
        checking = true
        store.setConnecting(provider.id, true)
        defer {
            checking = false
            store.setConnecting(provider.id, false)
        }
        do {
            let row = try await ConnectionsClient.submit(provider.id, target: .cloud, value: key)
            store.apply(row)
            if row.state == .on {
                expanded = false
                typing = false
                draft = ""
                onConnected(provider.id)
            } else if row.state == .needs {
                error = "\(provider.name) said no: \((row.reason ?? ConnectionsCatalog.reasonText(row.reasonCode, name: provider.name) ?? "the key was rejected").lowercased()). Check the key and paste it again."
            }
        } catch {
            self.error = ConnectionsClient.describe(error)
        }
    }
}

/// Type-erased button style so one button can switch styles.
struct AnyButtonStyle: ButtonStyle {
    private let make: (Configuration) -> AnyView
    init<S: ButtonStyle>(_ style: S) { make = { AnyView(style.makeBody(configuration: $0)) } }
    func makeBody(configuration: Configuration) -> some View { make(configuration) }
}

/// A sign-in on the computer or on an Allternit runner (plan rows, contract
/// §9): a device code (Copy + Open) or a link and a paste-back code (Open +
/// Paste). A runner row first shows the server's disclosure. Polls `/login`
/// every 2 s and stops on done, expired or failed. No "I entered it" button
/// anywhere.
private struct ConnLoginStep: View {
    let id: String
    @ObservedObject var store: ConnectionsStore
    let onConnected: () -> Void
    let onFallback: (String) -> Void

    private enum Phase: Equatable { case starting, disclosure(ConnDisclosure), vendor, done, failed(String), expired }

    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var phase: Phase = .starting
    /// The disclosure version the person accepted in this sheet.
    @State private var accepted: String?
    /// The clipboard's change count when the vendor page was opened: a
    /// newer one on return means the person copied the code.
    @State private var pasteboardAtOpen: Int?
    @State private var login: ConnLogin?
    @State private var opened = false
    @State private var copied = false
    @State private var submitting = false
    @State private var now = Date()
    @State private var attempt = 0

    private var entry: ConnEntry? { ConnectionsCatalog.entry(id) }
    /// Plan rows sign in on an Allternit runner, not on a computer.
    private var onRunner: Bool { ConnectionsCatalog.runnerIds.contains(id) || store.row(id).runsOn == "runner" }
    private var vendor: String {
        switch id {
        case "chatgpt", "codex": return "ChatGPT"
        case "claude-code": return "Claude"
        case "grok-cli": return "Grok"
        case "kimi-cli": return "Kimi"
        case "gemini-cli": return "Google"
        case "cursor": return "Cursor"
        default: return entry?.name ?? id
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if store.state(id) == .needs, let reason = store.row(id).reason ?? ConnectionsCatalog.reasonText(store.row(id).reasonCode, name: vendor) {
                Text("\(reason). Sign in again and everything that used it picks up where it left off.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 10).fill(Color(hex: "#E8A33A").opacity(0.12)))
            }
            if case .disclosure = phase {} else {
                Text(onRunner
                     ? "Your \(vendor) plan signs in on Allternit, with a code from \(vendor)'s own page. No computer needed. Allternit never sees your password."
                     : "\(entry?.name ?? vendor) on your computer signs in with a code from \(vendor)'s own page. Allternit never sees your password.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            switch phase {
            case .disclosure(let disclosure):
                ConnDisclosureView(disclosure: disclosure, onAccept: {
                    accepted = disclosure.version
                    attempt += 1
                }, onDecline: {
                    onFallback(store.row(id).fallbacks.first { $0.hasSuffix("-key") } ?? ConnectionsCatalog.keyFallback(id))
                })
            case .starting:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Getting your \(vendor) sign-in ready…").font(.system(size: 13.5)).foregroundColor(Color("TextSecondary"))
                }
            case .vendor:
                if let login { vendorStep(login) }
            case .done:
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                    Text(store.statusLine(entry ?? ConnEntry(id: id, group: .tools, name: vendor, logo: .symbol("key"), ways: [.code])))
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                }
            case .expired:
                retry("The code ran out. Get a new one.")
            case .failed(let message):
                retry(message)
            }
        }
        .task(id: attempt) { await run() }
        .onReceive(Timer.publish(every: 1, on: .main, in: .common).autoconnect()) { now = $0 }
        .onChange(of: scenePhase) { _, next in
            if next == .active { pickUpCopiedCode() }
        }
    }

    /// Claude's paste-back code (contract §6): back from Safari with a new
    /// clipboard, take the code from it without a Paste tap.
    private func pickUpCopiedCode() {
        guard phase == .vendor, opened, !submitting, login?.kind == .linkPaste,
              let before = pasteboardAtOpen, UIPasteboard.general.changeCount != before,
              UIPasteboard.general.hasStrings else { return }
        pasteboardAtOpen = UIPasteboard.general.changeCount
        let text = (UIPasteboard.general.string ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard Self.looksLikeCode(text) else { return }
        Task { await submitCode(text) }
    }

    /// One token, no spaces, a sane length: a code, not a copied sentence.
    static func looksLikeCode(_ text: String) -> Bool {
        (8...512).contains(text.count) && !text.contains(where: \.isWhitespace)
    }

    private func retry(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(message).font(.system(size: 13.5)).foregroundColor(Color("TextPrimary"))
            HStack(spacing: 8) {
                Button("Try again") { attempt += 1 }.buttonStyle(ConnPrimaryButtonStyle())
                if onRunner {
                    Button("Use a key instead") { onFallback(ConnectionsCatalog.keyFallback(id)) }.buttonStyle(ConnQuietButtonStyle())
                } else if !store.computerOnline {
                    Button("Add a computer") { onFallback("this-mac") }.buttonStyle(ConnQuietButtonStyle())
                }
            }
        }
    }

    @ViewBuilder
    private func vendorStep(_ login: ConnLogin) -> some View {
        switch login.kind {
        case .linkPaste:
            let steps = ["Open \(vendor)", "Approve", "Copy the code", "Done"]
            VStack(alignment: .leading, spacing: 10) {
                ConnStepStrip(steps: steps, at: opened ? 2 : 0)
                if !opened {
                    Text("\(vendor) opens in Safari. Approve, then tap Copy on the code \(vendor) shows. Come back here: the code is picked up from the clipboard, or press Paste.")
                        .font(.system(size: 13.5)).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
                    openButton(login)
                } else if submitting {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Checking the code…").font(.system(size: 13.5)).foregroundColor(Color("TextSecondary"))
                    }
                } else {
                    HStack(spacing: 8) {
                        PasteButton(payloadType: String.self) { strings in
                            let pasted = strings.first ?? ""
                            Task { @MainActor in await submitCode(pasted) }
                        }
                        .buttonBorderShape(.capsule)
                        .labelStyle(.titleAndIcon)
                        .tint(Color("TextPrimary"))
                        openButton(login, label: "Open \(vendor) again", quiet: true)
                    }
                }
                countdown(login)
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 14).fill(Color("FillNeutral")))
        default:
            // device_code (Codex, Grok) and oauth: Open, then it finishes by itself.
            let prefilled = login.urlWithCode != nil
            let steps = prefilled ? ["Open \(vendor)", "Confirm", "Done"] : ["Open \(vendor)", "Paste the code", "Done"]
            VStack(alignment: .leading, spacing: 10) {
                ConnStepStrip(steps: steps, at: opened ? 1 : 0)
                if let code = login.code {
                    HStack(spacing: 10) {
                        Text(code)
                            .font(.system(size: 22, weight: .semibold, design: .monospaced))
                            .tracking(3)
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                            .background(RoundedRectangle(cornerRadius: 10).fill(Color("BgPrimary")))
                            .textSelection(.enabled)
                            .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
                        Button {
                            UIPasteboard.general.string = code
                            copied = true
                        } label: {
                            Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        }
                        .buttonStyle(ConnQuietButtonStyle())
                    }
                    Text(prefilled
                         ? "\(vendor) opens with your code filled in. Check it shows \(code) and tap Confirm."
                         : "We copy the code when you tap Open. Paste it on \(vendor)'s page.")
                        .font(.system(size: 13.5)).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
                }
                if opened {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Waiting for you on \(vendor)… this finishes by itself").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                    }
                    openButton(login, label: "Open \(vendor) again", quiet: true)
                } else {
                    openButton(login)
                }
                countdown(login)
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 14).fill(Color("FillNeutral")))
        }
    }

    private func openButton(_ login: ConnLogin, label: String? = nil, quiet: Bool = false) -> some View {
        Button {
            if login.kind == .deviceCode, login.urlWithCode == nil, let code = login.code {
                UIPasteboard.general.string = code
                copied = true
            }
            pasteboardAtOpen = UIPasteboard.general.changeCount
            if let url = login.openURL { openURL(url) }
            opened = true
        } label: {
            Label(label ?? "Open \(vendor)", systemImage: "arrow.up.right.square")
        }
        .buttonStyle(quiet ? AnyButtonStyle(ConnQuietButtonStyle()) : AnyButtonStyle(ConnPrimaryButtonStyle()))
        .disabled(login.openURL == nil)
    }

    @ViewBuilder
    private func countdown(_ login: ConnLogin) -> some View {
        if let expiry = login.expiry {
            let left = max(0, Int(expiry.timeIntervalSince(now)))
            Text("Code works for \(left / 60):\(String(format: "%02d", left % 60))")
                .font(.system(size: 12).monospacedDigit())
                .foregroundColor(Color("TextTertiary"))
        }
    }

    private func run() async {
        phase = .starting
        opened = false
        copied = false
        login = nil
        store.setConnecting(id, true)
        defer { if phase != .done { store.setConnecting(id, false) } }
        let target = store.target(for: id)
        do {
            let result = try await ConnectionsClient.connect(id, target: target, way: "code", acceptDisclosure: accepted)
            if result.state == .on, result.login == nil {
                store.markOn(id)
                phase = .done
                onConnected()
                return
            }
            guard let login = result.login else {
                phase = .failed("Could not start. Try again")
                return
            }
            self.login = login
            phase = .vendor
            guard login.poll else { return }
            while !Task.isCancelled {
                try await Task.sleep(for: .seconds(2))
                let poll = try await ConnectionsClient.login(id, target: target)
                switch poll.status {
                case .pending:
                    continue
                case .done:
                    if let row = poll.row { store.apply(row) } else { store.markOn(id) }
                    phase = .done
                    onConnected()
                    return
                case .expired:
                    phase = .expired
                    return
                case .failed:
                    phase = .failed("The sign-in didn't finish. Try again.")
                    return
                }
            }
        } catch is CancellationError {
            return
        } catch ConnectionsError.disclosureRequired(let disclosure) {
            // §9.2: read and accept first (also when the version changed).
            store.setConnecting(id, false)
            phase = .disclosure(disclosure)
        } catch {
            phase = .failed(ConnectionsClient.describe(error))
        }
    }

    private func submitCode(_ raw: String) async {
        let code = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty else { return }
        submitting = true
        defer { submitting = false }
        do {
            let row = try await ConnectionsClient.submit(id, target: store.target(for: id), value: code)
            store.apply(row)
            if row.state == .on {
                phase = .done
                onConnected()
            }
            // Otherwise the /login poll reports the result.
        } catch ConnectionsError.disclosureRequired(let disclosure) {
            phase = .disclosure(disclosure)
        } catch {
            phase = .failed(ConnectionsClient.describe(error))
        }
    }
}

/// The disclosure step (contract §9.2, §9.8): the server's title and
/// sentences exactly as sent, then accept or use a key instead. Nothing is
/// sent on decline.
struct ConnDisclosureView: View {
    let disclosure: ConnDisclosure
    var busy = false
    let onAccept: () -> Void
    let onDecline: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(disclosure.title)
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(disclosure.body.enumerated()), id: \.offset) { _, line in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Circle().fill(Color("TextTertiary")).frame(width: 4, height: 4).accessibilityHidden(true)
                        Text(line)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextPrimary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
            HStack(spacing: 8) {
                Button(action: onAccept) {
                    HStack(spacing: 6) {
                        if busy { ProgressView().controlSize(.small).tint(Color("BgPrimary")) }
                        Text(disclosure.acceptLabel)
                    }
                }
                .buttonStyle(ConnPrimaryButtonStyle())
                .disabled(busy)
                .accessibilityIdentifier("conn-disclosure-accept")
                Button(disclosure.declineLabel, action: onDecline)
                    .buttonStyle(ConnQuietButtonStyle())
                    .disabled(busy)
                    .accessibilityIdentifier("conn-disclosure-decline")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-disclosure")
    }
}
