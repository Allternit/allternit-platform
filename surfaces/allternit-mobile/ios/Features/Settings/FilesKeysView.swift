import SwiftUI
import UIKit
import AllternitFilesKit

/// The Files key on this iPhone (SPEC section 13), shown in Settings > Files >
/// Devices (`FilesDashboardView.devices`) after the "This iPhone" section. It
/// lets this iPhone open the sealed replicas other computers keep, so a file
/// on a sleeping computer opens without waking it.
///
/// - **No key on the account yet:** "Set up" makes it here (first device).
/// - **New iPhone:** "Ask for access" sends a request and shows a 6-digit
///   code; the screen checks every 5 seconds until another device approves.
/// - **Approve / Decline:** requests from the user's other devices, each with
///   the code computed here from that device's key. A request whose code
///   doesn't match its key can only be declined.
/// - **Devices:** every device with the key, each with Remove. After a
///   removal the key needs rotating: "Make a new key" seals a new one to the
///   devices still listed.
/// - **Computers:** "Give key" hands a computer the key over the relay so it
///   can keep replicas.
@MainActor
struct FilesKeysSection: View {
    @ObservedObject private var files = FilesDrive.shared
    @State private var info: FilesKeyInfo?
    @State private var state: FilesKeyState?
    @State private var busy: String?
    @State private var error: String?
    @State private var notice: String?
    @State private var confirming: FilesKeyApproval?
    @State private var removing: FilesKeyDevice?

    private var service: FilesKeyService { FilesKeyService(store: files.store) }
    private var isReady: Bool { if case .ready = state { return true } else { return false } }
    private var approvals: [FilesKeyApproval] { info.map { service.approvals($0) } ?? [] }
    private var computers: [FilesSharedComputer] { files.store.computers }

    var body: some View {
        Group {
            Section {
                thisDevice
                if let error {
                    Text(error).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
                if let notice {
                    Text(notice).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            } header: {
                Text("Files key on this iPhone")
            } footer: {
                Text("Your computers keep sealed copies (replicas) of chosen folders on each other. With the Files key, this iPhone opens a file from a copy while its computer sleeps, without waking it. The key never leaves your devices; Allternit stores only copies it can't open.")
            }
            .listRowBackground(Color("FillNeutral"))

            if isReady, !approvals.isEmpty {
                Section {
                    ForEach(approvals) { approvalRow($0) }
                } header: {
                    Text("Waiting for approval")
                } footer: {
                    Text("Approve a device only if it shows the same code as here.")
                }
                .listRowBackground(Color("FillNeutral"))
            }

            if isReady, let info, info.rotationNeeded {
                Section {
                    Text("A device was removed. Make a new Files key so it can't open copies made from now on. Your other devices get the new key automatically.")
                        .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                    Button { Task { await rotate(info) } } label: { busyLabel("Make a new key", tag: "rotate") }
                        .disabled(busy != nil)
                } header: {
                    Text("New key needed")
                }
                .listRowBackground(Color("FillNeutral"))
            }

            if let info, info.keyId != nil, !info.devices.isEmpty {
                Section {
                    ForEach(info.devices, id: \.deviceId) { deviceRow($0) }
                } header: {
                    Text("Devices with the Files key")
                }
                .listRowBackground(Color("FillNeutral"))
            }

            if isReady, let info, !computers.isEmpty {
                Section {
                    ForEach(computers) { computerRow($0, info: info) }
                } header: {
                    Text("Computers")
                } footer: {
                    Text("A computer needs the key to keep sealed copies for your other computers.")
                }
                .listRowBackground(Color("FillNeutral"))
            }
        }
        .task { await refresh() }
        .task(id: waitingCode) { await pollWhileWaiting() }
        .confirmationDialog(
            confirming.map { "Give \($0.request.deviceName) the Files key?" } ?? "",
            isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
            titleVisibility: .visible, presenting: confirming
        ) { approval in
            Button("Approve") { Task { await approve(approval) } }
        } message: { approval in
            Text("It can then open your replicas. Check that \(approval.request.deviceName) shows \(Self.spaced(approval.code)).")
        }
        .confirmationDialog(
            removing.map { "Remove \($0.deviceName)?" } ?? "",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible, presenting: removing
        ) { device in
            Button("Remove", role: .destructive) { Task { await remove(device) } }
        } message: { device in
            Text("\(device.deviceName) loses the Files key. Make a new key afterwards so it can't open new copies.")
        }
    }

    private var waitingCode: String? { if case .waiting(let code) = state { return code } else { return nil } }

    // MARK: Rows

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
                Text("Approve this iPhone on another of your devices (Settings > Files > Devices on a phone, or the Devices tab in Files on the web or Desktop). It shows this code:")
                    .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                Text(Self.spaced(code))
                    .font(.system(size: 34, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for approval…").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            }
            .padding(.vertical, 4)
            Button { Task { await ask() } } label: { busyLabel("Send the request again", tag: "ask") }.disabled(busy != nil)
        case .needsRequest:
            Text("Your other devices already have the Files key. Ask one of them to share it with this iPhone.")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            Button { Task { await ask() } } label: { busyLabel("Ask for access", tag: "ask") }.disabled(busy != nil)
        case .noKey:
            Text("Your account has no Files key yet. Make it on this iPhone, then approve your other devices from here.")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            Button { Task { await createKey() } } label: { busyLabel("Set up the Files key", tag: "create") }.disabled(busy != nil)
        }
    }

    private func approvalRow(_ approval: FilesKeyApproval) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(approval.request.deviceName).font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
            if approval.codeMatches {
                Text(Self.spaced(approval.code))
                    .font(.system(size: 20, weight: .semibold, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
            } else {
                Text("This request's code doesn't match its key. Don't approve it; decline it and ask the device to request again.")
                    .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
            }
            HStack(spacing: 10) {
                if approval.codeMatches {
                    Button("Approve") { confirming = approval }
                        .buttonStyle(.borderedProminent)
                        .disabled(busy != nil)
                }
                Button { Task { await decline(approval) } } label: { busyLabel("Decline", tag: "decline.\(approval.id)") }
                    .buttonStyle(.bordered)
                    .disabled(busy != nil)
            }
        }
        .padding(.vertical, 4)
    }

    private func deviceRow(_ device: FilesKeyDevice) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(device.deviceId == files.store.deviceId ? "\(device.deviceName) (this iPhone)" : device.deviceName)
                    .font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                Text(device.hasCurrentKey ? "Has the current key" : "Waiting for the new key")
                    .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            if isReady {
                Button("Remove", role: .destructive) { removing = device }
                    .buttonStyle(.bordered)
                    .disabled(busy != nil)
            }
        }
    }

    private func computerRow(_ computer: FilesSharedComputer, info: FilesKeyInfo) -> some View {
        let has = info.deviceIds.contains(computer.id)
        return HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(computer.name).font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                Text(has ? "Has the key" : "No key yet").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            Button { Task { await give(computer, has: has) } } label: {
                busyLabel(has ? "Send again" : "Give key", tag: "give.\(computer.id)")
            }
            .buttonStyle(.bordered)
            .disabled(busy != nil)
        }
    }

    private func busyLabel(_ title: String, tag: String) -> some View {
        HStack { Text(title); if busy == tag { ProgressView().controlSize(.small) } }
    }

    // MARK: Actions

    private func refresh() async {
        do {
            let next = try await service.keyInfo()
            info = next
            state = try await service.state(next)
            error = nil
        } catch {
            if state == nil { state = .needsRequest }
            self.error = Self.message(error)
        }
    }

    /// While the code shows, checks every 5 seconds for the approval.
    private func pollWhileWaiting() async {
        guard waitingCode != nil else { return }
        while !Task.isCancelled, waitingCode != nil {
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            guard !Task.isCancelled else { return }
            await refresh()
            if isReady { Haptics.success() }
        }
    }

    private func ask() async {
        await run("ask") { _ = try await service.requestAccess(deviceName: UIDevice.current.name) }
    }

    private func createKey() async {
        await run("create") { _ = try await service.createFirstKey(deviceName: UIDevice.current.name) }
    }

    private func approve(_ approval: FilesKeyApproval) async {
        await run("approve.\(approval.id)") { try await service.approve(approval) }
    }

    private func decline(_ approval: FilesKeyApproval) async {
        await run("decline.\(approval.id)") { try await service.decline(approval) }
    }

    private func remove(_ device: FilesKeyDevice) async {
        await run("remove.\(device.deviceId)") { try await service.removeDevice(device.deviceId) }
    }

    /// New key for every listed device, then the computers among them get
    /// their new envelope over the relay (an asleep one gets it next time).
    private func rotate(_ info: FilesKeyInfo) async {
        await run("rotate") {
            _ = try await service.rotate(info)
            var missed: [String] = []
            for computer in computers where info.devices.contains(where: { $0.deviceId == computer.id }) {
                do {
                    try await deliverUnlocking(computer) { try await service.deliver(to: $0, deviceId: computer.id) }
                } catch {
                    missed.append(computer.name)
                }
            }
            notice = missed.isEmpty ? nil : "New key made. Give it to \(missed.joined(separator: ", ")) when they're on."
        }
    }

    private func give(_ computer: FilesSharedComputer, has: Bool) async {
        await run("give.\(computer.id)") {
            try await deliverUnlocking(computer) { client in
                if has { try await service.deliver(to: client, deviceId: computer.id) } else { try await service.giveComputer(client) }
            }
            notice = "\(computer.name) has the Files key."
        }
    }

    /// Runs `action` against the computer; when it holds a different key the
    /// runtime wants a fresh unlock (Face ID, then a signed token), so ask
    /// for it once and try again.
    private func deliverUnlocking(_ computer: FilesSharedComputer, _ action: (FilesClient) async throws -> Void) async throws {
        let client = files.client(for: computer.id)
        do {
            try await action(client)
        } catch let error as FilesError where error.code == .protectedRootLocked {
            try await client.unlock(reason: "Replace the Files key on \(computer.name)")
            try await action(client)
        }
    }

    private func run(_ tag: String, _ action: () async throws -> Void) async {
        busy = tag
        defer { busy = nil }
        do {
            notice = nil
            try await action()
            Haptics.success()
        } catch {
            self.error = Self.message(error)
            await refreshKeepingError()
            return
        }
        await refresh()
    }

    private func refreshKeepingError() async {
        let kept = error
        await refresh()
        if error == nil { error = kept }
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
        if (error as NSError).domain == "com.apple.LocalAuthentication" { return "Unlock was cancelled." }
        return error.localizedDescription
    }
}
