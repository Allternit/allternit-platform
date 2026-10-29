import SwiftUI
import UIKit

/// Settings → Phones: "Let your bots use this iPhone". Pick a Desktop, see
/// the same 6-digit code here and on the Desktop, approve there. Once
/// paired: share the screen, see whether bots can reach the app, unpair.
struct AppPhonesSettingsView: View {
    @ObservedObject private var phone = AppPhoneService.shared

    @State private var runtimes: [RuntimeDevice] = []
    @State private var isLoading = false
    @State private var loadError: String?
    @State private var phoneName = UIDevice.current.name
    @State private var isUnpairConfirmPresented = false

    var body: some View {
        List {
            if let credentials = phone.credentials {
                pairedSection(credentials)
                screenSection
                unpairSection
            } else {
                switch phone.pairStep {
                case .registering, .waitingForDesktop:
                    pairingSection
                default:
                    introSection
                    desktopsSection
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Phones")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            if phone.credentials == nil { await loadRuntimes() }
        }
        .refreshable { await loadRuntimes() }
        .confirmationDialog("Unpair this iPhone?", isPresented: $isUnpairConfirmPresented, titleVisibility: .visible) {
            Button("Unpair", role: .destructive) {
                Task {
                    await phone.unpair()
                    await loadRuntimes()
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Bots on \(phone.credentials?.runtimeName ?? "your Desktop") won't be able to see this screen or use the ACI browser until you pair again.")
        }
    }

    // MARK: - Not paired

    private var introSection: some View {
        Section {
            TextField("Name", text: $phoneName)
                .font(.subheadline)
                .textInputAutocapitalization(.words)
        } header: {
            Text("Let your bots use this iPhone")
        } footer: {
            Text("Pair this iPhone with one of your Desktops. Your bots can then see the screen while you share it and use the ACI browser in this app. Taps on other apps aren't supported.")
        }
    }

    @ViewBuilder
    private var desktopsSection: some View {
        Section {
            if isLoading && runtimes.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading your Desktops…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else if let loadError {
                Text(loadError)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                Button("Try again") { Task { await loadRuntimes() } }
                    .font(.subheadline)
            } else if desktops.isEmpty {
                Text("No Desktop is paired with your account yet. Open Allternit Desktop and sign in, then pull to refresh.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(desktops) { runtime in
                    Button(action: { phone.pair(with: runtime, name: trimmedName) }) {
                        HStack(spacing: 10) {
                            Image(systemName: "desktopcomputer")
                                .font(.subheadline)
                                .frame(width: 20)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(runtime.name)
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextPrimary"))
                                Text(runtime.isOnline ? runtime.typeLabel : "\(runtime.typeLabel) · offline")
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            Spacer()
                            Image(systemName: "chevron.right")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!runtime.isOnline)
                }
            }

            if case .failed(let message) = phone.pairStep {
                Text(message)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        } header: {
            Text("Choose a Desktop")
        } footer: {
            Text("The Desktop must be open. You'll approve this iPhone there.")
        }
    }

    private var pairingSection: some View {
        Section {
            VStack(spacing: 12) {
                if case .waitingForDesktop(let code?) = phone.pairStep {
                    Text(Self.spaced(code))
                        .font(.system(size: 34, weight: .semibold, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
                    Text("Check that your Desktop shows the same code, then approve there.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                } else {
                    ProgressView()
                    Text(phone.pairStep == .registering ? "Connecting to your Desktop…" : "Waiting for your Desktop…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
                Button("Cancel") { phone.cancelPairing() }
                    .font(.subheadline)
                    .foregroundColor(Theme.statusError)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
        } header: {
            Text("Pairing")
        }
    }

    // MARK: - Paired

    private func pairedSection(_ c: AppPhoneCredentials) -> some View {
        Section {
            row("Name", c.name)
            row("Desktop", c.runtimeName)
            HStack {
                Text("Bots can reach this app")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Circle()
                    .fill(phone.isControlConnected ? Theme.statusSuccess : Color("TextSecondary").opacity(0.5))
                    .frame(width: 8, height: 8)
                Text(phone.isControlConnected ? "Connected" : "Connecting…")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        } header: {
            Text("This iPhone")
        } footer: {
            Text("Bots reach this app only while it's open. They can use the ACI browser here; a banner shows when they do, with Stop.")
        }
    }

    private var screenSection: some View {
        Section {
            ScreenShareButton()
        } header: {
            Text("Screen")
        } footer: {
            Text("Sends this iPhone's screen and app sound to your Desktop until you stop. Nothing is encoded until a bot or person is watching. The microphone is never sent.")
        }
    }

    private var unpairSection: some View {
        Section {
            Button(action: { isUnpairConfirmPresented = true }) {
                HStack(spacing: 10) {
                    Image(systemName: "xmark.circle")
                        .font(.subheadline)
                        .frame(width: 20)
                    Text("Unpair this iPhone")
                        .font(.subheadline)
                    Spacer()
                }
                .foregroundColor(Theme.statusError)
                .contentShape(Rectangle())
            }
        }
    }

    // MARK: - Helpers

    private func row(_ title: String, _ value: String) -> some View {
        HStack {
            Text(title)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
        }
    }

    /// Desktops and VPS runtimes the phone can pair with — not other phones.
    private var desktops: [RuntimeDevice] {
        runtimes.filter { $0.runtimeType != "ios" && $0.runtimeType != "mobile" }
    }

    private var trimmedName: String {
        let name = phoneName.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? "iPhone" : name
    }

    /// "123456" → "123 456".
    private static func spaced(_ code: String) -> String {
        guard code.count == 6 else { return code }
        return "\(code.prefix(3)) \(code.suffix(3))"
    }

    private func loadRuntimes() async {
        isLoading = true
        defer { isLoading = false }
        do {
            runtimes = try await RuntimeDevicesClient().fetchRuntimes()
            loadError = nil
        } catch APIError.httpError(statusCode: 401, _) {
            loadError = "Sign in with your Allternit account to pair this iPhone."
        } catch {
            loadError = error.localizedDescription
        }
    }
}
