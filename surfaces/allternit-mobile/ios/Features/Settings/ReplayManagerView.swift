import SwiftUI

/// Phase-1 Replay Manager for iOS.
///
/// Mirrors the web's ReplayManagerView: search, capture-level filter, stat
/// cards, and per-row replay against `/api/v1/runtime/replay/sessions/:run_id/execute`.
struct ReplayManagerView: View {
    @StateObject private var store = RuntimeOperationsStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var searchText = ""
    @State private var captureFilter: CaptureFilter = .all
    @State private var lastReplayError: String? = nil

    private var filteredManifests: [ReplayManifest] {
        store.replayManifests.filter { manifest in
            if captureFilter != .all,
               manifest.captureLevel.lowercased() != captureFilter.rawValue.lowercased() {
                return false
            }
            if let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).presence,
               !manifest.runId.lowercased().contains(query.lowercased()) {
                return false
            }
            return true
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .task {
            store.fetchIfNeeded()
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Replay Manager")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Text("Inspect and replay captured runtime sessions")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            Button(action: { dismiss() }) {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.replayManifests.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.replayManifests.isEmpty {
            Spacer()
            let offline = isConnectionFailure(loadError)
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: "Failed to load replay sessions",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else {
            ScrollView {
                VStack(spacing: 16) {
                    searchAndFilter
                    statCards
                    infoBanner
                    if let lastReplayError {
                        replayErrorBanner(lastReplayError)
                    }
                    manifestList
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
            .refreshable {
                await store.refresh()
            }
        }
    }

    // MARK: - Search + filter

    private var searchAndFilter: some View {
        VStack(spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search by run ID…", text: $searchText)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
            }
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )

            HStack(spacing: 8) {
                ForEach(CaptureFilter.allCases) { filter in
                    Button(action: { captureFilter = filter }) {
                        Text(filter.label)
                            .font(.system(size: 12, weight: .semibold))
                            .tracking(0.5)
                            .foregroundColor(captureFilter == filter ? Color("TextPrimary") : Color("TextSecondary"))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(captureFilter == filter ? Color("AccentPrimary").opacity(0.16) : Color("BgPanel"))
                            .clipShape(Capsule())
                            .overlay(
                                Capsule()
                                    .stroke(captureFilter == filter ? Color("AccentPrimary").opacity(0.4) : Theme.borderWarmDefault, lineWidth: 1)
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }

    // MARK: - Stat cards

    private var statCards: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
            statCard(
                label: "Captured Runs",
                value: "\(store.replayManifests.count)",
                icon: "clock.arrow.circlepath",
                color: Color("AccentPrimary")
            )
            statCard(
                label: "Full Capture",
                value: "\(store.replayManifests.filter { $0.captureLevel.lowercased() == "full" }.count)",
                icon: "checkmark.circle",
                color: Theme.statusSuccess
            )
            statCard(
                label: "Captured Outputs",
                value: "\(store.replayManifests.reduce(0) { $0 + $1.outputCount })",
                icon: "list.number",
                color: Theme.statusInfo
            )
        }
    }

    private func statCard(label: String, value: String, icon: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Image(systemName: icon)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundColor(color)
                Spacer()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(label.uppercased())
                    .font(.system(size: 9, weight: .semibold))
                    .tracking(1)
                    .foregroundColor(Color("TextSecondary"))
                Text(value)
                    .font(.system(size: 22, weight: .heavy, design: .rounded))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Info banner

    private var infoBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "info.circle")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
            Text("The current API exposes replay execution and capture counts, but not manifest export or delete. This panel reflects those live capabilities directly.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
        }
        .padding(12)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func replayErrorBanner(_ message: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 14))
                .foregroundColor(Theme.statusError)
            Text(message)
                .font(.caption)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            Button(action: { lastReplayError = nil }) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(Theme.statusError.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.statusError.opacity(0.3), lineWidth: 1)
        )
    }

    // MARK: - Manifest list

    @ViewBuilder
    private var manifestList: some View {
        if filteredManifests.isEmpty {
            emptyState
        } else {
            VStack(spacing: 12) {
                ForEach(filteredManifests) { manifest in
                    manifestRow(manifest)
                }
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "clock.arrow.circlepath",
            title: "No replay sessions found",
            message: searchText.isEmpty && captureFilter == .all
                ? "Run tools or workflows with replay enabled to populate this view."
                : "Clear the filters to inspect the full capture catalog.",
            art: searchText.isEmpty && captureFilter == .all ? .bell : nil
        )
        .padding(.vertical, 24)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func manifestRow(_ manifest: ReplayManifest) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Text(manifest.runId)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 8)
                captureBadge(manifest.captureLevel)
            }

            HStack(spacing: 12) {
                manifestMiniStat("Outputs", value: "\(manifest.outputCount)")
                manifestMiniStat("Timestamps", value: "\(manifest.timestampCount)")
                manifestMiniStat("Readiness", value: manifest.captureLevel.lowercased() == "none" ? "Metadata Only" : "Replayable")
            }

            Button(action: { replay(manifest.runId) }) {
                HStack(spacing: 6) {
                    if store.replayingRunId == manifest.runId {
                        ProgressView()
                            .scaleEffect(0.7)
                    } else {
                        Image(systemName: "play.fill")
                            .font(.system(size: 11, weight: .semibold))
                    }
                    Text(store.replayingRunId == manifest.runId ? "Replaying…" : "Replay Session")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("AccentPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("AccentPrimary").opacity(0.25), lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            .disabled(store.replayingRunId == manifest.runId)
            .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func manifestMiniStat(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label.uppercased())
                .font(.system(size: 9, weight: .semibold))
                .tracking(1)
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
    }

    private func captureBadge(_ level: String) -> some View {
        let isFull = level.lowercased() == "full"
        return Text("\(level) capture")
            .font(.system(size: 10, weight: .bold))
            .tracking(0.8)
            .foregroundColor(isFull ? Theme.statusSuccess : Theme.statusWarning)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background((isFull ? Theme.statusSuccess : Theme.statusWarning).opacity(0.12))
            .clipShape(Capsule())
    }

    // MARK: - Actions

    private func replay(_ runId: String) {
        lastReplayError = nil
        Task {
            do {
                try await store.executeReplay(runId: runId)
            } catch {
                lastReplayError = error.localizedDescription
            }
        }
    }

    // MARK: - Filter model

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private enum CaptureFilter: String, CaseIterable, Identifiable {
        case all
        case minimal
        case full

        var id: String { rawValue }

        var label: String {
            switch self {
            case .all: return "All"
            case .minimal: return "Minimal"
            case .full: return "Full"
            }
        }
    }
}

// MARK: - String helpers

private extension String {
    /// Returns `self` if non-empty after trimming; otherwise `nil`.
    var presence: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
