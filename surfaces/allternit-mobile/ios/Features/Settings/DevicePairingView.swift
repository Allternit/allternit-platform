import SwiftUI

/// `GET /api/v1/runtime-pairings/code/:code` (cloud-api runtime_pairing.rs).
struct DevicePairingRequest: Decodable, Equatable, Sendable {
    let pairingId: String
    let userCode: String
    let name: String
    let runtimeType: String
    let hostname: String?
    let platform: String?
    let publicKeyFingerprint: String
    let capabilities: [String]
    let status: String
    let expiresAt: String
}

/// One row of `GET /api/v1/runtime-devices` → `{ runtimes: [...] }`.
struct RuntimeDeviceSummary: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let runtimeType: String
    let hostname: String?
    let platform: String?
    let version: String?
    let status: String
    let lastSeenAt: String?
    let createdAt: String
}

private struct RuntimeDevicesEnvelope: Decodable, Sendable {
    let runtimes: [RuntimeDeviceSummary]?
}

/// `GET /api/v1/runtime-presence` (runtime heartbeats every 60 s).
struct RuntimePresenceRow: Decodable, Sendable {
    let runtimeId: String
    let online: Bool
    let lastSeen: String?
}

private struct EmptyBody: Encodable {}

/// Settings → Computers → Devices (the PWA's DevicePairingPanel): approve a
/// machine that ran `gizzi pair` by typing its code, and review or remove the
/// paired devices. Talks to cloud-api with the signed-in session.
struct DevicePairingView: View {
    @State private var code = ""
    @State private var pairing: DevicePairingRequest? = nil
    @State private var isLookingUp = false
    @State private var acting: String? = nil
    @State private var pairingError: String? = nil
    @State private var approvedName: String? = nil

    @State private var devices: [RuntimeDeviceSummary] = []
    @State private var presence: [String: RuntimePresenceRow] = [:]
    @State private var isLoadingDevices = false
    @State private var devicesError: String? = nil
    @State private var removeTarget: RuntimeDeviceSummary? = nil

    var body: some View {
        List {
            approveSection
            devicesSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Devices")
        .navigationBarTitleDisplayMode(.inline)
        .task { await loadDevices() }
        .refreshable { await loadDevices() }
        .confirmationDialog(
            "Remove \(removeTarget?.name ?? "device")?",
            isPresented: Binding(get: { removeTarget != nil }, set: { if !$0 { removeTarget = nil } }),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                if let target = removeTarget { Task { await revoke(target) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("It stops reaching your Allternit account until you pair it again.")
        }
    }

    // MARK: - Approve

    @ViewBuilder
    private var approveSection: some View {
        Section {
            if let approvedName {
                Label("\(approvedName) is now connected to your account.", systemImage: "checkmark.circle.fill")
                    .font(.subheadline)
                    .foregroundColor(Theme.statusSuccess)
            }
            HStack {
                TextField("XXXX-XXXX", text: $code)
                    .font(.system(.body, design: .monospaced))
                    .textInputAutocapitalization(.characters)
                    .autocorrectionDisabled()
                    .onChange(of: code) { _, newValue in
                        let normalized = Self.normalize(newValue)
                        if normalized != newValue { code = normalized }
                        if normalized.count == 9 { Task { await lookup(normalized) } }
                    }
                    .accessibilityLabel("Pairing code")
                if isLookingUp { ProgressView() }
            }
            if let pairing {
                VStack(alignment: .leading, spacing: 4) {
                    Text(pairing.name)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text([pairing.runtimeType, pairing.hostname, pairing.platform].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Text("Key \(pairing.publicKeyFingerprint)")
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                HStack {
                    Button(action: { Task { await approve(pairing) } }) {
                        Text(acting == "approve" ? "Approving…" : "Approve")
                            .font(.subheadline.weight(.semibold))
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Color("AccentPrimary"))
                    Button(action: { Task { await deny(pairing) } }) {
                        Text(acting == "deny" ? "Denying…" : "Deny")
                            .font(.subheadline)
                    }
                    .buttonStyle(.bordered)
                }
                .disabled(acting != nil)
            }
            if let pairingError {
                Text(pairingError)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        } header: {
            Text("Approve a device")
        } footer: {
            Text("Run `gizzi pair` on the machine and type the code it shows.")
        }
    }

    // MARK: - Devices

    @ViewBuilder
    private var devicesSection: some View {
        Section {
            if isLoadingDevices && devices.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let devicesError, devices.isEmpty {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't load devices",
                    message: FriendlyErrorMessage.from(devicesError),
                    actionTitle: "Try again",
                    action: { Task { await loadDevices() } }
                )
            } else if devices.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "laptopcomputer",
                    title: "No paired devices",
                    message: "Install Allternit Desktop or run `gizzi pair` on a computer to connect it."
                )
            } else {
                ForEach(devices) { device in
                    deviceRow(device)
                }
            }
        } header: {
            Text("Paired devices")
        }
    }

    private func deviceRow(_ device: RuntimeDeviceSummary) -> some View {
        let state = presenceLabel(for: device)
        return HStack(spacing: 12) {
            Image(systemName: device.runtimeType.contains("ios") ? "iphone" : "laptopcomputer")
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(device.name)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Text([device.platform, device.version.map { "v\($0)" }].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            HStack(spacing: 4) {
                Circle().fill(state.color).frame(width: 7, height: 7)
                Text(state.text)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Button(action: { removeTarget = device }) {
                Image(systemName: "xmark.circle")
                    .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(device.name)")
        }
    }

    private func presenceLabel(for device: RuntimeDeviceSummary) -> (text: String, color: Color) {
        if device.status == "revoked" { return ("Removed", Color("TextSecondary")) }
        guard let row = presence[device.id] else { return ("Unknown", Color("TextSecondary")) }
        if row.online { return ("Online", Theme.statusSuccess) }
        if let raw = row.lastSeen, let seen = Self.parseDate(raw), Date().timeIntervalSince(seen) < 86_400 {
            return ("Asleep", Theme.statusWarning)
        }
        return ("Offline", Color("TextSecondary"))
    }

    // MARK: - Calls

    @MainActor
    private func loadDevices() async {
        isLoadingDevices = true
        devicesError = nil
        defer { isLoadingDevices = false }
        do {
            let envelope: RuntimeDevicesEnvelope = try await APIClient.shared.get(path: "runtime-devices")
            devices = (envelope.runtimes ?? []).filter { $0.status != "revoked" }
        } catch {
            devicesError = error.localizedDescription
        }
        // Presence is best effort: unknown, never "offline", on failure.
        if let rows: [RuntimePresenceRow] = try? await APIClient.shared.get(path: "runtime-presence") {
            presence = Dictionary(rows.map { ($0.runtimeId, $0) }, uniquingKeysWith: { a, _ in a })
        }
    }

    @MainActor
    private func lookup(_ normalized: String) async {
        isLookingUp = true
        pairingError = nil
        approvedName = nil
        defer { isLookingUp = false }
        do {
            pairing = try await APIClient.shared.get(path: "runtime-pairings/code/\(Self.escape(normalized))")
        } catch {
            pairing = nil
            pairingError = "This pairing code is invalid or expired."
        }
    }

    @MainActor
    private func approve(_ info: DevicePairingRequest) async {
        acting = "approve"
        pairingError = nil
        defer { acting = nil }
        do {
            try await APIClient.shared.post(path: "runtime-pairings/code/\(Self.escape(info.userCode))/approve", body: EmptyBody())
            approvedName = info.name
            pairing = nil
            code = ""
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            await loadDevices()
        } catch {
            pairingError = "Couldn't approve this device. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    @MainActor
    private func deny(_ info: DevicePairingRequest) async {
        acting = "deny"
        pairingError = nil
        defer { acting = nil }
        do {
            try await APIClient.shared.post(path: "runtime-pairings/code/\(Self.escape(info.userCode))/deny")
            pairing = nil
            code = ""
        } catch {
            pairingError = "Couldn't deny this device. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    @MainActor
    private func revoke(_ device: RuntimeDeviceSummary) async {
        removeTarget = nil
        do {
            try await APIClient.shared.delete(path: "runtime-devices/\(Self.escape(device.id))")
            devices.removeAll { $0.id == device.id }
        } catch {
            devicesError = error.localizedDescription
        }
    }

    // MARK: - Helpers

    /// Same normalization as the web: XXXX-XXXX, A–Z and 2–9.
    static func normalize(_ value: String) -> String {
        let allowed = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZ23456789")
        let compact = String(value.uppercased().filter { allowed.contains($0) }.prefix(8))
        guard compact.count > 4 else { return compact }
        return "\(compact.prefix(4))-\(compact.dropFirst(4))"
    }

    private static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? value
    }

    static func parseDate(_ raw: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: raw) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: raw)
    }
}
