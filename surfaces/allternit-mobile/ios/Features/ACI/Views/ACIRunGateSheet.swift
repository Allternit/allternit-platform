import SwiftUI
import LocalAuthentication

/// What a run is waiting on. One at a time; the view model holds the latest.
enum ACIRunGate: Identifiable, Equatable {
    case approval(ACIApprovalRequest)
    case needsYou(ACIHumanRequest)
    case blocked(ACIBlock)

    var id: String {
        switch self {
        case .approval(let a): return "approval-\(a.approvalId)"
        case .needsYou(let h): return "human-\(h.runId ?? "run")-\(h.reason)"
        case .blocked(let b): return "block-\(b.runId ?? "run")-\(b.domain)-\(b.kind)"
        }
    }
}

/// Face ID / fingerprint / device passcode before an approval is sent.
enum ACIDeviceAuth {
    /// True when the owner proved it is them. A phone with no passcode set
    /// cannot prove anything, so it answers false and the approval is not sent.
    static func confirm(reason: String) async -> Bool {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else { return false }
        return (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) ?? false
    }
}

/// The sheet that appears over the run view when the bot needs the owner:
/// credential approval, needs-you, or blocked. Copy follows CONTRACT.md.
struct ACIRunGateSheet: View {
    let gate: ACIRunGate
    let botName: String
    let onApprove: (_ scope: String?) -> Void
    let onDeny: () -> Void
    let onContinue: () -> Void
    let onStop: () -> Void
    let onRerun: (ACIRerunTarget) -> Void
    /// Typed secret + optional vault name; the sheet clears its field after sending.
    var onSecret: (String, String?) -> Void = { _, _ in }

    @State private var secret = ""
    @State private var saveToVault = false
    @State private var vaultName = ""
    @State private var authFailed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            switch gate {
            case .approval(let request): approval(request)
            case .needsYou(let request): needsYou(request)
            case .blocked(let block): blocked(block)
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white)
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
        .presentationBackground(Color.white)
    }

    // MARK: Credential or action approval

    @ViewBuilder
    private func approval(_ request: ACIApprovalRequest) -> some View {
        if request.isCredential, let name = request.credentialName {
            let domain = request.credentialDomain ?? "this site"
            title("Fill your \(name) login on \(domain)?")
            caption("\(botName) will fill it in. The password never shows on screen or in the log.")
            primary("Approve") { Task { await approveWithAuth(scope: nil, reason: "Fill your \(name) login on \(domain)") } }
            secondary("Allow for this run") { Task { await approveWithAuth(scope: "run", reason: "Allow \(name) on \(domain) for this run") } }
            destructive("Deny", action: onDeny)
        } else {
            title("\(botName) wants to do something")
            caption(request.message ?? "Review it before it goes ahead.")
            primary("Approve") { Task { await approveWithAuth(scope: nil, reason: "Approve this action") } }
            destructive("Deny", action: onDeny)
        }
        if authFailed {
            caption("Face ID or your passcode didn't confirm, so nothing was approved.")
        }
    }

    private func approveWithAuth(scope: String?, reason: String) async {
        let ok = await ACIDeviceAuth.confirm(reason: reason)
        await MainActor.run {
            authFailed = !ok
            if ok { onApprove(scope) }
        }
    }

    // MARK: Needs you

    @ViewBuilder
    private func needsYou(_ request: ACIHumanRequest) -> some View {
        title("\(botName) needs you")
        caption(request.reason)
        if request.kind == "safari_profile" {
            ForEach(Array(request.steps.enumerated()), id: \.offset) { index, step in
                caption("\(index + 1). \(step)")
            }
            caption("Do these on your Mac, then tap Continue.")
        }
        if request.secret {
            SecureField("Secret", text: $secret)
                .textContentType(.password)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .padding(12)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(PolishTokens.neutralFill))
            Toggle("Save to vault", isOn: $saveToVault)
                .font(.system(size: 14))
            if saveToVault {
                TextField("Name", text: $vaultName)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .padding(12)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(PolishTokens.neutralFill))
            }
            primary("Send") {
                let value = secret
                let name = saveToVault ? vaultName.trimmingCharacters(in: .whitespaces) : ""
                secret = ""
                vaultName = ""
                onSecret(value, name.isEmpty ? nil : name)
            }
        } else {
            primary("Continue", action: onContinue)
        }
        destructive("Stop", action: onStop)
    }

    // MARK: Blocked

    @ViewBuilder
    private func blocked(_ block: ACIBlock) -> some View {
        title("\(block.domain) blocked \(botName)")
        caption(blockReason(block.kind))
        primary("Continue", action: onContinue)
        if block.rerunTargets.isEmpty {
            caption("No other computer is ready to re-run this. Add one in Computers.")
        } else {
            ForEach(block.rerunTargets) { target in
                secondary("Re-run on \(target.name)") { onRerun(target) }
            }
            caption("A re-run starts fresh. No logins carry over.")
        }
        destructive("Stop", action: onStop)
    }

    private func blockReason(_ kind: String) -> String {
        switch kind {
        case "login": return "The site wants you to sign in."
        case "two_factor": return "The site wants a verification code."
        case "captcha": return "The site is showing a puzzle."
        case "rate_limited": return "The site says there were too many requests."
        default: return "The site is not letting the bot in."
        }
    }

    // MARK: Pieces

    private func title(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 20, weight: .semibold))
            .foregroundColor(PolishTokens.ink)
    }

    private func caption(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 14))
            .foregroundColor(Color("TextSecondary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func primary(_ text: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(text)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(PolishTokens.ink))
        }
        .buttonStyle(.plain)
    }

    private func secondary(_ text: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(text)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(PolishTokens.ink)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(PolishTokens.neutralFill))
        }
        .buttonStyle(.plain)
    }

    private func destructive(_ text: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(text)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color(hex: "#C0392B"))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
        }
        .buttonStyle(.plain)
    }
}
