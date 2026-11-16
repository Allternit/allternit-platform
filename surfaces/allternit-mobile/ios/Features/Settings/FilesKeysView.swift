import SwiftUI
import UIKit
import AllternitFilesKit

/// The Files key on this iPhone (SPEC section 13): what lets it open the
/// sealed replicas other computers keep, so a file on a sleeping computer
/// opens without waking it.
///
/// - **No key on the account yet:** "Set up" makes it here (first device).
/// - **New iPhone:** "Ask for access" sends a request and shows a 6-digit
///   code; the user approves it on another device that shows the same code.
/// - **Approve:** requests from the user's other devices, each with the
///   code computed here from that device's key. Approve only when both
///   screens show the same code.
///
/// Hook: Settings > Files > Devices (`FilesDashboardView.devices`, #2453)
/// puts `FilesKeysSection()` after the "This iPhone" section. Until that
/// lands, `FilesKeysView` is the same content as its own screen.
struct FilesKeysView: View {
    var body: some View {
        List { FilesKeysSection() }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Files key")
            .navigationBarTitleDisplayMode(.inline)
    }
}

@MainActor
struct FilesKeysSection: View {
    @State private var state: FilesKeyState?
    @State private var approvals: [FilesKeyApproval] = []
    @State private var busy = false
    @State private var error: String?
    @State private var confirming: FilesKeyApproval?

    private var service: FilesKeyService { FilesKeyService(store: FilesDriveBridge.shared.store) }

    var body: some View {
        Group {
            Section {
                thisDevice
                if let error {
                    Text(error).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            } header: {
                Text("Files key on this iPhone")
            } footer: {
                Text("Your computers keep sealed copies (replicas) of chosen folders on each other. With the Files key, this iPhone opens a file from a copy while its computer sleeps, without waking it. The key never leaves your devices; Allternit stores only copies it can't open.")
            }
            .listRowBackground(Color("FillNeutral"))

            if case .ready = state, !approvals.isEmpty {
                Section {
                    ForEach(approvals) { approval in
                        approvalRow(approval)
                    }
                } header: {
                    Text("Waiting for approval")
                } footer: {
                    Text("Approve a device only if it shows the same code as here.")
                }
                .listRowBackground(Color("FillNeutral"))
            }
        }
        .task { await refresh() }
        .refreshable { await refresh() }
        .confirmationDialog(
            confirming.map { "Give \($0.request.deviceName) the Files key?" } ?? "",
            isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
            titleVisibility: .visible, presenting: confirming
        ) { approval in
            Button("Approve") { Task { await approve(approval) } }
        } message: { approval in
            Text("It can then open your replicas. Check that \(approval.request.deviceName) shows \(Self.spaced(approval.code)).")
        }
    }

    @ViewBuilder
    private var thisDevice: some View {
        switch state {
        case nil:
            HStack { Text("Checking…").foregroundColor(Color("TextSecondary")); Spacer(); ProgressView() }
        case .ready(let keyId):
            LabeledContent("Status", value: "Ready")
            LabeledContent("Key", value: keyId)
        case .waiting(let code):
            VStack(alignment: .leading, spacing: 8) {
                Text("Approve this iPhone on another of your devices (Settings > Files > Devices on a phone, or Files settings on the web or Desktop). It shows this code:")
                    .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                Text(Self.spaced(code))
                    .font(.system(size: 34, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
            }
            .padding(.vertical, 4)
            Button { Task { await refresh() } } label: { busyLabel("Check again") }.disabled(busy)
            Button { Task { await ask() } } label: { Text("Send the request again") }.disabled(busy)
        case .needsRequest:
            Text("Your other devices already have the Files key. Ask one of them to share it with this iPhone.")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            Button { Task { await ask() } } label: { busyLabel("Ask for access") }.disabled(busy)
        case .noKey:
            Text("Your account has no Files key yet. Make it on this iPhone, then approve your other devices from here.")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            Button { Task { await createKey() } } label: { busyLabel("Set up the Files key") }.disabled(busy)
        }
    }

    private func approvalRow(_ approval: FilesKeyApproval) -> some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(approval.request.deviceName).font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                if approval.codeMatches {
                    Text(Self.spaced(approval.code))
                        .font(.system(size: 20, weight: .semibold, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                } else {
                    Text("This request's code doesn't match its key. Don't approve it; ask the device to request again.")
                        .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                }
            }
            Spacer()
            if approval.codeMatches {
                Button("Approve") { confirming = approval }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy)
            }
        }
        .padding(.vertical, 4)
    }

    private func busyLabel(_ title: String) -> some View {
        HStack { Text(title); if busy { Spacer(); ProgressView() } }
    }

    // MARK: Actions

    private func refresh() async {
        error = nil
        do {
            let next = try await service.state()
            state = next
            if case .ready = next {
                approvals = (try? await service.pendingRequests()) ?? []
            } else {
                approvals = []
            }
        } catch {
            if state == nil { state = .needsRequest }
            self.error = Self.message(error)
        }
    }

    private func ask() async {
        await run { _ = try await service.requestAccess(deviceName: UIDevice.current.name) }
    }

    private func createKey() async {
        await run { _ = try await service.createFirstKey() }
    }

    private func approve(_ approval: FilesKeyApproval) async {
        await run { try await service.approve(approval) }
    }

    private func run(_ action: () async throws -> Void) async {
        busy = true
        defer { busy = false }
        do {
            try await action()
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            await refresh()
        } catch {
            self.error = Self.message(error)
        }
    }

    /// "123 456"
    static func spaced(_ code: String) -> String {
        code.count == 6 ? String(code.prefix(3)) + " " + String(code.suffix(3)) : code
    }

    static func message(_ error: Error) -> String {
        if let files = error as? FilesError {
            if files.isAuthFailure { return "Open the app's Files once more to sign Files in again." }
            return files.localizedDescription
        }
        return error.localizedDescription
    }
}
