import SwiftUI

/// Intelli-Schedule panel for iOS.
///
/// Fetches Cowork tasks, runs the optimizer, and displays the recommended
/// execution order with start/end times and risk indicators.
struct IntelliSchedulePanel: View {
    @StateObject private var store = IntelliScheduleStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                header
                Divider().background(Color("BorderSubtle"))
                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
        }
        .task {
            store.fetchAndOptimizeIfNeeded()
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack {
            Text("Intelli-Schedule")
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))

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
        if store.isLoading && store.tasks.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let errorMessage = store.errorMessage, store.tasks.isEmpty {
            FriendlyStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't build schedule",
                message: FriendlyErrorMessage.from(errorMessage),
                actionTitle: "Retry",
                action: { store.fetchAndOptimizeIfNeeded() }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let output = store.output, output.orderedTasks.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: "checkmark.circle",
                title: "No runnable tasks",
                message: "All tasks are blocked or already scheduled.",
                art: .bell
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let output = store.output {
            scheduleList(output: output)
        } else {
            FriendlyStateView(
                style: .empty,
                icon: "calendar.badge.clock",
                title: "No schedule yet",
                message: "Add Cowork tasks and run the optimizer to see a plan.",
                art: .calendar
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func scheduleList(output: IntelliScheduleOutput) -> some View {
        ScrollView {
            LazyVStack(spacing: 12) {
                if !output.unrunnable.isEmpty {
                    HStack {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundColor(Theme.statusWarning)
                        Text("\(output.unrunnable.count) task(s) cannot fit before their deadlines")
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                        Spacer()
                    }
                    .padding(.horizontal, 20)
                }

                ForEach(output.schedule, id: \.taskId) { entry in
                    scheduleRow(entry: entry)
                }
            }
            .padding(.vertical, 16)
        }
    }

    private func scheduleRow(entry: IntelliScheduleEntry) -> some View {
        HStack(spacing: 12) {
            riskIndicator(entry.risk)

            VStack(alignment: .leading, spacing: 4) {
                Text(store.task(withId: entry.taskId)?.title ?? entry.taskId)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)

                HStack(spacing: 6) {
                    Text(formatDate(entry.startTime))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Text("→")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Text(formatDate(entry.endTime))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }

            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(Color("BgPanel"))
        .cornerRadius(12)
        .padding(.horizontal, 20)
    }

    private func riskIndicator(_ risk: IntelliScheduleRisk) -> some View {
        let color: Color = {
            switch risk {
            case .low: return Theme.statusSuccess
            case .medium: return Theme.statusWarning
            case .high: return Theme.statusError
            }
        }()

        return Circle()
            .fill(color)
            .frame(width: 10, height: 10)
    }

    private func formatDate(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }
}
