import SwiftUI

/// The old "Connect a computer or Allternit Cloud · Connect" strip is gone
/// (phone calm pass, contract C4). The chip above the composer
/// (`ComputerChipSlot`, Features/Shared/ComputerChip.swift) says it in Chat,
/// Bots and Code, and screens with no composer say it in their empty state
/// (`FriendlyStateView`, with a Connect action). The screens that still mount
/// this view (Code, Routines, Loops, Tasks) only get the data refresh: the
/// compute status, the computer list and the tunnel instances, so the chip
/// and the empty states read fresh values. It draws nothing.
struct ConnectComputerView: View {
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var instances = InstanceStore.shared
    @ObservedObject private var compute = ComputeStatusStore.shared

    var body: some View {
        Color.clear
            .frame(height: 0)
            .accessibilityHidden(true)
            .task {
                await compute.refreshIfNeeded()
                await relay.refresh()
                await instances.refreshIfNeeded()
            }
    }
}

/// The connect kit's computer picker (the chip's Connect / Fix, the + sheet,
/// the empty states' Connect): the account's runtime
/// devices (`GET /api/v1/runtime-devices`), tap one to relay through it, or
/// leave it on Automatic (the account's default computer, else the first
/// online one — `RuntimeRelayStore.pick`).
///
/// `showsPairPaths` (the Code sessions view's +, phone calm I9, A3): the top
/// section becomes `ComputerConnectPanel`, the two real ways a computer pairs
/// (Allternit Desktop sign-in, or `ao fabric pair` in a terminal). The list of
/// computers under it stays the same.
struct ConnectComputerSheet: View {
    var showsPairPaths: Bool = false
    var onConnected: ((String) -> Void)? = nil

    @ObservedObject private var relay = RuntimeRelayStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    if showsPairPaths {
                        ComputerConnectPanel(onConnected: onConnected)
                            .listRowBackground(Color("BgPanel"))
                    } else {
                        Text("Open Allternit Desktop on your Mac or PC and sign in with this account, or start your Allternit cloud computer. It shows up here when it's online.")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                            .listRowBackground(Color("BgPanel"))
                    }
                }

                Section("Your computers") {
                    // Only the first read shows Loading; the pair panel's re-reads
                    // every few seconds keep the list still.
                    if relay.isLoading && relay.devices.isEmpty && relay.availability == .unknown {
                        HStack {
                            ProgressView()
                            Text("Loading…").foregroundColor(Color("TextSecondary"))
                        }
                        .listRowBackground(Color("BgPanel"))
                    } else if let error = relay.lastError, relay.devices.isEmpty {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Couldn't load your computers")
                                .foregroundColor(Color("TextPrimary"))
                            Text(error)
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .listRowBackground(Color("BgPanel"))
                    } else if relay.devices.isEmpty {
                        Text("No computers yet.")
                            .foregroundColor(Color("TextSecondary"))
                            .listRowBackground(Color("BgPanel"))
                    } else {
                        automaticRow
                        ForEach(relay.devices) { device in
                            deviceRow(device)
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Connect a computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
                ToolbarItem(placement: .topBarLeading) {
                    Button(action: { Task { await relay.refresh() } }) {
                        Image(systemName: "arrow.clockwise")
                    }
                    .accessibilityLabel("Refresh")
                    .disabled(relay.isLoading)
                }
            }
            .refreshable { await relay.refresh() }
            .task { await relay.refresh() }
        }
    }

    private var automaticRow: some View {
        Button(action: { relay.select(nil) }) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Automatic")
                        .foregroundColor(Color("TextPrimary"))
                    Text(relay.activeDevice.map { "Using \($0.displayName)" } ?? "Your default computer, else the first online one")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer()
                if relay.selectedRuntimeId == nil {
                    Image(systemName: "checkmark").foregroundColor(Color("TextPrimary"))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(Color("BgPanel"))
    }

    private func deviceRow(_ device: RelayRuntimeDevice) -> some View {
        Button(action: { relay.select(device.id) }) {
            HStack(spacing: 10) {
                Circle()
                    .fill(device.isOnline ? Theme.statusSuccess : (device.isWakeable ? Theme.statusWarning : Color("TextSecondary").opacity(0.4)))
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(device.displayName)
                        .foregroundColor(Color("TextPrimary"))
                    Text(statusLine(device))
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer()
                if relay.selectedRuntimeId == device.id {
                    Image(systemName: "checkmark").foregroundColor(Color("TextPrimary"))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(Color("BgPanel"))
        .accessibilityLabel("\(device.displayName), \(statusLine(device))")
    }

    private func statusLine(_ device: RelayRuntimeDevice) -> String {
        let state: String
        if device.isOnline {
            state = "Online"
        } else if device.isWakeable {
            state = "Asleep · wakes when used"
        } else {
            state = "Offline"
        }
        var parts = [device.typeLabel, state]
        if device.isPreferred { parts.append("Default") }
        return parts.joined(separator: " · ")
    }
}

// MARK: - Pair paths (Code: "Connect a computer")

enum ComputerPairing {
    /// The command the terminal path types. Real: the node's own pairing (A3,
    /// web `FABRIC_PAIR_COMMAND`).
    static let fabricPairCommand = "ao fabric pair"
    /// Where Allternit Desktop downloads (the compute gate's default link).
    static let desktopDownloadURL = URL(string: "https://www.allternit.com/download")!
}

/// The two real ways a computer pairs with this account (web
/// `ComputerConnectPanel`):
/// - From Allternit Desktop: open it on the computer and sign in with this
///   account. Signing in pairs it; there is no setting to turn on.
/// - From a terminal: `ao fabric pair`, typed out once, with Copy.
/// Then it watches `RuntimeRelayStore`: "Connected" shows only when the
/// computer list gains an online computer, never from the animation.
struct ComputerConnectPanel: View {
    var onConnected: ((String) -> Void)? = nil

    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ReducedMotion private var reduced
    @State private var before: Set<String>? = nil
    @State private var typed = 0
    @State private var copied = false
    @State private var fired = false

    private var fresh: RelayRuntimeDevice? {
        newlyOnlineComputers(before: before, now: relay.devices, id: \.id, isOnline: \.isOnline).first
    }

    private var command: String { ComputerPairing.fabricPairCommand }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let fresh {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundColor(Theme.statusSuccess)
                    Text("Connected · \(fresh.displayName)")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .transition(reduced ? .opacity : .scale(scale: 0.9).combined(with: .opacity))
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.updatesFrequently)
            }

            VStack(alignment: .leading, spacing: 8) {
                Label("From Allternit Desktop", systemImage: "desktopcomputer")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                VStack(alignment: .leading, spacing: 8) {
                    Text("Open Allternit Desktop on your computer and sign in with this account. Signing in pairs it, so there is no code to type.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    Link(destination: ComputerPairing.desktopDownloadURL) {
                        HStack(spacing: 4) {
                            Text("Get Allternit Desktop")
                            Image(systemName: "arrow.up.right")
                        }
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            }

            Text("or")
                .font(.system(size: 11))
                .foregroundColor(Color("TextTertiary"))
                .frame(maxWidth: .infinity)

            VStack(alignment: .leading, spacing: 8) {
                Label("From a terminal", systemImage: "apple.terminal")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                terminalCard
                (Text("In a terminal on your computer, run ") + Text(command).font(.system(size: 12, design: .monospaced)).foregroundColor(Color("TextPrimary")))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity)
                    .multilineTextAlignment(.center)
            }

            if fresh == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for your computer…")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity)
            }
        }
        .padding(.vertical, 4)
        .animation(reduced ? Motion.fade : .spring(response: 0.3, dampingFraction: 0.8), value: fresh?.id)
        .task { await watch() }
        .task(id: reduced) { await typeCommand() }
        .onChange(of: fresh?.id) { _, id in
            guard id != nil, !fired, let fresh else { return }
            fired = true
            Haptics.success()
            onConnected?(fresh.displayName)
        }
    }

    /// The dark terminal: traffic lights, Copy, and `~ $ ao fabric pair`.
    private var terminalCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                ForEach(["#ff5f57", "#febc2e", "#28c840"], id: \.self) { hex in
                    Circle().fill(Color(hex: hex)).frame(width: 8, height: 8)
                }
                Spacer()
                Button(action: copy) {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .font(.system(size: 13))
                        .foregroundColor(TerminalPalette.foreground)
                        .frame(width: 44, height: 32)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(copied ? "Copied" : "Copy \(command)")
            }
            .accessibilityElement(children: .contain)
            HStack(spacing: 0) {
                Text("~ $ ").foregroundColor(TerminalPalette.secondary)
                Text(String(command.prefix(typed))).foregroundColor(Color(hex: "#E8A27C"))
                if typed < command.count {
                    Text("▌").foregroundColor(TerminalPalette.foreground).accessibilityHidden(true)
                }
            }
            .font(.system(size: 13, design: .monospaced))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(command)
        }
        .padding(12)
        .background(TerminalPalette.background, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private func copy() {
        UIPasteboard.general.string = command
        Haptics.light()
        copied = true
        Task {
            try? await Task.sleep(for: .seconds(1.6))
            copied = false
        }
    }

    /// Types the command once, a letter every 90 ms. Reduced motion: whole.
    @MainActor
    private func typeCommand() async {
        if reduced {
            typed = command.count
            return
        }
        typed = 0
        while typed < command.count, !Task.isCancelled {
            try? await Task.sleep(for: .milliseconds(90))
            typed += 1
        }
    }

    /// Snapshot what is online once the first list is read, then re-read the
    /// list every few seconds while the panel is up.
    @MainActor
    private func watch() async {
        await relay.refresh()
        if before == nil {
            before = Set(relay.devices.filter(\.isOnline).map(\.id))
        }
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            await relay.refresh()
        }
    }
}
