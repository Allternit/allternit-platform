import SwiftUI
import UIKit
import AllternitFilesKit

/// Settings > Files (design 4, Dashboard A: tabs Sharing, Devices, Audit).
///
/// - Devices: each computer's state, its wake setting for Files (Ask first,
///   Automatically, Never; `PUT /computer-files/:id/wake-settings`) and its
///   disks; then this iPhone (files kept offline).
/// - Sharing: every live share on every computer, with Copy link and Turn off.
/// - Audit: what guests did with those shares, newest first.
struct FilesDashboardView: View {
    enum Tab: String, CaseIterable, Identifiable {
        case sharing = "Sharing", devices = "Devices", audit = "Audit"
        var id: String { rawValue }
    }

    struct ComputerRow: Identifiable {
        let computer: FilesSharedComputer
        var state: ComputerState?
        var wake: WakeSetting?
        var storage: StorageDiscovery?
        var error: String?
        var id: ComputerId { computer.id }
    }

    struct AuditRow: Identifiable {
        let access: ShareAccess
        let share: Share
        let computerName: String
        var id: String { "\(access.shareId)/\(access.at)/\(access.guestId)/\(access.action.rawValue)" }
    }

    @ObservedObject private var files = FilesDrive.shared
    @State private var tab: Tab = .sharing
    @State private var rows: [ComputerRow] = []
    @State private var shares: [(share: Share, computerName: String)] = []
    @State private var audit: [AuditRow] = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var revoking: Share?
    @State private var toast: ACIToast.Message?

    var body: some View {
        List {
            Section {
                Picker("Section", selection: $tab) {
                    ForEach(Tab.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
            }
            if !files.isEnabled {
                FriendlyStateView(style: .empty, icon: "folder", title: "Files is off",
                                  message: "Files isn't turned on for this account yet.")
                    .listRowBackground(Color.clear)
            } else if isLoading && rows.isEmpty {
                ProgressView().frame(maxWidth: .infinity).listRowBackground(Color.clear)
            } else {
                switch tab {
                case .sharing: sharing
                case .devices: devices
                case .audit: auditList
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Files")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task { await load() }
        .overlay(alignment: .bottom) { ACIToast(message: $toast) }
        .confirmationDialog("Turn off this share?", isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } }),
                            titleVisibility: .visible, presenting: revoking) { share in
            Button("Turn off", role: .destructive) { Task { await revoke(share) } }
        } message: { _ in
            Text("People with the link lose access on their next request.")
        }
    }

    // MARK: Sharing

    @ViewBuilder
    private var sharing: some View {
        if shares.isEmpty {
            FriendlyStateView(style: .empty, icon: "person.2", title: "No shares",
                              message: "Folders you share from the web or Desktop show here, and you can turn them off.")
                .listRowBackground(Color.clear)
        } else {
            Section {
                ForEach(shares, id: \.share.shareId) { entry in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(Self.shareTitle(entry.share, computerName: entry.computerName))
                            .font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                        Text(Self.shareLine(entry.share))
                            .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                        HStack(spacing: 10) {
                            Button {
                                UIPasteboard.general.string = entry.share.linkUrl
                                toast = ACIToast.Message(kind: .success, title: "Link copied")
                            } label: { Label("Copy link", systemImage: "link") }
                            Spacer()
                            Button(role: .destructive) { revoking = entry.share } label: { Text("Turn off") }
                        }
                        .font(.system(size: 14, weight: .medium))
                        .buttonStyle(.borderless)
                    }
                    .padding(.vertical, 4)
                }
            }
            .listRowBackground(Color("FillNeutral"))
        }
    }

    // MARK: Devices

    @ViewBuilder
    private var devices: some View {
        ForEach(rows) { row in
            Section {
                LabeledContent("State", value: Self.stateLabel(row.state))
                Picker("Wake for Files", selection: wakeBinding(row.id)) {
                    ForEach(WakeSetting.allCases, id: \.self) { Text(FilesFormat.wakeLabel($0)).tag($0) }
                }
                if let volumes = row.storage?.volumes, !volumes.isEmpty {
                    ForEach(volumes, id: \.volumeId) { volume in
                        LabeledContent(volume.label, value: "\(FilesFormat.bytes(volume.freeBytes)) free of \(FilesFormat.bytes(volume.totalBytes))")
                    }
                }
                if let error = row.error {
                    Text(error).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                }
            } header: {
                Text(row.computer.name)
            } footer: {
                if row.id == rows.last?.id {
                    Text("Browsing the saved list never wakes a computer. Opening or saving a file wakes it only as set here, and an awake computer bills credits at your plan's awake rate.")
                }
            }
            .listRowBackground(Color("FillNeutral"))
        }
        Section("This iPhone") {
            LabeledContent("Name", value: files.store.deviceName)
            LabeledContent("Kept offline", value: Self.count(files.store.pins.count, "file"))
        }
        .listRowBackground(Color("FillNeutral"))
    }

    private func wakeBinding(_ computerId: ComputerId) -> Binding<WakeSetting> {
        Binding(
            get: { rows.first { $0.id == computerId }?.wake ?? .ask },
            set: { value in
                guard let index = rows.firstIndex(where: { $0.id == computerId }) else { return }
                let old = rows[index].wake
                rows[index].wake = value
                Task {
                    do {
                        let saved = try await FilesCloudAPI.setWakeSetting(value, for: computerId)
                        if let i = rows.firstIndex(where: { $0.id == computerId }) { rows[i].wake = saved }
                    } catch {
                        if let i = rows.firstIndex(where: { $0.id == computerId }) { rows[i].wake = old }
                        toast = ACIToast.Message(kind: .error, title: "Couldn't change the wake setting",
                                                 detail: FilesComputerModel.message(error))
                    }
                }
            }
        )
    }

    // MARK: Audit

    @ViewBuilder
    private var auditList: some View {
        if audit.isEmpty {
            FriendlyStateView(style: .empty, icon: "checkmark.shield", title: "No guest activity",
                              message: "When someone opens, downloads or uploads through a share, it shows here.")
                .listRowBackground(Color.clear)
        } else {
            Section {
                ForEach(audit) { row in
                    VStack(alignment: .leading, spacing: 4) {
                        HStack {
                            Text(Self.actionLabel(row.access)).font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                            Spacer()
                            Text(FilesFormat.date(row.access.at)).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                        }
                        Text("Guest \(row.access.guestId) · \(Self.shareTitle(row.share, computerName: row.computerName))")
                            .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.vertical, 2)
                }
            }
            .listRowBackground(Color("FillNeutral"))
        }
    }

    // MARK: Loading

    private func load() async {
        files.reload()
        guard files.isEnabled else { isLoading = false; return }
        isLoading = true
        defer { isLoading = false }
        var newRows: [ComputerRow] = []
        var newShares: [(share: Share, computerName: String)] = []
        for computer in files.computers {
            var row = ComputerRow(computer: computer)
            async let endpoints = try? files.client(for: computer.id).endpoints(refresh: true)
            async let wake = try? FilesCloudAPI.wakeSetting(computer.id)
            async let storage = try? FilesCloudAPI.storage(computer.id)
            async let list = Self.capture { try await FilesCloudAPI.shares(computer.id) }
            let e = await endpoints
            row.state = e?.computerState
            row.wake = await wake ?? e?.wake
            row.storage = await storage
            switch await list {
            case .success(let computerShares):
                newShares += computerShares.filter { $0.revokedAt == nil }.map { ($0, computer.name) }
            case .failure(let error):
                row.error = FilesComputerModel.message(error)
            }
            newRows.append(row)
        }
        rows = newRows
        shares = newShares.sorted { $0.share.createdAt > $1.share.createdAt }
        var events: [AuditRow] = []
        for entry in shares.prefix(20) {
            let access = (try? await FilesCloudAPI.access(entry.share.shareId)) ?? []
            events += access.map { AuditRow(access: $0, share: entry.share, computerName: entry.computerName) }
        }
        audit = Array(events.sorted { $0.access.at > $1.access.at }.prefix(200))
    }

    private func revoke(_ share: Share) async {
        do {
            try await FilesCloudAPI.revokeShare(share.shareId)
            shares.removeAll { $0.share.shareId == share.shareId }
            toast = ACIToast.Message(kind: .success, title: "Share turned off")
        } catch {
            toast = ACIToast.Message(kind: .error, title: "Couldn't turn off the share", detail: FilesComputerModel.message(error))
        }
        revoking = nil
    }

    // MARK: Labels

    static func shareTitle(_ share: Share, computerName: String) -> String {
        "\(FilesFormat.roleLabel(share.role)) · \(computerName)"
    }

    static func shareLine(_ share: Share) -> String {
        var parts: [String] = []
        if let expires = share.expiresAt, let date = FilesFormat.parse(expires) {
            parts.append("Ends \(date.formatted(.dateTime.month(.abbreviated).day()))")
        } else {
            parts.append("Until turned off")
        }
        if share.passcodeRequired { parts.append("passcode") }
        parts.append(count(Int(share.guestCount), "guest"))
        if share.guestWake { parts.append("may wake the computer") }
        return parts.joined(separator: " · ")
    }

    static func actionLabel(_ access: ShareAccess) -> String {
        let verb: String
        switch access.action {
        case .redeem: verb = "Opened the link"
        case .list: verb = "Listed files"
        case .download: verb = "Downloaded a file"
        case .upload: verb = "Uploaded a file"
        case .comment: verb = "Commented"
        case .wake: verb = "Asked to wake the computer"
        }
        return access.result == .denied ? "\(verb) (blocked)" : verb
    }

    static func stateLabel(_ state: ComputerState?) -> String {
        switch state {
        case .awake: return "Awake"
        case .asleep: return "Asleep"
        case .waking: return "Waking up"
        case .offline: return "Offline"
        case nil: return "Unknown"
        }
    }

    static func capture<T: Sendable>(_ operation: @Sendable () async throws -> T) async -> Result<T, Error> {
        do { return .success(try await operation()) } catch { return .failure(error) }
    }

    static func count(_ n: Int, _ noun: String) -> String { n == 1 ? "1 \(noun)" : "\(n) \(noun)s" }
}
