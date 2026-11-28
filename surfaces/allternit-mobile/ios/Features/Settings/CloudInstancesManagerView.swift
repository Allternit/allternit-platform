import SwiftUI

/// Phase-1 Cloud Instances manager for iOS.
///
/// Mirrors the web's CloudInstancesPanel wizard flow at a surface level:
/// list deployment sessions, start a manual SSH-based wizard, advance the
/// state machine, bootstrap, cancel, and delete. Automated provider flows
/// and saved provider tokens are deferred.
struct CloudInstancesManagerView: View {
    @StateObject private var store = CloudInstancesStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var isCreateSheetPresented = false
    @State private var actionError: String? = nil

    var body: some View {
        DockingTitleScrollView(title: "Cloud Instances", description: "BYO-VPS deployment wizard", chrome: .navigationBar) {
            content
        }
        .refreshable {
            await store.refresh()
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle("Cloud Instances")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityLabel("Close")
            }
        }
        .task {
            store.fetchIfNeeded()
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CloudInstancesCreateSheet()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.sessions.isEmpty {
            ProgressView()
                .frame(maxWidth: .infinity)
                .padding(.vertical, 40)
        } else if let loadError = store.loadError, store.sessions.isEmpty {
            let offline = isConnectionFailure(loadError)
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: "Failed to load deployments",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            .padding(.vertical, 20)
        } else {
            VStack(spacing: 16) {
                actionBar
                if let actionError {
                    errorBanner(actionError)
                }
                if store.activeSession != nil {
                    runCard
                }
                sessionList
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 8)
        }
    }

    // MARK: - Action bar

    private var actionBar: some View {
        HStack(spacing: 12) {
            Button(action: { store.fetchIfNeeded(force: true) }) {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .semibold))
                    Text("Refresh")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Theme.borderWarmDefault, lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Refresh")

            Button(action: { isCreateSheetPresented = true }) {
                HStack(spacing: 6) {
                    Image(systemName: "plus")
                        .font(.system(size: 12, weight: .semibold))
                    Text("Deploy Instance")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("AccentPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("AccentPrimary").opacity(0.25), lineWidth: 1)
                )
            }
            .buttonStyle(.plain)

            Spacer()
        }
    }

    // MARK: - Session list

    @ViewBuilder
    private var sessionList: some View {
        if store.sessions.isEmpty {
            emptyState
        } else {
            VStack(spacing: 12) {
                ForEach(store.sessions) { session in
                    sessionRow(session)
                }
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "cloud",
            title: "No cloud deployments",
            message: "Deploy an Allternit runtime to your own server.",
            actionTitle: "Deploy Instance",
            action: { isCreateSheetPresented = true }
        )
        .padding(.vertical, 24)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func sessionRow(_ session: CloudWizardSession) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.context.instanceName ?? "Deployment \(session.id.prefix(8))")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(providerLabel(session.context.provider ?? "manual"))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 8)
                stepBadge(session.currentStep)
            }

            if let ip = session.context.instanceIp {
                Text("IP: \(ip)")
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
            }

            HStack {
                Spacer()
                if store.activeSession?.id == session.id {
                    Text("Active")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(Color("AccentPrimary"))
                } else {
                    Button(action: { store.setActiveSession(session) }) {
                        Text("Resume")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("AccentPrimary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 6)
                            .background(Color("AccentPrimary").opacity(0.12))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                    .buttonStyle(.plain)
                }

                Button(action: { deleteSession(session.id) }) {
                    Image(systemName: "trash")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Theme.statusError)
                        .padding(8)
                        .background(Theme.statusError.opacity(0.08))
                        .clipShape(Circle())
                }
                .buttonStyle(.plain)
                .disabled(store.isDeletingId == session.id)
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Run card

    @ViewBuilder
    private var runCard: some View {
        if let session = store.activeSession {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(session.context.instanceName ?? "Active Deployment")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text("Step: \(stepLabel(session.currentStep))")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    Button(action: { store.setActiveSession(nil) }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .buttonStyle(.plain)
                }

                if let result = store.bootstrapResult {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 6) {
                            Image(systemName: "checkmark.circle")
                                .foregroundColor(Theme.statusSuccess)
                            Text("\(result.instanceName) is online")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundColor(Theme.statusSuccess)
                        }
                        Text("Mesh IP: \(result.meshIp)")
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                        Text("URL: \(result.url)")
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .padding(12)
                    .background(Theme.statusSuccess.opacity(0.08))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                } else {
                    VStack(alignment: .leading, spacing: 8) {
                        stepItem("Validate credentials", done: stepRank(session.currentStep) > 2, active: session.currentStep == "ValidateCredentials")
                        stepItem("Preflight checks", done: stepRank(session.currentStep) > 3, active: session.currentStep == "Preflight")
                        stepItem("Connect to server", done: stepRank(session.currentStep) > 4, active: session.currentStep == "Provisioning")
                        stepItem("Install Allternit", done: ["Verification", "Complete"].contains(session.currentStep), active: session.currentStep == "Bootstrap")
                    }

                    if let guidance = session.context.agentGuidance?.last {
                        Text(guidance)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    HStack {
                        if isAdvanceable(session.currentStep) {
                            Button(action: advance) {
                                actionLabel("Advance", icon: "arrow.right", loading: store.isAdvancing)
                            }
                            .buttonStyle(.plain)
                            .disabled(store.isAdvancing)
                        }
                        if session.currentStep == "Bootstrap" {
                            Button(action: bootstrap) {
                                actionLabel("Install Allternit", icon: "arrow.down.circle", loading: store.isBootstrapping)
                            }
                            .buttonStyle(.plain)
                            .disabled(store.isBootstrapping)
                        }
                        Spacer()
                        if !isTerminal(session.currentStep) {
                            Button(action: cancel) {
                                actionLabel("Cancel", icon: "xmark", color: Theme.statusError, loading: store.isCancelling)
                            }
                            .buttonStyle(.plain)
                            .disabled(store.isCancelling)
                        }
                    }
                }
            }
            .padding(14)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusLG)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
        }
    }

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private func stepItem(_ label: String, done: Bool, active: Bool) -> some View {
        HStack(spacing: 8) {
            if done {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(Theme.statusSuccess)
            } else if active {
                ProgressView()
                    .scaleEffect(0.6)
            } else {
                Circle()
                    .fill(Color("TextSecondary").opacity(0.3))
                    .frame(width: 8, height: 8)
            }
            Text(label)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(done || active ? Color("TextPrimary") : Color("TextSecondary"))
        }
    }

    private func stepLabel(_ step: String) -> String {
        switch step {
        case "SelectProvider": return "Provider selected"
        case "EnterCredentials": return "Credentials entered"
        case "ValidateCredentials": return "Validating credentials"
        case "Preflight": return "Preflight checks"
        case "Provisioning": return "Connecting"
        case "Bootstrap": return "Ready to install"
        case "Verification": return "Verifying"
        case "Complete": return "Complete"
        case "Failed": return "Failed"
        case "Cancelled": return "Cancelled"
        default: return "Waiting"
        }
    }

    private func stepRank(_ step: String) -> Int {
        switch step {
        case "SelectProvider", "AgentAssistedSignup", "Cancelled": return 0
        case "HumanPaymentCheckpoint", "HumanVerificationCheckpoint", "AwaitingHumanAction", "EnterCredentials": return 1
        case "ValidateCredentials": return 2
        case "Preflight": return 3
        case "Provisioning": return 4
        case "Bootstrap", "Failed": return 5
        case "Verification": return 6
        case "Complete": return 7
        default: return 0
        }
    }

    private func isAdvanceable(_ step: String) -> Bool {
        ["SelectProvider", "EnterCredentials", "ValidateCredentials", "Preflight", "Provisioning"].contains(step)
    }

    private func isTerminal(_ step: String) -> Bool {
        ["Complete", "Failed", "Cancelled"].contains(step)
    }

    private func providerLabel(_ provider: String) -> String {
        switch provider {
        case "hetzner": return "Hetzner Cloud"
        case "digitalocean": return "DigitalOcean"
        case "aws": return "AWS"
        case "manual": return "Existing server (SSH)"
        default: return provider.capitalized
        }
    }

    private func stepBadge(_ step: String) -> some View {
        let color: Color = isTerminal(step)
            ? (step == "Complete" ? Theme.statusSuccess : (step == "Failed" ? Theme.statusError : Color("TextSecondary")))
            : Theme.statusInfo
        return Text(stepLabel(step))
            .font(.system(size: 10, weight: .bold))
            .tracking(0.8)
            .foregroundColor(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(color.opacity(0.12))
            .clipShape(Capsule())
    }

    private func actionLabel(_ title: String, icon: String, color: Color? = nil, loading: Bool = false) -> some View {
        HStack(spacing: 4) {
            if loading {
                ProgressView()
                    .scaleEffect(0.6)
            } else {
                Image(systemName: icon)
                    .font(.system(size: 11, weight: .semibold))
            }
            Text(title)
                .font(.system(size: 12, weight: .semibold))
        }
        .foregroundColor(color ?? Color("AccentPrimary"))
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background((color ?? Color("AccentPrimary")).opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 14))
                .foregroundColor(Theme.statusError)
            Text(message)
                .font(.caption)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            Button(action: { actionError = nil }) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(Theme.statusError.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.statusError.opacity(0.3), lineWidth: 1)
        )
    }

    // MARK: - Actions

    private func advance() {
        actionError = nil
        Task {
            do {
                try await store.advanceActiveSession()
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func bootstrap() {
        actionError = nil
        Task {
            do {
                try await store.bootstrapActiveSession()
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func cancel() {
        actionError = nil
        Task {
            do {
                try await store.cancelActiveSession()
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func deleteSession(_ id: String) {
        actionError = nil
        Task {
            do {
                try await store.deleteSession(id)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }
}

// MARK: - Create sheet (manual-only phase 1)

struct CloudInstancesCreateSheet: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var store = CloudInstancesStore.shared

    @State private var instanceName = ""
    @State private var sshHost = ""
    @State private var sshPort = "22"
    @State private var sshUsername = "root"
    @State private var authType = "key"
    @State private var privateKey = ""
    @State private var password = ""

    private var isValid: Bool {
        !instanceName.trimmingCharacters(in: .whitespaces).isEmpty
            && !sshHost.trimmingCharacters(in: .whitespaces).isEmpty
            && (Int(sshPort) != nil)
            && !sshUsername.trimmingCharacters(in: .whitespaces).isEmpty
            && (authType == "key" ? !privateKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty : !password.isEmpty)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    Text("Allternit signs in over SSH once to validate access, then installs the runtime. Credentials are sent to the Allternit API and never shown again.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)

                    formField("Instance Name", text: $instanceName, placeholder: "my-server")
                    formField("SSH Host", text: $sshHost, placeholder: "203.0.113.10")
                    formField("SSH Port", text: $sshPort, placeholder: "22", keyboard: .numberPad)
                    formField("SSH Username", text: $sshUsername, placeholder: "root")

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Authentication")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Picker("Authentication", selection: $authType) {
                            Text("SSH Key").tag("key")
                            Text("Password").tag("password")
                        }
                        .pickerStyle(.menu)
                    }

                    if authType == "key" {
                        textArea("Private Key", text: $privateKey)
                    } else {
                        SecureField("Password", text: $password)
                            .font(.subheadline)
                            .padding(.horizontal, 12)
                            .frame(height: 44)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(Color("BorderSubtle"), lineWidth: 1)
                            )
                    }

                    if let error = store.startError {
                        Text(error)
                            .font(.caption)
                            .foregroundColor(Theme.statusError)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(20)
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Deploy to Existing Server")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Start") { start() }
                        .disabled(!isValid || store.isStarting)
                }
            }
        }
    }

    private func formField(_ label: String, text: Binding<String>, placeholder: String, keyboard: UIKeyboardType = .default) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            TextField(placeholder, text: text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(height: 44)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )
                .keyboardType(keyboard)
        }
    }

    private func textArea(_ label: String, text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            TextEditor(text: text)
                .font(.system(.body, design: .monospaced))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 120)
                .padding(8)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )
        }
    }

    private func start() {
        store.clearErrors()
        guard let port = Int(sshPort) else { return }
        let request = CloudWizardStartRequest(
            provider: "manual",
            sshHost: sshHost.trimmingCharacters(in: .whitespaces),
            sshPort: port,
            sshUsername: sshUsername.trimmingCharacters(in: .whitespaces),
            sshPrivateKey: authType == "key" ? privateKey.trimmingCharacters(in: .whitespacesAndNewlines) : nil,
            sshPassword: authType == "password" ? password : nil,
            instanceName: instanceName.trimmingCharacters(in: .whitespaces).presence
        )
        Task {
            do {
                try await store.startManualSession(request)
                await MainActor.run {
                    dismiss()
                }
            } catch {
                store.setStartError(error.localizedDescription)
            }
        }
    }
}

private extension String {
    var presence: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
