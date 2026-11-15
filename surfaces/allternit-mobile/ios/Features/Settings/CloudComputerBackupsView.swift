import SwiftUI

/// `GET /api/v1/provisioned-instances` row (cloud-api).
struct CloudComputerSummary: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let status: String
    let tier: String?
    let cpuCores: Int?
    let memoryMb: Int?

    /// Restore points and copies work only in these states (backend rule).
    var supportsBackups: Bool { ["running", "stopped", "sleeping"].contains(status) }

    var label: String {
        var text = tier.map { $0.capitalized } ?? "Cloud computer"
        if let cpuCores, let memoryMb {
            text += " · \(cpuCores) vCPU / \(Int((Double(memoryMb) / 1024).rounded())) GB"
        }
        return text
    }
}

private struct CloudComputersEnvelope: Decodable, Sendable {
    let instances: [CloudComputerSummary]?
}

struct RestorePoint: Decodable, Identifiable, Equatable, Sendable {
    let name: String
    let createdAt: String
    var id: String { name }
}

private struct RestorePointsEnvelope: Decodable, Sendable {
    let snapshots: [RestorePoint]
    let max: Int
}

private struct RestorePointEnvelope: Decodable, Sendable {
    let snapshot: RestorePoint
}

struct ComputerExport: Decodable, Equatable, Sendable {
    let id: String
    let status: String
    let createdAt: String
    let expiresAt: String
    let error: String?
    let downloadPath: String?
}

private struct ComputerExportEnvelope: Decodable, Sendable {
    let export: ComputerExport
}

/// Settings → Computers → Backups (the PWA's CloudComputerBackups): restore
/// points and "Download a copy" for each Allternit cloud computer. Control
/// plane routes `/api/v1/provisioned-instances/:id/snapshots|exports`.
struct CloudComputerBackupsView: View {
    @State private var computers: [CloudComputerSummary] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil

    var body: some View {
        List {
            if isLoading && computers.isEmpty {
                Section { ProgressView().frame(maxWidth: .infinity) }
            } else if let loadError, computers.isEmpty {
                Section {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Couldn't load your cloud computers",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Try again",
                        action: { Task { await load() } }
                    )
                }
            } else if computers.isEmpty {
                Section {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "externaldrive.badge.icloud",
                        title: "No cloud computers",
                        message: "Backups appear here once you have an Allternit cloud computer."
                    )
                }
            } else {
                ForEach(computers) { computer in
                    ComputerBackupSection(computer: computer)
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Backups")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .refreshable { await load() }
    }

    @MainActor
    private func load() async {
        isLoading = true
        loadError = nil
        defer { isLoading = false }
        do {
            let envelope: CloudComputersEnvelope = try await APIClient.shared.get(path: "provisioned-instances")
            computers = (envelope.instances ?? []).filter { $0.status != "deleted" }
        } catch {
            // The route may answer a bare array.
            if let list: [CloudComputerSummary] = try? await APIClient.shared.get(path: "provisioned-instances") {
                computers = list.filter { $0.status != "deleted" }
            } else {
                loadError = error.localizedDescription
            }
        }
    }
}

/// One computer's restore points + download.
private struct ComputerBackupSection: View {
    let computer: CloudComputerSummary
    @Environment(\.openURL) private var openURL

    @State private var points: [RestorePoint]? = nil
    @State private var maxPoints = 0
    @State private var busy: String? = nil
    @State private var errorMessage: String? = nil
    @State private var confirmRestore: RestorePoint? = nil
    @State private var copy: ComputerExport? = nil

    private var base: String {
        "provisioned-instances/\(computer.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? computer.id)"
    }

    var body: some View {
        Section {
            if !computer.supportsBackups {
                Text("Backups are available while the computer is running, stopped or asleep. It's \(computer.status) now.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            } else if points == nil {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading restore points…").font(.caption).foregroundColor(Color("TextSecondary"))
                }
            } else if let points {
                if points.isEmpty {
                    Text("No restore points yet.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                ForEach(points) { point in
                    HStack {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(Self.when(point.createdAt))
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                        }
                        Spacer()
                        Button("Restore") { confirmRestore = point }
                            .font(.caption.weight(.semibold))
                            .buttonStyle(.bordered)
                        Button(action: { Task { await remove(point) } }) {
                            Image(systemName: "trash")
                        }
                        .buttonStyle(.bordered)
                        .accessibilityLabel("Delete restore point")
                    }
                    .disabled(busy != nil)
                }
                Button(action: { Task { await create() } }) {
                    SettingsRowLabel(
                        title: busy == "create" ? "Taking restore point…" : "Take a restore point",
                        systemImage: "camera"
                    )
                }
                .disabled(busy != nil || points.count >= maxPoints && maxPoints > 0)

                downloadRow
            }
            if let errorMessage {
                Text(errorMessage).font(.caption).foregroundColor(Theme.statusError)
            }
        } header: {
            Text(computer.label)
        } footer: {
            if let points, maxPoints > 0, points.count >= maxPoints {
                Text("You can keep \(maxPoints). Delete one to take another.")
            }
        }
        .task { await loadPoints() }
        .confirmationDialog(
            "Restore to \(confirmRestore.map { Self.when($0.createdAt) } ?? "")?",
            isPresented: Binding(get: { confirmRestore != nil }, set: { if !$0 { confirmRestore = nil } }),
            titleVisibility: .visible
        ) {
            Button("Restore", role: .destructive) {
                if let point = confirmRestore { Task { await restore(point) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The computer goes back to this point. Changes made after it are lost.")
        }
    }

    @ViewBuilder
    private var downloadRow: some View {
        if let copy, copy.status == "ready", let path = copy.downloadPath,
           let url = URL(string: path, relativeTo: AppConfig.apiBaseURL)?.absoluteURL {
            Button(action: { openURL(url) }) {
                SettingsRowLabel(title: "Download the copy", systemImage: "icloud.and.arrow.down")
            }
        } else if copy?.status == "preparing" || busy == "export" {
            HStack(spacing: 8) {
                ProgressView()
                Text("Preparing a copy… this can take a few minutes.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        } else {
            Button(action: { Task { await startExport() } }) {
                SettingsRowLabel(title: "Download a copy", systemImage: "icloud.and.arrow.down")
            }
            .disabled(busy != nil)
            if copy?.status == "failed" {
                Text(copy?.error ?? "The last copy failed.")
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        }
    }

    @MainActor
    private func loadPoints() async {
        guard computer.supportsBackups else { return }
        do {
            let envelope: RestorePointsEnvelope = try await APIClient.shared.get(path: "\(base)/snapshots")
            points = envelope.snapshots
            maxPoints = envelope.max
        } catch {
            points = []
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    @MainActor
    private func create() async {
        busy = "create"
        errorMessage = nil
        defer { busy = nil }
        do {
            let envelope: RestorePointEnvelope = try await APIClient.shared.post(path: "\(base)/snapshots", body: [String: String]())
            points = [envelope.snapshot] + (points ?? [])
        } catch {
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    @MainActor
    private func restore(_ point: RestorePoint) async {
        confirmRestore = nil
        busy = "restore"
        errorMessage = nil
        defer { busy = nil }
        do {
            try await APIClient.shared.post(path: "\(base)/snapshots/\(Self.escape(point.name))/restore")
        } catch {
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    @MainActor
    private func remove(_ point: RestorePoint) async {
        busy = "delete"
        errorMessage = nil
        defer { busy = nil }
        do {
            try await APIClient.shared.delete(path: "\(base)/snapshots/\(Self.escape(point.name))")
            points?.removeAll { $0.name == point.name }
        } catch {
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    @MainActor
    private func startExport() async {
        busy = "export"
        errorMessage = nil
        defer { busy = nil }
        do {
            let envelope: ComputerExportEnvelope = try await APIClient.shared.post(path: "\(base)/exports", body: [String: String]())
            copy = envelope.export
            // Poll until ready or failed (5 s, like the web), while visible.
            while copy?.status == "preparing", !Task.isCancelled {
                try await Task.sleep(nanoseconds: 5_000_000_000)
                let next: ComputerExportEnvelope = try await APIClient.shared.get(path: "\(base)/exports/\(Self.escape(envelope.export.id))")
                copy = next.export
            }
        } catch is CancellationError {
            return
        } catch {
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? value
    }

    static func when(_ raw: String) -> String {
        guard let date = DevicePairingView.parseDate(raw) else { return raw }
        return date.formatted(.dateTime.month(.abbreviated).day().hour().minute())
    }
}
