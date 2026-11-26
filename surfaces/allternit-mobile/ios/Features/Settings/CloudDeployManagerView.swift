import SwiftUI

/// Phase-1 Cloud Deploy manager for iOS.
///
/// Mirrors the web's CloudDeployView wizard flow at a surface level: list
/// deployments, create a manual deployment via a form, and poll status.
/// Agent-assisted signup and WebSocket events are deferred.
struct CloudDeployManagerView: View {
    @StateObject private var store = CloudDeployStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var isCreateSheetPresented = false
    @State private var cancelError: String? = nil

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .task {
            store.fetchIfNeeded()
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CloudDeployCreateSheet()
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Cloud Deploy")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Text("Provision Allternit on cloud infrastructure")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            Button(action: { dismiss() }) {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.deployments.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.deployments.isEmpty {
            Spacer()
            let offline = isConnectionFailure(loadError)
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: "Failed to load deployments",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else {
            ScrollView {
                VStack(spacing: 16) {
                    actionBar
                    statCards
                    if let cancelError {
                        errorBanner(cancelError)
                    }
                    deploymentList
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
            .refreshable {
                await store.refresh()
            }
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
                    Text("New Deployment")
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

    // MARK: - Stat cards

    private var statCards: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
            statCard(label: "Total", value: "\(store.deployments.count)", icon: "rocket", color: Color("AccentPrimary"))
            statCard(label: "Running", value: "\(store.deployments.filter { $0.status == "deploying" }.count)", icon: "bolt", color: Theme.statusWarning)
            statCard(label: "Complete", value: "\(store.deployments.filter { $0.status == "complete" }.count)", icon: "checkmark.circle", color: Theme.statusSuccess)
            statCard(label: "Error", value: "\(store.deployments.filter { $0.status == "error" }.count)", icon: "exclamationmark.triangle", color: Theme.statusError)
        }
    }

    private func statCard(label: String, value: String, icon: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Image(systemName: icon)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundColor(color)
                Spacer()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(label.uppercased())
                    .font(.system(size: 9, weight: .semibold))
                    .tracking(1)
                    .foregroundColor(Color("TextSecondary"))
                Text(value)
                    .font(.system(size: 22, weight: .heavy, design: .rounded))
                    .foregroundColor(Color("TextPrimary"))
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

    // MARK: - Deployment list

    @ViewBuilder
    private var deploymentList: some View {
        if store.deployments.isEmpty {
            emptyState
        } else {
            VStack(spacing: 12) {
                ForEach(store.deployments) { deployment in
                    deploymentRow(deployment)
                }
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "rocket",
            title: "No deployments yet",
            message: "Provision a new cloud instance to run Allternit.",
            actionTitle: "New Deployment",
            action: { isCreateSheetPresented = true },
            art: .log,
            size: .panel
        )
        .padding(.vertical, 24)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func deploymentRow(_ deployment: Deployment) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(deployment.instanceName)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("\(deployment.providerId) / \(deployment.regionId) / \(deployment.instanceTypeId)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                statusBadge(deployment.status)
            }

            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text("Progress")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Text("\(deployment.progress)%")
                        .font(.caption)
                        .foregroundColor(Color("TextPrimary"))
                }
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 4)
                            .fill(Color("BgSecondary"))
                        RoundedRectangle(cornerRadius: 4)
                            .fill(progressColor(deployment.status))
                            .frame(width: max(0, geo.size.width * CGFloat(deployment.progress) / 100))
                    }
                }
                .frame(height: 6)
            }

            Text(deployment.message)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)

            if let ip = deployment.instanceIp {
                HStack {
                    Text("IP: \(ip)")
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                }
            }

            if deployment.status == "deploying" {
                HStack {
                    Spacer()
                    Button(action: { cancel(deployment.id) }) {
                        HStack(spacing: 4) {
                            if store.isCancellingId == deployment.id {
                                ProgressView()
                                    .scaleEffect(0.6)
                            } else {
                                Image(systemName: "xmark")
                                    .font(.system(size: 11, weight: .semibold))
                            }
                            Text("Cancel")
                                .font(.system(size: 12, weight: .semibold))
                        }
                        .foregroundColor(Theme.statusError)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 6)
                        .background(Theme.statusError.opacity(0.08))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isCancellingId == deployment.id)
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

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private func statusBadge(_ status: String) -> some View {
        let (label, color) = statusInfo(status)
        return Text(label)
            .font(.system(size: 10, weight: .bold))
            .tracking(0.8)
            .foregroundColor(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(color.opacity(0.12))
            .clipShape(Capsule())
    }

    private func statusInfo(_ status: String) -> (label: String, color: Color) {
        switch status.lowercased() {
        case "complete": return ("Complete", Theme.statusSuccess)
        case "deploying": return ("Deploying", Theme.statusWarning)
        case "error": return ("Error", Theme.statusError)
        case "cancelled": return ("Cancelled", Color("TextSecondary"))
        default: return (status.capitalized, Theme.statusInfo)
        }
    }

    private func progressColor(_ status: String) -> Color {
        switch status.lowercased() {
        case "complete": return Theme.statusSuccess
        case "error", "cancelled": return Theme.statusError
        case "deploying": return Theme.statusWarning
        default: return Theme.statusInfo
        }
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
            Button(action: { cancelError = nil }) {
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

    private func cancel(_ id: String) {
        cancelError = nil
        Task {
            do {
                try await store.cancelDeployment(id)
            } catch {
                cancelError = error.localizedDescription
            }
        }
    }
}

// MARK: - Create sheet

struct CloudDeployCreateSheet: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var store = CloudDeployStore.shared

    @State private var providerId = ""
    @State private var regionId = ""
    @State private var instanceTypeId = ""
    @State private var storageGb = "20"
    @State private var instanceName = ""
    @State private var mode = "manual"
    @State private var apiToken = ""
    @State private var sshHost = ""
    @State private var sshPort = "22"
    @State private var sshUsername = ""
    @State private var sshPrivateKey = ""

    private var isValid: Bool {
        !providerId.trimmingCharacters(in: .whitespaces).isEmpty
            && !regionId.trimmingCharacters(in: .whitespaces).isEmpty
            && !instanceTypeId.trimmingCharacters(in: .whitespaces).isEmpty
            && !instanceName.trimmingCharacters(in: .whitespaces).isEmpty
            && (Int(storageGb) != nil)
            && (mode == "automated" ? !apiToken.trimmingCharacters(in: .whitespaces).isEmpty : true)
            && (mode == "manual"
                ? !sshHost.trimmingCharacters(in: .whitespaces).isEmpty
                    && !sshUsername.trimmingCharacters(in: .whitespaces).isEmpty
                    && !sshPrivateKey.trimmingCharacters(in: .whitespaces).isEmpty
                : true)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 16) {
                    formField("Provider", text: $providerId, placeholder: "hetzner")
                    formField("Region", text: $regionId, placeholder: "fsn1")
                    formField("Instance Type", text: $instanceTypeId, placeholder: "cx21")
                    formField("Storage (GB)", text: $storageGb, placeholder: "20", keyboard: .numberPad)
                    formField("Instance Name", text: $instanceName, placeholder: "allternit-node-1")

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Mode")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Picker("Mode", selection: $mode) {
                            Text("Manual (existing VPS)").tag("manual")
                            Text("Automated (API token)").tag("automated")
                        }
                        .pickerStyle(.menu)
                    }

                    if mode == "automated" {
                        formField("API Token", text: $apiToken, placeholder: "provider API token")
                    } else {
                        formField("SSH Host", text: $sshHost, placeholder: "203.0.113.10")
                        formField("SSH Port", text: $sshPort, placeholder: "22", keyboard: .numberPad)
                        formField("SSH Username", text: $sshUsername, placeholder: "root")
                        textArea("SSH Private Key", text: $sshPrivateKey)
                    }

                    if let error = store.createError {
                        Text(error)
                            .font(.caption)
                            .foregroundColor(Theme.statusError)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .padding(20)
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("New Deployment")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Deploy") { create() }
                        .disabled(!isValid || store.isCreating)
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

    private func create() {
        store.setCreateError(nil)
        guard let storage = Int(storageGb) else { return }
        let request = DeploymentCreateRequest(
            providerId: providerId.trimmingCharacters(in: .whitespaces),
            regionId: regionId.trimmingCharacters(in: .whitespaces),
            instanceTypeId: instanceTypeId.trimmingCharacters(in: .whitespaces),
            storageGb: storage,
            instanceName: instanceName.trimmingCharacters(in: .whitespaces),
            mode: mode,
            apiToken: mode == "automated" ? apiToken.trimmingCharacters(in: .whitespaces) : nil,
            sshHost: mode == "manual" ? sshHost.trimmingCharacters(in: .whitespaces) : nil,
            sshPort: mode == "manual" ? Int(sshPort) : nil,
            sshUsername: mode == "manual" ? sshUsername.trimmingCharacters(in: .whitespaces) : nil,
            sshPrivateKey: mode == "manual" ? sshPrivateKey.trimmingCharacters(in: .whitespacesAndNewlines) : nil
        )
        Task {
            do {
                try await store.createDeployment(request)
                await MainActor.run {
                    dismiss()
                }
            } catch {
                store.setCreateError(error.localizedDescription)
            }
        }
    }
}
