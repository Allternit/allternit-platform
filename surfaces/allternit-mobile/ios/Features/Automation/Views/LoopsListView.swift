import SwiftUI

/// Automation Tasks tab surface — Loops sub-view (Phase 3; Goals under
/// `/v1/automations` is a later phase). Structural pattern mirrors
/// `RoutinesListView`: search, row -> detail push, toolbar "+" opens a
/// creation sheet. The Cron/Routines/Loops segmented control (top of all
/// three list views, bound to `modeStore.automationKind`) is what makes this
/// reachable from the same "Automation Tasks" tab — see ChatView.swift's
/// `.automation` case.
///
/// Data: `LoopStore.shared` over `GET v1/automations/loops` on gizzi-code's
/// own server (LoopsClient — same host as PtyClient/PermissionClient/
/// CronClient/RoutinesClient, not the `allternit-api` relay ProjectsClient
/// uses).
struct LoopsListView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var loopStore = LoopStore.shared

    @State private var searchText = ""
    @State private var statusFilter: StatusFilter = .all
    /// Pushed detail (nil = list).
    @State private var detailLoop: Loop? = nil
    @State private var isCreateSheetPresented = false
    @State private var actionError: String? = nil

    private enum StatusFilter: String, CaseIterable {
        case all = "All"
        case running = "Running"
        case succeeded = "Succeeded"
        case exhausted = "Max iterations"
    }

    private var visibleLoops: [Loop] {
        var loops = loopStore.loops
        switch statusFilter {
        case .all: break
        case .running: loops = loops.filter { $0.state == "running" }
        case .succeeded: loops = loops.filter { $0.state == "succeeded" }
        case .exhausted: loops = loops.filter { $0.state == "max_iterations" }
        }
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return loops }
        return loops.filter { $0.command.localizedCaseInsensitiveContains(query) }
    }

    private var filterMenu: some View {
        Menu {
            ForEach(StatusFilter.allCases, id: \.self) { filter in
                Button(action: { statusFilter = filter }) {
                    Label(filter.rawValue, systemImage: statusFilter == filter ? "checkmark" : "")
                }
            }
        } label: {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 32, height: 32)
                .background(Color("BgPanel"))
                .clipShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Filter loops")
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Header (RoutinesListView's standalone-tab chrome: sidebar
                // toggle + title + "+").
                HStack {
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .medium)
                        generator.impactOccurred()
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                            isSidebarOpen.toggle()
                        }
                    }) {
                        Image(systemName: "line.3.horizontal")
                            .font(.title3)
                            .foregroundColor(Color("TextPrimary"))
                            .frame(width: 44, height: 44)
                    }
                    .accessibilityLabel("Open sidebar")

                    Menu {
                        ForEach(AutomationKind.allCases, id: \.self) { kind in
                            Button(action: { modeStore.automationKind = kind }) {
                                Label(
                                    kind.rawValue,
                                    systemImage: modeStore.automationKind == kind ? "checkmark" : ""
                                )
                            }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text(modeStore.automationKind.rawValue)
                                .font(.system(.title3, design: .serif))
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                            Image(systemName: "chevron.down")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }

                    Spacer()

                    filterMenu

                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        isCreateSheetPresented = true
                    }) {
                        Image(systemName: "plus")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 32, height: 32)
                            .background(Color("BgPanel"))
                            .clipShape(Circle())
                    }
                    .accessibilityLabel("New loop")
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 10)
                .background(Color("BgPrimary"))

                Divider().background(Color("BorderSubtle"))

                // Runs on the user's computer: "Connect a computer" when none answers.

                ConnectComputerView()

                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailLoop) { loop in
                LoopDetailView(loop: loop)
            }
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CreateLoopSheet { command, exitCondition, maxIterations in
                createLoop(command: command, exitCondition: exitCondition, maxIterations: maxIterations)
            }
        }
        .task {
            loopStore.fetchLoopsIfNeeded()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        VStack(spacing: 0) {
            if loopStore.isLoading && loopStore.loops.isEmpty {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError = loopStore.loadError, loopStore.loops.isEmpty {
                Spacer()
                FriendlyStateView(
                    style: errorStyle(loadError),
                    icon: "wifi.slash",
                    title: "Couldn't load loops",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { loopStore.fetchLoopsIfNeeded(force: true) }
                )
                Spacer()
            } else if loopStore.loops.isEmpty {
                Spacer()
                emptyState
                Spacer()
            } else {
                listContent
            }
        }
    }

    private var listContent: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search loops", text: $searchText)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if !searchText.isEmpty {
                    Button(action: { searchText = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(Color("BgSecondary"))
            .cornerRadius(10)
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 12)

            if let actionError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Action failed",
                    message: actionError
                )
            }

            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(visibleLoops) { loop in
                        loopRow(loop)
                    }
                    if visibleLoops.isEmpty {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "magnifyingglass",
                            title: "No matches",
                            message: "No loops match."
                        )
                        .padding(.top, 24)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await loopStore.refresh()
            }
        }
    }

    private func loopRow(_ loop: Loop) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailLoop = loop
        }) {
            HStack(spacing: 12) {
                LoopStaminaRing(loop: loop)
                VStack(alignment: .leading, spacing: 2) {
                    Text(loop.command)
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(Self.subtitleText(loop))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer()
                Self.statusBadge(loop.state)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 64)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "repeat",
            title: "No loops",
            message: "Run a command on repeat until it exits clean or hits its iteration cap.",
            actionTitle: "Create loop",
            action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isCreateSheetPresented = true
            }
        )
    }

    // MARK: - Actions

    private func createLoop(command: String, exitCondition: String?, maxIterations: Int) {
        Task {
            do {
                try await loopStore.createLoop(command: command, exitCondition: exitCondition, maxIterations: maxIterations)
            } catch {
                actionError = "Couldn't create the loop: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Formatting (shared with LoopDetailView)

    /// Status badge colors mirroring RoutinesListView's convention, adapted
    /// to loop `state` values (running/succeeded/max_iterations,
    /// loop-engine.ts:19,72-105) — `max_iterations` means it ran out of
    /// attempts without its exit condition firing, so it reads as a warning
    /// rather than a hard failure.
    static func statusColor(_ state: String) -> Color {
        switch state {
        case "running": return Theme.statusInfo
        case "succeeded": return Theme.statusSuccess
        case "max_iterations": return Theme.statusWarning
        default: return Color("TextSecondary")
        }
    }

    private static func statusBadge(_ state: String) -> some View {
        Text(Self.stateLabel(state))
            .font(.caption2)
            .fontWeight(.semibold)
            .foregroundColor(statusColor(state))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(statusColor(state).opacity(0.15))
            .clipShape(Capsule())
    }

    private static func stateLabel(_ state: String) -> String {
        state == "max_iterations" ? "Max iterations" : state.capitalized
    }

    private static func subtitleText(_ loop: Loop) -> String {
        var parts: [String] = []
        let count = loop.iterationLog.count
        parts.append("\(count)/\(loop.maxIterations) iterations")
        if let exitCondition = loop.exitCondition, !exitCondition.isEmpty {
            parts.append(exitCondition)
        }
        return parts.joined(separator: " · ")
    }

    /// `time_created`/`time_updated` are ms-epoch numbers, same convention
    /// as Routine (RoutinesListView.relativeText).
    static func relativeText(_ msEpoch: Double?) -> String? {
        RoutinesListView.relativeText(msEpoch)
    }

    private func errorStyle(_ error: String) -> FriendlyStateView.Style {
        let lowered = error.lowercased()
        if lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("internet connection")
            || lowered.contains("offline")
            || lowered.contains("network") {
            return .offline
        }
        return .error
    }
}
