import SwiftUI

/// One plain "no computer online" state for the surfaces that run on the
/// user's computer (Code, Automation) — the native twin of the PWA's
/// `PhoneNoComputer` (allternit-ai src/fabric-session/phone/PhoneNoComputer.tsx):
///
/// - No computer paired → "Connect a computer or Allternit Cloud" · Connect
/// - Paired but none answers → "Can't reach your computer" · Fix
/// - A sleeping cloud computer waking up → "Waking your computer…"
/// - Reachable (relay, mesh or a registered tunnel instance) → nothing.
///
/// Tapping the action opens `ConnectComputerSheet`, the computer picker.
struct ConnectComputerView: View {
    /// The shell's own copy, above every section (PhoneShell `phone-main`).
    /// Section copies stand down while the shell shows it.
    var isShellBanner = false

    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var instances = InstanceStore.shared
    @Environment(\.shellShowsNoComputer) private var shellShowsNoComputer
    @State private var isSheetPresented = false

    var body: some View {
        Group {
            if shellShowsNoComputer && !isShellBanner {
                EmptyView()
            } else if relay.isWaking {
                row(text: "Waking your computer…", action: nil, showsProgress: true)
            } else if shouldShow {
                row(
                    text: isPaired ? "Can't reach your computer" : "Connect a computer or Allternit Cloud",
                    action: isPaired ? "Fix" : "Connect",
                    showsProgress: false
                )
            }
        }
        .task {
            await relay.refresh()
            await instances.refreshIfNeeded()
        }
        .sheet(isPresented: $isSheetPresented) {
            ConnectComputerSheet()
        }
    }

    /// A paired computer exists (online or not).
    private var isPaired: Bool { !relay.devices.isEmpty }

    private var shouldShow: Bool {
        #if DEBUG
        if UserDefaults.standard.string(forKey: "gizzi-url") != nil { return false }
        #endif
        // A registered `gizzi serve --tunnel` instance that is online is a
        // direct target of its own (InstanceConnection.resolve).
        if instances.instances.contains(where: \.isOnline) { return false }
        switch relay.availability {
        case .unknown, .available: return false
        case .unreachable, .none: return true
        }
    }

    /// `phone-no-computer`: a rounded neutral strip, 8pt under the top and
    /// 16pt in from the sides, 14pt text and a plain semibold action.
    private func row(text: String, action: String?, showsProgress: Bool) -> some View {
        HStack(spacing: 12) {
            if showsProgress {
                ProgressView().controlSize(.small)
            }
            Text(text)
                .font(.system(size: 14))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
            if let action {
                Button(action: { isSheetPresented = true }) {
                    Text(action)
                        .font(.system(size: 14, weight: .semibold))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundColor(Color("TextPrimary"))
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isStaticText)
    }
}

extension EnvironmentValues {
    /// True under MainWorkspaceView, which shows the one no-computer banner
    /// above every section, as the web phone shell does.
    @Entry var shellShowsNoComputer: Bool = false
}

/// The computer picker behind `ConnectComputerView`: the account's runtime
/// devices (`GET /api/v1/runtime-devices`), tap one to relay through it, or
/// leave it on Automatic (the account's default computer, else the first
/// online one — `RuntimeRelayStore.pick`).
struct ConnectComputerSheet: View {
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text("Open Allternit Desktop on your Mac or PC and sign in with this account, or start your Allternit cloud computer. It shows up here when it's online.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .listRowBackground(Color("BgPanel"))
                }

                Section("Your computers") {
                    if relay.isLoading && relay.devices.isEmpty {
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
