import SwiftUI

/// Cowork's flat task list (COWORK_TASKS_MAP.md's scope decision): two
/// segments — Tasks (`assigneeType != .agent`) and Agent Tasks
/// (`assigneeType == .agent`), matching `CoworkProjectView.tsx:66-74`'s own
/// filter logic. Presented as a sheet from `ComposerPlusSheet`'s new row —
/// this app has no dedicated Cowork screen to push onto — with its own
/// internal `NavigationStack` for the list → detail push, mirroring
/// `ProjectsListView`'s sheet-hosted chrome (dismiss button instead of a
/// sidebar toggle) and list/detail precedent.
struct CoworkTasksListView: View {
    @StateObject private var tasksStore = CoworkTasksStore.shared
    @Environment(\.dismiss) private var dismiss

    private enum Segment: String, CaseIterable {
        case tasks = "Tasks"
        case agentTasks = "Agent Tasks"
    }

    @State private var selectedSegment: Segment = .tasks
    @State private var detailTask: CoworkTask? = nil
    @State private var isCreateSheetPresented = false
    @State private var actionError: String? = nil

    private var visibleTasks: [CoworkTask] {
        tasksStore.tasks.filter { task in
            selectedSegment == .agentTasks
                ? task.assigneeType == .agent
                : task.assigneeType != .agent
        }
    }

    private var filterMenu: some View {
        Menu {
            ForEach(Segment.allCases, id: \.self) { segment in
                Button(action: { selectedSegment = segment }) {
                    Label(segment.rawValue, systemImage: selectedSegment == segment ? "checkmark" : "")
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
        .accessibilityLabel("Filter tasks")
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Header (matches ComposerPlusSheet / ProjectsListView chrome).
                HStack {
                    Text("Cowork Tasks")
                        .font(.system(.title3, design: .serif))
                        .fontWeight(.medium)
                        .foregroundColor(Color("TextPrimary"))
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
                    .accessibilityLabel("New task")
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

                Divider().background(Color("BorderSubtle"))

                // Decisions waiting for you (web TasksBoardScreen →
                // DecisionsDeck); takes no room when nothing waits.
                DecisionsDeckView()
                    .padding(.horizontal, 16)
                    .padding(.top, 12)

                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailTask) { task in
                CoworkTaskDetailView(task: task)
            }
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CreateCoworkTaskSheet(
                defaultAssigneeType: selectedSegment == .agentTasks ? .agent : .human
            ) { title, description, assigneeType in
                createTask(title: title, description: description, assigneeType: assigneeType)
            }
        }
        .task {
            tasksStore.fetchTasksIfNeeded()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        VStack(spacing: 0) {
            if let actionError {
                Text(actionError)
                    .font(.caption)
                    .foregroundColor(Theme.statusWarning)
                    .padding(.horizontal, 20)
                    .padding(.bottom, 8)
            }

            if tasksStore.isLoading && tasksStore.tasks.isEmpty {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError = tasksStore.loadError, tasksStore.tasks.isEmpty {
                Spacer()
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load tasks",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { tasksStore.fetchTasksIfNeeded(force: true) }
                )
                Spacer()
            } else if visibleTasks.isEmpty {
                Spacer()
                emptyState
                Spacer()
            } else {
                listContent
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "checklist",
            title: selectedSegment == .agentTasks ? "No agent tasks yet" : "No tasks yet",
            message: "Create a task to get started.",
            actionTitle: "Create task",
            action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isCreateSheetPresented = true
            }
        )
    }

    private var listContent: some View {
        ScrollView {
            LazyVStack(spacing: 8) {
                ForEach(visibleTasks) { task in
                    taskRow(task)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .refreshable {
            await tasksStore.refresh()
        }
    }

    private func taskRow(_ task: CoworkTask) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailTask = task
        }) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(task.title)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    HStack(spacing: 8) {
                        statusBadge(task.status)
                        if task.priority != 0 {
                            Text("P\(task.priority)")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
                Spacer()
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

    private func statusBadge(_ status: CoworkTaskStatus) -> some View {
        Text(status.label)
            .font(.caption)
            .fontWeight(.semibold)
            .foregroundColor(Self.statusColor(status))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Self.statusColor(status).opacity(0.15))
            .clipShape(Capsule())
    }

    /// Backlog/Todo → muted (no dedicated "muted" status token exists in
    /// Theme, so TextSecondary stands in), InProgress → info, InReview →
    /// warning (both mid-flight states, but InReview is the one waiting on a
    /// human), Done → success — the only three hues Theme actually defines
    /// (Theme.statusSuccess/Warning/Info, Color+Theme.swift:72-74).
    static func statusColor(_ status: CoworkTaskStatus) -> Color {
        switch status {
        case .backlog, .todo: return Color("TextSecondary")
        case .inProgress: return Theme.statusInfo
        case .inReview: return Theme.statusWarning
        case .done: return Theme.statusSuccess
        }
    }

    // MARK: - Actions

    private func createTask(title: String, description: String?, assigneeType: CoworkAssigneeType) {
        Task {
            do {
                try await tasksStore.create(
                    title: title,
                    description: description,
                    assigneeType: assigneeType.rawValue
                )
            } catch {
                actionError = "Couldn't create the task: \(error.localizedDescription)"
            }
        }
    }
}
