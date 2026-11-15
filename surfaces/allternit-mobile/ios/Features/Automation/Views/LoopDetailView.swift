import SwiftUI

/// Loop detail — structural pattern from `RoutineDetailView`: header
/// (command/state/exit-condition/iteration-cap) + a Restart + Delete action
/// row. Unlike Routine's Run (which starts an inert routine for the first
/// time), a loop is already running the moment it's created
/// (`LoopEngine.startLoop` is called inline in the create route,
/// automations.ts:269) — so this view's Run action is really "restart",
/// and stays disabled while `state == "running"` even though the endpoint
/// itself doesn't reject a redundant call. There's no run-history endpoint,
/// so `iteration_log` (refetched via `LoopStore.refresh()`, whole-list,
/// since there's no `GET /loops/:id`) stands in for it — each entry
/// expands to show its full command output, mirroring
/// `AutomationTaskDetailView`'s run-row expand pattern.
struct LoopDetailView: View {
    let loop: Loop

    @StateObject private var loopStore = LoopStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var actionError: String? = nil
    @State private var isMutating = false
    @State private var isDeleteConfirmPresented = false
    @State private var expandedIterations: Set<Int> = []

    /// The live store record (run/delete land here); falls back to the value
    /// captured on push (RoutineDetailView's `liveRoutine`).
    private var liveLoop: Loop {
        loopStore.loop(withId: loop.id) ?? loop
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if let actionError {
                    Text(actionError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }

                headerSection
                if liveLoop.state == "max_iterations" {
                    interruptBanner
                }
                actionRow
                loopCardSection
                iterationsSection
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(liveLoop.command)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(role: .destructive, action: { isDeleteConfirmPresented = true }) {
                    Image(systemName: "trash")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .alert("Delete this loop?", isPresented: $isDeleteConfirmPresented) {
            Button("Delete", role: .destructive) { deleteLoop() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This can't be undone.")
        }
    }

    // MARK: - Header

    private var headerSection: some View {
        HStack(alignment: .top, spacing: 16) {
            LoopStaminaRing(loop: liveLoop, diameter: 52, lineWidth: 4)

            VStack(alignment: .leading, spacing: 10) {
                infoRow(label: "State", value: liveLoop.state.capitalized, valueColor: LoopsListView.statusColor(liveLoop.state))
                infoRow(label: "Iterations", value: "\(liveLoop.iterationLog.count) / \(liveLoop.maxIterations)")
                if let exitCondition = liveLoop.exitCondition, !exitCondition.isEmpty {
                    infoRow(label: "Exit condition", value: exitCondition)
                } else {
                    infoRow(label: "Exit condition", value: "Exit code 0 (default)")
                }
                if let updatedText = LoopsListView.relativeText(liveLoop.timeUpdated) {
                    infoRow(label: "Updated", value: updatedText)
                }
            }
        }
    }

    /// The loop's actual "interrupt": there's no permission gate on a raw
    /// shell-command loop (`LoopEngine.startLoop` just spawns `loop.command`
    /// on a timer — no agent, no approval step), but running out of its
    /// iteration budget without the exit condition firing IS the point
    /// where it stops itself and waits on a human decision. Surfacing that
    /// prominently, with a one-tap Restart, is the honest equivalent here.
    private var interruptBanner: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.subheadline)
                .foregroundColor(Theme.statusWarning)
            VStack(alignment: .leading, spacing: 2) {
                Text("Ran out of iterations")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Exit condition never fired within \(liveLoop.maxIterations) tries. Restart to try again.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 8)
        }
        .padding(12)
        .background(Theme.statusWarning.opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.statusWarning.opacity(0.35), lineWidth: 1)
        )
    }

    private func infoRow(label: String, value: String, valueColor: Color = Color("TextPrimary")) -> some View {
        HStack {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            Text(value)
                .font(.caption)
                .fontWeight(.medium)
                .foregroundColor(valueColor)
        }
    }

    // MARK: - Actions

    private var actionRow: some View {
        HStack(spacing: 12) {
            actionButton(
                icon: "arrow.clockwise",
                label: liveLoop.state == "running" ? "Running…" : "Restart",
                action: runLoop
            )
            .disabled(isMutating || liveLoop.state == "running")
        }
    }

    private func actionButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            VStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .medium))
                Text(label)
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity)
            .frame(height: 68)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusLG)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }

    // MARK: - Iterations

    /// Loop Card and the attempt ledger — only for measured loops.
    @ViewBuilder
    private var loopCardSection: some View {
        if let metric = liveLoop.metric {
            VStack(alignment: .leading, spacing: 10) {
                Text("Loop Card")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(metric.direction.capitalized) is better, keep at \(Int((metric.keepBar * 100).rounded()))%, \(metric.minPairs) pairs")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Text(metric.command)
                        .font(.caption.monospaced())
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))

                ForEach(liveLoop.ledger ?? [], id: \.iteration) { row in
                    VStack(alignment: .leading, spacing: 4) {
                        HStack {
                            Text("#\(row.iteration) \(row.verdict)")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Spacer()
                            if let delta = row.delta {
                                Text(String(format: "%+.1f%% over %d pairs", delta * 100, row.n))
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                        Text(row.why)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                }
            }
        }
    }

    private var iterationsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Iterations")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            if liveLoop.iterationLog.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "arrow.2.circlepath",
                    title: "No iterations yet",
                    message: "Loop iterations will appear here as they run."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(liveLoop.iterationLog, id: \.iteration) { entry in
                        iterationRow(entry)
                    }
                }
            }
        }
    }

    private func iterationRow(_ entry: LoopIteration) -> some View {
        let isExpanded = expandedIterations.contains(entry.iteration)
        let hasOutput = !entry.output.isEmpty
        return VStack(alignment: .leading, spacing: 6) {
            Button(action: {
                guard hasOutput else { return }
                if isExpanded {
                    expandedIterations.remove(entry.iteration)
                } else {
                    expandedIterations.insert(entry.iteration)
                }
            }) {
                HStack(spacing: 12) {
                    Circle()
                        .fill(entry.exitCode == 0 ? Theme.statusSuccess : .red)
                        .frame(width: 8, height: 8)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Iteration \(entry.iteration)")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                        Text("Exit \(entry.exitCode) · \(Self.timingText(entry))")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    if hasOutput {
                        Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .buttonStyle(.plain)

            if isExpanded, hasOutput {
                Text(entry.output)
                    .font(.caption.monospaced())
                    .foregroundColor(Color("TextSecondary"))
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("BgSecondary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    /// `LoopIteration.timestamp` is ISO-8601 (`new Date().toISOString()`,
    /// loop-engine.ts:43), unlike the loop row's own ms-epoch timestamps —
    /// parsed with the same convention AutomationTaskDetailView uses for
    /// cron's ISO run timestamps.
    private static func timingText(_ entry: LoopIteration) -> String {
        if let date = try? Date(entry.timestamp, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            let formatter = RelativeDateTimeFormatter()
            formatter.unitsStyle = .abbreviated
            return formatter.localizedString(for: date, relativeTo: Date())
        }
        return entry.timestamp
    }

    // MARK: - Actions

    private func runLoop() {
        isMutating = true
        Task {
            do {
                try await loopStore.runLoop(id: loop.id)
            } catch {
                actionError = "Couldn't restart: \(error.localizedDescription)"
            }
            isMutating = false
        }
    }

    private func deleteLoop() {
        Task {
            do {
                try await loopStore.deleteLoop(id: loop.id)
                dismiss()
            } catch {
                actionError = "Couldn't delete: \(error.localizedDescription)"
            }
        }
    }
}
