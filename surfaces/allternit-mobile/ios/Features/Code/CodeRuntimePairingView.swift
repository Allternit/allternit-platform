import SwiftUI

// MARK: - Runtime pairing sheet

/// Pairing sheet behind the environment menu's "Pair Runtime…" entry. The
/// user never types a runtime id:
///
/// 1. "Choose a runtime" (default) — GET /api/v1/runtime-devices lists the
///    account's paired devices (name, type, online status, last seen);
///    tapping a row pairs it via EnvironmentStore and dismisses.
/// 2. "Have a pairing code?" — the XXXX-XXXX code a NEW device prints
///    (`gizzi pair`, a VPS first boot): GET /api/v1/runtime-pairings/code/:code
///    shows the device, POST .../approve connects it (the web/desktop
///    DevicePairingPanel flow), then the sheet auto-pairs the device once it
///    checks in to the runtime list.
///
/// Manual runtime-id entry remains as a collapsed "Advanced" fallback.
struct RuntimePairingView: View {
    @StateObject private var environmentStore = EnvironmentStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var runtimes: [RuntimeDevice] = []
    @State private var isLoadingRuntimes = false
    @State private var runtimesError: String? = nil

    @State private var pairingCode = ""
    @State private var pairingInfo: RuntimePairingInfo? = nil
    @State private var isLookingUp = false
    @State private var isApproving = false
    @State private var isDenying = false
    @State private var codeError: String? = nil
    @State private var approvedName: String? = nil

    @State private var manualRuntimeId = ""

    private let client = RuntimeDevicesClient()

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    if let pairedRuntimeId = environmentStore.pairedRuntimeId {
                        pairedCard(pairedRuntimeId)
                    }
                    chooseCard
                    codeCard
                    advancedCard
                }
                .padding(20)
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Pair Runtime")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
            .task {
                await loadRuntimes()
            }
            .refreshable {
                await loadRuntimes()
            }
        }
    }

    private func pairedCard(_ runtimeId: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(Theme.statusSuccess)

            VStack(alignment: .leading, spacing: 2) {
                Text("Paired runtime")
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                Text(runtimeId)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer()

            Button("Unpair") {
                environmentStore.unpair()
            }
            .font(.subheadline)
            .foregroundColor(Theme.statusError)
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
    }

    // MARK: - Choose a runtime

    /// The default path: the account's paired devices from
    /// GET /api/v1/runtime-devices. Tap pairs + dismisses; loading, error
    /// (with retry), and empty states mirror the session list's.
    private var chooseCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Choose a runtime")
                .font(.subheadline)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextPrimary"))

            Text("Cloud requests relay through the runtime you pick. Pair a new device with a code below.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            if isLoadingRuntimes && runtimes.isEmpty {
                HStack {
                    Spacer()
                    ProgressView()
                        .padding(.vertical, 12)
                    Spacer()
                }
            } else if let runtimesError, runtimes.isEmpty {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "wifi.slash",
                    title: "Couldn't load runtimes",
                    message: FriendlyErrorMessage.from(runtimesError),
                    actionTitle: "Retry",
                    action: { Task { await loadRuntimes() } }
                )
            } else if runtimes.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "server.rack",
                    title: "No runtimes yet",
                    message: "Pair one with a code below."
                )
            } else {
                VStack(spacing: 0) {
                    ForEach(runtimes) { runtime in
                        runtimeRow(runtime)
                    }
                }
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
    }

    /// One runtime row: status dot, type icon, name + type · last-seen
    /// subtitle, checkmark on the currently paired one.
    private func runtimeRow(_ runtime: RuntimeDevice) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            environmentStore.pair(withRuntimeId: runtime.id)
            dismiss()
        }) {
            HStack(spacing: 12) {
                Image(systemName: runtime.runtimeType == "vps" || runtime.runtimeType == "hosted" ? "server.rack" : "desktopcomputer")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text(runtime.name)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)

                    Text("\(runtime.typeLabel) · \(Self.lastSeenLabel(for: runtime))")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }

                Spacer()

                Circle()
                    .fill(runtime.isOnline ? Theme.statusSuccess : Color("TextSecondary"))
                    .frame(width: 8, height: 8)

                Image(systemName: "checkmark")
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                    .opacity(environmentStore.pairedRuntimeId == runtime.id ? 1 : 0)
                    .frame(width: 20)
            }
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Pairing code

    /// The new-device path: the XXXX-XXXX code a runtime prints when it asks
    /// to pair (`gizzi pair`, a VPS first boot). Continue looks the code up
    /// and swaps in the device details with Approve / Back.
    private var codeCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Have a pairing code?")
                .font(.subheadline)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextPrimary"))

            Text("Enter the code a runtime prints when it asks to connect — e.g. `gizzi pair` output or a VPS first boot.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            if let approvedName {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundColor(Theme.statusSuccess)
                    Text("\(approvedName) is connected to your Allternit account — pick it from the list above once it checks in.")
                        .font(.caption)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if let pairingInfo {
                pairingDetail(pairingInfo)
            } else {
                HStack(spacing: 10) {
                    TextField("ABCD-2345", text: $pairingCode)
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .padding(.horizontal, 12)
                        .padding(.vertical, 9)
                        .background(Color("BgTertiary"))
                        .cornerRadius(10)
                        .onChange(of: pairingCode) { _, newValue in
                            let normalized = Self.normalizeCode(newValue)
                            if normalized != newValue {
                                pairingCode = normalized
                            }
                            codeError = nil
                        }

                    Button(action: lookup) {
                        HStack(spacing: 8) {
                            if isLookingUp {
                                ProgressView()
                                    .scaleEffect(0.8)
                            }
                            Text(isLookingUp ? "Checking…" : "Continue")
                        }
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("OnAccent"))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 9)
                        .background(Color("AccentPrimary"))
                        .cornerRadius(10)
                    }
                    .disabled(pairingCode.count != 9 || isLookingUp)
                    .opacity(pairingCode.count != 9 || isLookingUp ? 0.5 : 1)
                }
            }

            if let codeError {
                Text(codeError)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
    }

    /// The looked-up pairing request (DevicePairingPanel.tsx:343-433): device
    /// name, type · code subtitle, optional host/system rows, Approve + Back.
    private func pairingDetail(_ pairing: RuntimePairingInfo) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Image(systemName: pairing.runtimeType == "vps" || pairing.runtimeType == "hosted" ? "server.rack" : "desktopcomputer")
                    .font(.title3)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Color("BgTertiary"))
                    .clipShape(RoundedRectangle(cornerRadius: 10))

                VStack(alignment: .leading, spacing: 2) {
                    Text("Connect \(pairing.name)?")
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextPrimary"))
                    Text("\(RuntimeDevice.typeLabel(for: pairing.runtimeType)) · Code \(pairing.userCode)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            if let hostname = pairing.hostname, !hostname.isEmpty {
                detailRow("Host", hostname)
            }
            if let platform = pairing.platform, !platform.isEmpty {
                detailRow("System", platform)
            }

            HStack(spacing: 10) {
                Button(action: approve) {
                    HStack(spacing: 8) {
                        if isApproving {
                            ProgressView()
                                .scaleEffect(0.8)
                        }
                        Text(isApproving ? "Connecting…" : "Approve")
                    }
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("OnAccent"))
                    .padding(.horizontal, 14)
                    .padding(.vertical, 9)
                    .background(Color("AccentPrimary"))
                    .cornerRadius(10)
                }
                .disabled(isApproving || isDenying)
                .opacity(isApproving ? 0.5 : 1)

                Button(action: deny) {
                    Text(isDenying ? "Denying…" : "Deny")
                        .font(.subheadline)
                        .fontWeight(.semibold)
                }
                .foregroundColor(Theme.statusError)
                .disabled(isApproving || isDenying)
                .opacity(isDenying ? 0.5 : 1)

                Button("Back") {
                    pairingInfo = nil
                    codeError = nil
                }
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .disabled(isApproving || isDenying)
            }
        }
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            Text(value)
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    // MARK: - Advanced (manual id)

    /// Manual runtime-id entry, collapsed by default — the fallback for when
    /// neither the list nor a pairing code applies.
    private var advancedCard: some View {
        DisclosureGroup {
            VStack(alignment: .leading, spacing: 10) {
                Text("The id listed under Settings → Runtime devices on the web or desktop. Cloud requests then relay through this runtime.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)

                TextField("Runtime device ID", text: $manualRuntimeId)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .background(Color("BgTertiary"))
                    .cornerRadius(10)

                Button(action: saveManual) {
                    Text("Save Runtime ID")
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("OnAccent"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 10)
                        .background(Color("AccentPrimary"))
                        .cornerRadius(10)
                }
                .disabled(manualRuntimeId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .opacity(manualRuntimeId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? 0.5 : 1)
            }
            .padding(.top, 10)
        } label: {
            Text("Advanced: enter a runtime ID")
                .font(.subheadline)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextSecondary"))
        }
        .tint(Color("TextSecondary"))
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
    }

    // MARK: - Actions & loading

    @MainActor
    private func loadRuntimes() async {
        if runtimes.isEmpty {
            isLoadingRuntimes = true
        }
        runtimesError = nil

        do {
            runtimes = try await client.fetchRuntimes()
        } catch is CancellationError {
            // View disappeared mid-flight — keep the current state.
        } catch {
            runtimesError = error.localizedDescription
        }
        isLoadingRuntimes = false
    }

    /// GET /api/v1/runtime-pairings/code/:code. Success swaps the code field
    /// for the device details; any failure is shown inline.
    private func lookup() {
        let code = pairingCode
        guard code.count == 9, !isLookingUp else { return }

        isLookingUp = true
        codeError = nil

        Task {
            do {
                pairingInfo = try await client.lookupPairing(code: code)
            } catch {
                pairingInfo = nil
                codeError = error.localizedDescription
            }
            isLookingUp = false
        }
    }

    /// POST /api/v1/runtime-pairings/code/:code/deny (DevicePairingPanel.tsx
    /// denyPairing, runtime_pairing.rs deny_pairing) — marks the request
    /// denied and returns to the code-entry state; nothing to auto-pair.
    private func deny() {
        guard let pairing = pairingInfo, !isDenying, !isApproving else { return }

        isDenying = true
        codeError = nil

        Task {
            do {
                try await client.denyPairing(code: pairing.userCode)
                pairingInfo = nil
                pairingCode = ""
            } catch {
                codeError = error.localizedDescription
            }
            isDenying = false
        }
    }

    /// POST /api/v1/runtime-pairings/code/:code/approve, then auto-pair the
    /// device once it checks in to the runtime list; when it hasn't yet, the
    /// success message points at the (refreshed) list instead.
    private func approve() {
        guard let pairing = pairingInfo, !isApproving else { return }

        isApproving = true
        codeError = nil

        Task {
            do {
                let name = try await client.approvePairing(code: pairing.userCode) ?? pairing.name
                pairingInfo = nil
                pairingCode = ""
                await loadRuntimes()
                if await autoPairApprovedRuntime(named: name) {
                    isApproving = false
                    return
                }
                approvedName = name
            } catch {
                codeError = error.localizedDescription
            }
            isApproving = false
        }
    }

    /// The approve response carries no device id (the device registers itself
    /// when it next polls the pairing status), so poll the runtime list
    /// briefly for a device with the approved name and pair it directly.
    /// Returns true when paired (the sheet dismisses); false leaves the
    /// success message + refreshed list for the user to tap.
    @MainActor
    private func autoPairApprovedRuntime(named name: String) async -> Bool {
        for attempt in 0..<4 {
            if attempt > 0 {
                try? await Task.sleep(for: .seconds(1.5))
                await loadRuntimes()
            }
            if Task.isCancelled { return false }
            if let device = runtimes.first(where: { $0.name == name }) {
                environmentStore.pair(withRuntimeId: device.id)
                dismiss()
                return true
            }
        }
        return false
    }

    private func saveManual() {
        let runtimeId = manualRuntimeId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !runtimeId.isEmpty else { return }
        environmentStore.pair(withRuntimeId: runtimeId)
        dismiss()
    }

    // MARK: - Formatting

    /// Same normalization as the web panel (DevicePairingPanel.tsx:64-67):
    /// uppercase A-Z2-9, 8 chars, dashed XXXX-XXXX.
    private static func normalizeCode(_ value: String) -> String {
        let allowed = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZ23456789")
        let compact = String(value.uppercased().filter { allowed.contains($0) }.prefix(8))
        return compact.count > 4 ? "\(compact.prefix(4))-\(compact.suffix(compact.count - 4))" : compact
    }

    private static func lastSeenLabel(for runtime: RuntimeDevice) -> String {
        guard let raw = runtime.lastSeenAt, let date = parseTimestamp(raw) else {
            return "Last seen never"
        }
        // Built per call: a shared formatter static would be non-Sendable
        // state under Swift 6.
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return "Last seen \(formatter.localizedString(for: date, relativeTo: Date()))"
    }

    /// Backend timestamps are RFC-3339 (chrono `to_rfc3339`), with or without
    /// fractional seconds. Mirrored from CodeModeView's private copy.
    private static func parseTimestamp(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
    }
}
