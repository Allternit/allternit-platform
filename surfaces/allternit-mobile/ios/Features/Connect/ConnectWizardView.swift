import AuthenticationServices
import SwiftUI
import UIKit

// -----------------------------------------------------------------------------
// The step engine every connect flow runs on — the native port of allternit-ai
// `src/components/gateway/connect/ConnectWizard.tsx`: intro, action (the
// platform's own authorize page, opened in an in-app web sign-in sheet),
// form, waiting (live status from a poll, with cancel), code entry and done.
// Provider flows (ChannelConnectFlows.swift) own the steps and the current
// step; this view owns rendering, back/cancel, polling and errors.
// -----------------------------------------------------------------------------

struct WizardOption: Hashable {
    let value: String
    let label: String
    var hint: String? = nil
}

struct WizardField {
    enum Kind { case text, secure, multiline, radio([WizardOption]), select([WizardOption]), checkboxes([WizardOption]) }
    let key: String
    let label: String
    var kind: Kind = .text
    var placeholder: String? = nil
    var hint: String? = nil
    var optional = false
    var contentType: UITextContentType? = nil
    var keyboard: UIKeyboardType = .default

    var isMulti: Bool { if case .checkboxes = kind { return true } else { return false } }
}

/// What a form step collected.
struct WizardValues {
    var text: [String: String] = [:]
    var many: [String: Set<String>] = [:]
    func string(_ key: String) -> String { (text[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
}

/// One status poll (types.ts WaitingPoll).
struct WaitingPoll {
    enum State { case pending, done, failed }
    var state: State
    var detail: String? = nil
    /// An outside action offered mid-wait ("Open Telegram to confirm").
    var action: (label: String, url: URL)? = nil
    var error: String? = nil
}

struct ConnectStep {
    enum Kind {
        case intro(body: String?, bullets: [String])
        /// `signIn` opens the URL in an ASWebAuthenticationSession (OAuth / install
        /// pages); otherwise the system opens it (a Telegram link, an app).
        case action(body: String?, label: String, url: URL, hint: String?, doneLabel: String, signIn: Bool)
        case form(body: String?, fields: [WizardField], initial: WizardValues, submitLabel: String,
                  onSubmit: @MainActor (WizardValues) async throws -> String?)
        case waiting(body: String?, status: String, pollSeconds: Double, skippable: Bool, cancelLabel: String?,
                     poll: @MainActor () async throws -> WaitingPoll)
        case code(body: String?, length: Int, onSubmit: @MainActor (String) async throws -> String?,
                  onResend: (@MainActor () async -> Void)?)
        case done(body: String)
    }
    let id: String
    let title: String
    let kind: Kind
}

/// A provider flow: its steps (rebuilt from its own state) and the current step.
@MainActor
class ConnectFlow: ObservableObject, Identifiable {
    nonisolated let id = UUID()
    @Published var step: String
    /// Bumped by subclasses when state their steps read changes.
    @Published var revision = 0
    let title: String
    let icon: String
    /// The bot this was opened for; a new connection is switched on for it.
    let botId: String?
    /// The connection's account id, once there is one.
    var accountId: String?
    /// Set when the backend for this connection isn't on the account yet.
    @Published var comingSoon: String?
    @Published var preparing = false

    init(title: String, icon: String, botId: String?, firstStep: String) {
        self.title = title
        self.icon = icon
        self.botId = botId
        self.step = firstStep
    }

    var steps: [ConnectStep] { [] }

    /// Runs before the first step (availability checks, start links).
    func prepare() async {}

    /// The flow finished: switch the connection on for the bot it was opened for.
    func finish() async {
        if let botId, let accountId, !["slack", "discord", "teams", "whatsapp"].contains(accountId) {
            try? await BotModeClient.shared.setChannel(botId: botId, accountId: accountId, enabled: true)
        }
        await BotPhonesStore.shared.refresh()
    }

    func changed() { revision += 1 }
}

// MARK: - Web sign-in (ASWebAuthenticationSession)

/// Opens a provider's authorize page in the system web sign-in sheet (shares
/// Safari's cookies, so a signed-in Slack/Microsoft/Meta account carries over)
/// and returns when the person closes it. These pages finish on Allternit's
/// cloud, not with a redirect back to the app, so closing the sheet is the
/// return; the flow then checks the connection.
@MainActor
final class WebSignIn: NSObject {
    static let shared = WebSignIn()
    private var session: ASWebAuthenticationSession?

    func open(_ url: URL) async {
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            let session = ASWebAuthenticationSession(url: url, callbackURLScheme: nil) { _, _ in
                done.resume()
            }
            session.presentationContextProvider = self
            session.prefersEphemeralWebBrowserSession = false
            self.session = session
            if !session.start() { done.resume() }
        }
        session = nil
    }

}

extension WebSignIn: @preconcurrency ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        return scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first ?? UIWindow()
    }
}

// MARK: - Engine view

struct ConnectWizardView: View {
    @ObservedObject var flow: ConnectFlow
    var onClose: () -> Void

    @Environment(\.openURL) private var openURL
    @State private var values: [String: WizardValues] = [:]
    @State private var history: [String] = []
    @State private var busy = false
    @State private var error: String?
    @State private var poll: WaitingPoll?
    @State private var retry = 0
    @State private var code = ""
    @State private var prepared = false

    private var steps: [ConnectStep] { _ = flow.revision; return flow.steps }
    private var current: ConnectStep? { steps.first { $0.id == flow.step } ?? steps.first }
    private var index: Int { steps.firstIndex { $0.id == current?.id } ?? 0 }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if !prepared || flow.preparing {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("Checking what \(flow.title.replacingOccurrences(of: "Connect ", with: "")) needs…")
                                .font(.subheadline).foregroundColor(Color("TextSecondary"))
                        }
                        .padding(.top, 24)
                    } else if let soon = flow.comingSoon {
                        comingSoon(soon)
                    } else if let current {
                        progress
                        Text(current.title)
                            .font(.title3.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityAddTraits(.isHeader)
                        stepBody(current)
                        if let error {
                            Text(error)
                                .font(.footnote)
                                .foregroundColor(Theme.statusError)
                                .fixedSize(horizontal: false, vertical: true)
                                .accessibilityLabel("Error: \(error)")
                        }
                    }
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .bottom) { if prepared && flow.comingSoon == nil, let current { footer(current) } }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle(flow.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { onClose() }.disabled(busy)
                }
                ToolbarItem(placement: .principal) {
                    Label(flow.title, systemImage: flow.icon).labelStyle(.titleAndIcon).font(.headline)
                }
            }
            .task {
                guard !prepared else { return }
                await flow.prepare()
                prepared = true
            }
            .task(id: "\(flow.step)#\(retry)#\(prepared)") { await runPoll() }
        }
        .interactiveDismissDisabled(busy)
    }

    // MARK: Pieces

    private var progress: some View {
        HStack(spacing: 6) {
            ForEach(Array(steps.enumerated()), id: \.offset) { n, _ in
                Capsule()
                    .fill(n <= index ? Color("AccentPrimary") : Color("TextSecondary").opacity(0.25))
                    .frame(width: n == index ? 22 : 8, height: 6)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(index + 1) of \(steps.count)")
    }

    private func comingSoon(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Image(systemName: flow.icon)
                .font(.system(size: 26, weight: .medium))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 56, height: 56)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
                .accessibilityHidden(true)
            Text(text)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Button("Close") { onClose() }
                .font(.subheadline.weight(.semibold))
        }
        .padding(.top, 8)
    }

    private func para(_ text: String?) -> some View {
        Group {
            if let text, !text.isEmpty {
                Text(text)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder
    private func stepBody(_ step: ConnectStep) -> some View {
        switch step.kind {
        case .intro(let body, let bullets):
            para(body)
            ForEach(bullets, id: \.self) { b in
                Label {
                    Text(b).font(.subheadline).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "checkmark").foregroundColor(Theme.statusSuccess)
                }
            }
        case .action(let body, let label, let url, let hint, _, let signIn):
            para(body)
            Button {
                Task { await openAction(url, signIn: signIn) }
            } label: {
                Text(label).font(.headline).frame(maxWidth: .infinity, minHeight: 46)
            }
            .buttonStyle(.borderedProminent)
            .tint(Color("AccentPrimary"))
            .disabled(busy)
            if let hint { Text(hint).font(.caption).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true) }
        case .form(let body, let fields, let initial, _, _):
            para(body)
            ForEach(fields, id: \.key) { field in fieldView(field, stepId: step.id, initial: initial) }
        case .waiting(let body, let status, _, _, _, _):
            let failed = poll?.state == .failed
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    if failed {
                        Image(systemName: "exclamationmark.circle.fill").foregroundColor(Theme.statusError)
                    } else {
                        ProgressView()
                    }
                    Text(failed ? "This hit a snag" : status)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                }
                .accessibilityElement(children: .combine)
                para(failed ? (poll?.error ?? "Something went wrong.") : (poll?.detail ?? body))
                    .accessibilityAddTraits(.updatesFrequently)
                if !failed, let action = poll?.action {
                    Button(action.label) { openURL(action.url) }
                        .buttonStyle(.bordered)
                        .frame(maxWidth: .infinity)
                }
                if failed {
                    Button("Try again") { poll = nil; retry += 1 }
                        .buttonStyle(.borderedProminent)
                        .tint(Color("AccentPrimary"))
                }
            }
        case .code(let body, let length, _, let onResend):
            para(body)
            TextField(String(repeating: "0", count: length), text: $code)
                .keyboardType(.numberPad)
                .textContentType(.oneTimeCode)
                .font(.system(size: 22, weight: .semibold, design: .monospaced))
                .multilineTextAlignment(.center)
                .padding(12)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
                .onChange(of: code) { _, v in
                    let digits = String(v.filter(\.isNumber).prefix(length))
                    if digits != v { code = digits }
                }
                .accessibilityLabel("\(step.title) code")
            if let onResend {
                Button("Resend the code") { Task { await onResend() } }
                    .font(.footnote)
            }
        case .done(let body):
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 40))
                .foregroundColor(Theme.statusSuccess)
                .accessibilityHidden(true)
            para(body)
        }
    }

    private func binding(_ stepId: String, _ key: String, initial: WizardValues) -> Binding<String> {
        Binding(
            get: { values[stepId]?.text[key] ?? initial.text[key] ?? "" },
            set: { v in
                var vals = values[stepId] ?? initial
                vals.text[key] = v
                values[stepId] = vals
            }
        )
    }

    private func picked(_ stepId: String, _ key: String, initial: WizardValues) -> Set<String> {
        values[stepId]?.many[key] ?? initial.many[key] ?? []
    }

    @ViewBuilder
    private func fieldView(_ field: WizardField, stepId: String, initial: WizardValues) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(field.label + (field.optional ? " (optional)" : ""))
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            switch field.kind {
            case .text, .secure, .multiline:
                inputField(field, text: binding(stepId, field.key, initial: initial))
            case .select(let options):
                let value = binding(stepId, field.key, initial: initial)
                Picker(field.label, selection: value) {
                    Text(field.placeholder ?? "Choose…").tag("")
                    ForEach(options, id: \.value) { Text($0.label).tag($0.value) }
                }
                .pickerStyle(.menu)
                .tint(Color("TextPrimary"))
            case .radio(let options):
                let value = binding(stepId, field.key, initial: initial)
                ForEach(options, id: \.value) { o in
                    optionRow(o, on: value.wrappedValue == o.value, multi: false) { value.wrappedValue = o.value }
                }
            case .checkboxes(let options):
                let set = picked(stepId, field.key, initial: initial)
                ForEach(options, id: \.value) { o in
                    optionRow(o, on: set.contains(o.value), multi: true) {
                        var vals = values[stepId] ?? initial
                        var s = vals.many[field.key] ?? []
                        if s.contains(o.value) { s.remove(o.value) } else { s.insert(o.value) }
                        vals.many[field.key] = s
                        values[stepId] = vals
                    }
                }
            }
            if let hint = field.hint {
                Text(hint).font(.caption).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func inputField(_ field: WizardField, text: Binding<String>) -> some View {
        Group {
            switch field.kind {
            case .secure:
                SecureField(field.placeholder ?? "", text: text)
            case .multiline:
                TextField(field.placeholder ?? "", text: text, axis: .vertical).lineLimit(3...6)
            default:
                TextField(field.placeholder ?? "", text: text)
            }
        }
        .textContentType(field.contentType)
        .keyboardType(field.keyboard)
        .autocorrectionDisabled()
        .textInputAutocapitalization(field.contentType == .name || field.contentType == .organizationName ? .words : .never)
        .padding(11)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous))
        .accessibilityLabel(field.label)
    }

    private func optionRow(_ o: WizardOption, on: Bool, multi: Bool, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: multi ? (on ? "checkmark.square.fill" : "square") : (on ? "largecircle.fill.circle" : "circle"))
                    .foregroundColor(on ? Color("AccentPrimary") : Color("TextSecondary"))
                VStack(alignment: .leading, spacing: 2) {
                    Text(o.label).font(.subheadline.weight(.medium)).foregroundColor(Color("TextPrimary"))
                    if let hint = o.hint {
                        Text(hint).font(.caption).foregroundColor(Color("TextSecondary")).multilineTextAlignment(.leading)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(11)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: Theme.radiusSM, style: .continuous)
                .stroke(on ? Color("AccentPrimary") : Color.clear, lineWidth: 1.5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    // MARK: Footer

    @ViewBuilder
    private func footer(_ step: ConnectStep) -> some View {
        HStack(spacing: 12) {
            if !history.isEmpty, !isWaitingOrDone(step) {
                Button("Back") { back() }.disabled(busy)
            } else if case .waiting(_, _, _, _, let cancelLabel, _) = step.kind {
                Button(cancelLabel ?? "Cancel") { onClose() }.disabled(busy)
            }
            Spacer()
            primary(step)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .background(.bar)
    }

    private func isWaitingOrDone(_ step: ConnectStep) -> Bool {
        switch step.kind {
        case .waiting, .done: return true
        default: return false
        }
    }

    @ViewBuilder
    private func primary(_ step: ConnectStep) -> some View {
        if busy {
            ProgressView().frame(minHeight: 40)
        } else {
            switch step.kind {
            case .intro: primaryButton("Continue") { goNext() }
            case .action(_, _, _, _, let doneLabel, _): primaryButton(doneLabel) { goNext() }
            case .form(_, _, _, let submitLabel, _): primaryButton(submitLabel) { Task { await submit(step) } }
            case .code(_, let length, _, _): primaryButton("Verify", disabled: code.count < length) { Task { await submit(step) } }
            case .waiting(_, _, _, let skippable, _, _):
                if skippable { primaryButton("Continue") { goNext() } }
            case .done:
                primaryButton("Done") {
                    busy = true
                    Task {
                        await flow.finish()
                        busy = false
                        onClose()
                    }
                }
            }
        }
    }

    private func primaryButton(_ title: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.headline).padding(.horizontal, 6).frame(minHeight: 40)
        }
        .buttonStyle(.borderedProminent)
        .tint(Color("AccentPrimary"))
        .disabled(disabled)
    }

    // MARK: Navigation

    private func go(_ id: String) {
        if let cur = current?.id { history.removeAll { $0 == cur }; history.append(cur) }
        error = nil
        poll = nil
        code = ""
        flow.step = id
    }

    private func goNext() {
        let all = steps
        if all.indices.contains(index + 1) { go(all[index + 1].id) }
    }

    private func back() {
        guard let prev = history.popLast() else { return }
        error = nil
        poll = nil
        flow.step = prev
    }

    private func openAction(_ url: URL, signIn: Bool) async {
        if signIn {
            await WebSignIn.shared.open(url)
            // Back from the sign-in sheet: go check the connection.
            goNext()
        } else {
            openURL(url)
        }
    }

    private func submit(_ step: ConnectStep) async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            var next: String?
            switch step.kind {
            case .form(_, let fields, let initial, _, let onSubmit):
                var vals = values[step.id] ?? initial
                for f in fields where vals.text[f.key] == nil { vals.text[f.key] = initial.text[f.key] }
                let missing = fields.filter { f in
                    !f.optional && (f.isMulti ? (vals.many[f.key] ?? []).isEmpty : vals.string(f.key).isEmpty)
                }
                if !missing.isEmpty {
                    error = "Fill in: \(missing.map(\.label).joined(separator: ", "))."
                    return
                }
                next = try await onSubmit(vals)
            case .code(_, _, let onSubmit, _):
                next = try await onSubmit(code)
            default:
                return
            }
            if let next { go(next) } else { goNext() }
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func runPoll() async {
        guard prepared, let step = current, case .waiting(_, _, let seconds, _, _, let pollFn) = step.kind else { return }
        let stepId = step.id
        while !Task.isCancelled {
            do {
                let r = try await pollFn()
                guard !Task.isCancelled, flow.step == stepId else { return }
                poll = r
                switch r.state {
                case .pending:
                    try? await Task.sleep(nanoseconds: UInt64(max(1, seconds) * 1_000_000_000))
                case .done:
                    try? await Task.sleep(nanoseconds: 60_000_000)
                    // Advance from the freshest steps: a poll may have added one (Discord's channel pick).
                    let all = steps
                    if let i = all.firstIndex(where: { $0.id == stepId }), all.indices.contains(i + 1) { go(all[i + 1].id) }
                    return
                case .failed:
                    return
                }
            } catch is CancellationError {
                return
            } catch {
                poll = WaitingPoll(state: .failed, error: error.localizedDescription)
                return
            }
        }
    }
}
