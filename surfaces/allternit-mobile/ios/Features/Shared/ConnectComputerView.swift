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
