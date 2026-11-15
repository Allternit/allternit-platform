import SwiftUI
import UIKit

/// One `###` section of an agent's HEARTBEAT.md.
struct HeartbeatFileTask: Equatable {
    var name: String
    /// "09:00", "hourly", "daily", "weekly", or a cron.
    var schedule: String
    var description: String
}

/// `.allternit/governance/HEARTBEAT.md` in an agent's workspace, the same file
/// the web's HeartbeatScheduler reads and writes.
enum HeartbeatDocument {
    static func path(agentId: String) -> String {
        "agents/\(agentId)/.allternit/governance/HEARTBEAT.md"
    }

    static let jobPrefix = "Heartbeat: "

    /// A section with no `(schedule)`: older files wrote one for any schedule
    /// without a colon, so it may only survive in the name's first word.
    private static func schedule(fromName name: String) -> String {
        let first = name.split(whereSeparator: \.isWhitespace).first?.lowercased() ?? ""
        return first == "hourly" || first == "weekly" ? first : "daily"
    }

    /// parseHeartbeatContent; a name may have several words.
    static func parse(_ content: String) -> [HeartbeatFileTask] {
        var tasks: [HeartbeatFileTask] = []
        var current: HeartbeatFileTask? = nil
        func flush() {
            if let done = current, !done.name.isEmpty {
                var task = done
                task.description = done.description.trimmingCharacters(in: .newlines)
                tasks.append(task)
            }
            current = nil
        }
        for raw in content.components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("###"), !line.hasPrefix("####") {
                flush()
                let header = line.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)
                if header.hasSuffix(")"), let open = header.lastIndex(of: "(") {
                    let name = header[..<open].trimmingCharacters(in: .whitespaces)
                    let schedule = header[header.index(after: open)...].dropLast()
                    current = HeartbeatFileTask(name: name, schedule: String(schedule), description: "")
                } else {
                    current = HeartbeatFileTask(name: header, schedule: schedule(fromName: header), description: "")
                }
            } else if line.hasPrefix("## ") || line.hasPrefix("# ") {
                flush()
            } else if line.hasPrefix("-"), current != nil {
                current?.description += line.dropFirst().trimmingCharacters(in: .whitespaces) + "\n"
            }
        }
        flush()
        return tasks
    }

    /// generateHeartbeatContent.
    static func generate(_ tasks: [HeartbeatFileTask]) -> String {
        var lines = ["# HEARTBEAT.md — Agent Periodic Tasks", "", "## Scheduled Tasks", ""]
        for task in tasks {
            let plain = task.schedule == schedule(fromName: task.name) && !task.schedule.contains(":")
            lines.append(plain ? "### \(task.name)" : "### \(task.name) (\(task.schedule))")
            lines.append("")
            for line in task.description.components(separatedBy: "\n") where !line.isEmpty {
                lines.append("- \(line)")
            }
            lines.append("")
        }
        lines += ["## Task History", "", "*Tasks will be logged here when executed*", ""]
        return lines.joined(separator: "\n")
    }

    /// taskToJobConfig: common schedules to cron.
    static func cron(from schedule: String) -> String {
        let value = schedule.trimmingCharacters(in: .whitespaces)
        switch value.lowercased() {
        case "hourly": return "0 * * * *"
        case "daily": return "0 9 * * *"
        case "weekly": return "0 9 * * 1"
        default: break
        }
        let time = value.split(separator: ":")
        if time.count == 2, let hour = Int(time[0]), let minute = Int(time[1]), time[1].count == 2 {
            return "\(minute) \(hour) * * *"
        }
        return value
    }

    /// The other way, for the file: "09:00" for a daily time, else the cron.
    static func schedule(fromCron cron: String) -> String {
        if cron == "0 * * * *" { return "hourly" }
        if cron == "0 9 * * 1" { return "weekly" }
        let f = cron.split(separator: " ").map(String.init)
        if f.count == 5, f[2] == "*", f[3] == "*", f[4] == "*", let minute = Int(f[0]), let hour = Int(f[1]) {
            return String(format: "%02d:%02d", hour, minute)
        }
        return cron
    }

    /// taskToJobConfig's prompt.
    static func prompt(name: String, description: String) -> String {
        "Execute heartbeat task: \(name)\n\n\(description)"
    }
}

// MARK: - Agent picker

/// The Agent Heartbeats tab with no agent chosen (AutomationTasksView.tsx):
/// what heartbeats are, then every agent to configure them for.
struct HeartbeatAgentPicker: View {
    let query: String
    let onPick: (AgentRecord) -> Void
    @ObservedObject private var hub = AgentHubStore.shared

    private var shown: [AgentRecord] {
        let text = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !text.isEmpty else { return hub.agents }
        return hub.agents.filter {
            $0.botDisplayName.lowercased().contains(text) || ($0.description ?? "").lowercased().contains(text)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 8) {
                Label("Agent Heartbeat Tasks", systemImage: "clock")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Configure periodic tasks that agents execute automatically. Heartbeat tasks are stored in each agent's workspace and synced with the scheduler.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))

            content
        }
        .task { hub.fetchAgentsIfNeeded() }
    }

    @ViewBuilder
    private var content: some View {
        if hub.isLoading && hub.agents.isEmpty {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Loading agents…")
            }
            .font(.system(size: 13))
            .foregroundColor(Color("TextTertiary"))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 40)
        } else if let error = hub.loadError, hub.agents.isEmpty {
            FriendlyInlineStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load agents",
                message: FriendlyErrorMessage.from(error),
                actionTitle: "Try again",
                action: { hub.fetchAgentsIfNeeded(force: true) }
            )
        } else if hub.agents.isEmpty {
            FriendlyInlineStateView(
                style: .empty,
                icon: "cpu",
                title: "No agents available",
                message: "Create an agent first to configure heartbeat tasks."
            )
        } else if shown.isEmpty {
            FriendlyInlineStateView(
                style: .empty,
                icon: "magnifyingglass",
                title: "No agents match your search",
                message: "Try another search."
            )
        } else {
            LazyVStack(spacing: 12) {
                ForEach(shown) { agent in card(agent) }
            }
        }
    }

    private func card(_ agent: AgentRecord) -> some View {
        Button { onPick(agent) } label: {
            HStack(alignment: .top, spacing: 12) {
                BotAvatarView(bot: agent, size: 40)
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 8) {
                        Text(agent.botDisplayName)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Circle()
                            .fill(agent.status == "running" ? Theme.statusSuccess : Color("TextTertiary"))
                            .frame(width: 8, height: 8)
                            .accessibilityLabel(agent.status == "running" ? "Running" : "Not running")
                    }
                    Text((agent.description ?? "").isEmpty ? "No description" : (agent.description ?? ""))
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    HStack(spacing: 12) {
                        Text(agent.type)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                        Label("Configure Heartbeats", systemImage: "clock")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .padding(.top, 4)
                }
                Spacer(minLength: 0)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Scheduler

/// One agent's heartbeats (HeartbeatScheduler.tsx): the scheduler's jobs for
/// that agent, plus anything in its HEARTBEAT.md with no job yet.
///
/// Unlike the web, each change saves at once, so there's no Save button: the
/// web's Save makes a new job for every task every time it's pressed.
struct HeartbeatSchedulerView: View {
    let agent: AgentRecord
    let onClose: () -> Void

    @ObservedObject private var cron = CronJobStore.shared
    @State private var fileTasks: [HeartbeatFileTask] = []
    /// False until the file has been read, or after a read fails. Writing
    /// then would overwrite tasks it couldn't see.
    @State private var fileLoaded = false
    @State private var isLoading = false
    @State private var error: String? = nil
    @State private var isAdding = false
    @State private var expandedId: String? = nil
    @State private var busyId: String? = nil
    @State private var pendingDelete: HeartbeatRow? = nil

    /// A task on screen: a scheduler job, or a file section not yet synced.
    private struct HeartbeatRow: Identifiable {
        let id: String
        let name: String
        let schedule: String
        let description: String
        let job: CronJob?

        var fileTask: HeartbeatFileTask {
            HeartbeatFileTask(name: name, schedule: schedule, description: description)
        }
    }

    private var jobs: [CronJob] {
        cron.jobs.filter { $0.agentConfig?.agentId == agent.id }
    }

    private static func displayName(_ job: CronJob) -> String {
        job.name.hasPrefix(HeartbeatDocument.jobPrefix)
            ? String(job.name.dropFirst(HeartbeatDocument.jobPrefix.count))
            : job.name
    }

    private static func expression(_ job: CronJob) -> String {
        switch job.schedule {
        case .cron(let value, _): return value
        case .interval(let seconds, _): return "\(seconds)s"
        case .unknown: return ""
        }
    }

    private var rows: [HeartbeatRow] {
        let synced = jobs.map { job in
            HeartbeatRow(
                id: job.id,
                name: Self.displayName(job),
                schedule: HeartbeatDocument.schedule(fromCron: Self.expression(job)),
                description: job.description ?? "",
                job: job
            )
        }
        let names = Set(synced.map(\.name))
        let unsynced = fileTasks.filter { !names.contains($0.name) }.map { task in
            HeartbeatRow(id: "file-\(task.name)", name: task.name, schedule: task.schedule, description: task.description, job: nil)
        }
        return synced + unsynced
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            agentHeader
            schedulerHeader(rows)
            if let error { errorBanner(error) }
            if isAdding {
                HeartbeatAddForm(
                    onCancel: { isAdding = false },
                    onAdd: { name, time, description, enabled in
                        await add(name: name, time: time, description: description, enabled: enabled)
                    }
                )
            }
            if isLoading && rows.isEmpty {
                ProgressView().frame(maxWidth: .infinity).padding(.vertical, 40)
            } else if rows.isEmpty {
                if let loadError = cron.loadError {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Couldn't load heartbeats",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Try again",
                        action: { Task { await load() } }
                    )
                } else {
                    emptyState
                }
            } else {
                LazyVStack(spacing: 8) {
                    ForEach(rows) { row in rowView(row) }
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Image(systemName: "gearshape")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                Text("Heartbeat tasks are stored in .allternit/governance/HEARTBEAT.md and synced with the Cron scheduler.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
        }
        .task(id: agent.id) { await load() }
        .confirmationDialog(
            "Remove \(pendingDelete?.name ?? "this task")?",
            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
            titleVisibility: .visible
        ) {
            Button("Remove", role: .destructive) {
                if let row = pendingDelete { Task { await remove(row) } }
            }
            Button("Cancel", role: .cancel) {}
        }
    }

    private var agentHeader: some View {
        HStack(spacing: 12) {
            BotAvatarView(bot: agent, size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text(agent.botDisplayName)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text("Managing heartbeat tasks")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            Spacer(minLength: 8)
            Button(action: onClose) {
                Text("Change Agent")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 36)
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    private func schedulerHeader(_ rows: [HeartbeatRow]) -> some View {
        let synced = rows.filter { $0.job != nil }.count
        return HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Heartbeat Scheduler")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(rows.count) task\(rows.count == 1 ? "" : "s") configured • \(synced) synced")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 8)
            Button { Task { await load() } } label: {
                Group {
                    if isLoading { ProgressView().controlSize(.small) } else { Image(systemName: "arrow.clockwise") }
                }
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(isLoading)
            .accessibilityLabel("Refresh")
            Button { isAdding = true } label: {
                Label("Add", systemImage: "plus")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("BgPanel"))
                    .padding(.horizontal, 12)
                    .frame(height: 36)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(isAdding)
        }
        .padding(16)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "clock")
                .font(.system(size: 36, weight: .light))
                .foregroundColor(Color("TextTertiary"))
            Text("No heartbeat tasks configured")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
            Text("Add tasks to schedule periodic actions for your agent")
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(32)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4])))
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle")
            Text(message).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button("Dismiss") { error = nil }.font(.system(size: 12))
        }
        .font(.system(size: 13))
        .foregroundColor(Theme.statusError)
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.statusError.opacity(0.08)))
    }

    @ViewBuilder
    private func rowView(_ row: HeartbeatRow) -> some View {
        let isExpanded = expandedId == row.id
        let isBusy = busyId == row.id
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Button {
                    withAnimation(.easeOut(duration: 0.2)) { expandedId = isExpanded ? nil : row.id }
                } label: {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Color("TextTertiary"))
                            .rotationEffect(.degrees(isExpanded ? 90 : 0))
                            .padding(.top, 4)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(row.name)
                                .font(.system(size: 14, weight: .medium))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            Text(statusLine(row))
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                            if !row.description.isEmpty {
                                Text(row.description)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                                    .lineLimit(1)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)

                if let job = row.job {
                    Toggle("Enabled", isOn: Binding(
                        get: { job.status != "paused" },
                        set: { value in Task { await setEnabled(row, job: job, enabled: value) } }
                    ))
                    .labelsHidden()
                    .tint(Color("AccentPrimary"))
                    .disabled(isBusy)
                    .accessibilityLabel("\(row.name) enabled")
                    iconButton("play", label: "Run now", busy: isBusy) { Task { await run(row, job: job) } }
                } else {
                    Button { Task { await sync(row) } } label: {
                        Text("Sync")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 10)
                            .frame(height: 32)
                            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                            .contentShape(Rectangle().inset(by: -6))
                    }
                    .buttonStyle(.plain)
                    .disabled(isBusy)
                }
                iconButton("trash", label: "Remove task", busy: false) { pendingDelete = row }
                    .disabled(isBusy)
            }
            .padding(12)

            if isExpanded {
                Divider().background(Color("BorderSubtle"))
                HeartbeatRowEditor(
                    name: row.name,
                    schedule: row.schedule,
                    description: row.description,
                    isSynced: row.job != nil,
                    onSave: { name, schedule, description in
                        await save(row, name: name, schedule: schedule, description: description)
                    }
                )
                .id(row.id)
                .padding(12)
            }
        }
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func statusLine(_ row: HeartbeatRow) -> String {
        guard let job = row.job else { return "Not synced · \(row.schedule)" }
        if job.status == "paused" { return "Paused" }
        return CronWords.describe(type: "cron", expression: Self.expression(job))
    }

    private func iconButton(_ icon: String, label: String, busy: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Group {
                if busy { ProgressView().controlSize(.mini) } else { Image(systemName: icon).font(.system(size: 13)) }
            }
            .foregroundColor(Color("TextSecondary"))
            .frame(width: 32, height: 32)
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle().inset(by: -6))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: - Actions

    private var filePath: String { HeartbeatDocument.path(agentId: agent.id) }

    private func load() async {
        isLoading = true
        error = nil
        defer { isLoading = false }
        await cron.refresh()
        do {
            let content = try await CapabilitiesClient.shared.read(filePath)
            fileTasks = HeartbeatDocument.parse(content ?? "")
            fileLoaded = true
        } catch is CancellationError {
            return
        } catch {
            fileLoaded = false
            self.error = "Couldn't read HEARTBEAT.md. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// Writes the file from what's on screen, so it and the scheduler agree.
    private func writeFile() async {
        guard fileLoaded else {
            error = "Saved to the scheduler, but HEARTBEAT.md wasn't updated because it couldn't be read. Refresh and try again."
            return
        }
        let tasks = rows.map(\.fileTask)
        do {
            try await CapabilitiesClient.shared.write(filePath, content: HeartbeatDocument.generate(tasks))
            fileTasks = tasks
        } catch {
            self.error = "Saved to the scheduler, but HEARTBEAT.md wasn't updated. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func createJob(name: String, cron expression: String, description: String, enabled: Bool) async throws {
        try await cron.createAgentJob(
            name: HeartbeatDocument.jobPrefix + name,
            prompt: HeartbeatDocument.prompt(name: name, description: description),
            schedule: expression,
            description: description,
            agentId: agent.id,
            status: enabled ? nil : "paused",
            maxRetries: 3,
            timeoutSeconds: 300
        )
    }

    /// Returns whether it saved, so the form knows to close.
    private func add(name: String, time: String, description: String, enabled: Bool) async -> Bool {
        error = nil
        do {
            try await createJob(name: name, cron: HeartbeatDocument.cron(from: time), description: description, enabled: enabled)
            isAdding = false
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            await writeFile()
            return true
        } catch {
            self.error = "Couldn't add \(name). \(FriendlyErrorMessage.from(error.localizedDescription))"
            return false
        }
    }

    private func sync(_ row: HeartbeatRow) async {
        busyId = row.id
        error = nil
        defer { busyId = nil }
        do {
            let expression = HeartbeatDocument.cron(from: row.schedule)
            // A job with this name and schedule and no agent already is this
            // task (the web can make one): give it the agent, don't add a second.
            if let existing = cron.jobs.first(where: { job in
                job.type == "agent"
                    && (job.agentConfig?.agentId ?? "").isEmpty
                    && job.agentConfig?.bot == nil
                    && Self.displayName(job) == row.name
                    && Self.expression(job) == expression
            }) {
                try await cron.updateJob(id: existing.id, UpdateCronJobRequest(config: ["agentId": .string(agent.id)]))
            } else {
                try await createJob(name: row.name, cron: expression, description: row.description, enabled: true)
            }
            await writeFile()
        } catch {
            self.error = "Couldn't sync \(row.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func setEnabled(_ row: HeartbeatRow, job: CronJob, enabled: Bool) async {
        busyId = row.id
        error = nil
        defer { busyId = nil }
        do {
            if enabled { try await cron.resume(id: job.id) } else { try await cron.pause(id: job.id) }
        } catch {
            self.error = "Couldn't update \(row.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func run(_ row: HeartbeatRow, job: CronJob) async {
        busyId = row.id
        error = nil
        defer { busyId = nil }
        do {
            try await cron.runNow(id: job.id)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } catch {
            self.error = "Couldn't run \(row.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func remove(_ row: HeartbeatRow) async {
        busyId = row.id
        error = nil
        defer { busyId = nil }
        do {
            if let job = row.job { try await cron.deleteJob(id: job.id) }
            fileTasks.removeAll { $0.name == row.name }
            if expandedId == row.id { expandedId = nil }
            await writeFile()
        } catch {
            self.error = "Couldn't remove \(row.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    /// Returns whether it saved, so the form can reset.
    private func save(_ row: HeartbeatRow, name: String, schedule: String, description: String) async -> Bool {
        busyId = row.id
        error = nil
        defer { busyId = nil }
        do {
            // The old name's section goes, or it would come back as unsynced.
            fileTasks.removeAll { $0.name == row.name }
            if let job = row.job {
                var request = UpdateCronJobRequest()
                if name != row.name { request.name = HeartbeatDocument.jobPrefix + name }
                if description != row.description { request.description = description }
                if schedule != row.schedule { request.schedule = HeartbeatDocument.cron(from: schedule) }
                if name != row.name || description != row.description {
                    request.config = ["prompt": .string(HeartbeatDocument.prompt(name: name, description: description))]
                }
                try await cron.updateJob(id: job.id, request)
                expandedId = job.id
            } else {
                fileTasks.append(HeartbeatFileTask(name: name, schedule: schedule, description: description))
                expandedId = "file-\(name)"
            }
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            await writeFile()
            return true
        } catch {
            fileTasks.append(row.fileTask)
            self.error = "Couldn't save \(row.name). \(FriendlyErrorMessage.from(error.localizedDescription))"
            return false
        }
    }
}

// MARK: - Forms

/// "New Heartbeat Task".
private struct HeartbeatAddForm: View {
    let onCancel: () -> Void
    let onAdd: @MainActor (_ name: String, _ time: String, _ description: String, _ enabled: Bool) async -> Bool

    @State private var name = ""
    @State private var time = Calendar.current.date(bySettingHour: 9, minute: 0, second: 0, of: Date()) ?? Date()
    @State private var description = ""
    @State private var enabled = true
    @State private var isSaving = false

    private var timeValue: String {
        let c = Calendar.current.dateComponents([.hour, .minute], from: time)
        return String(format: "%02d:%02d", c.hour ?? 9, c.minute ?? 0)
    }

    private var trimmedName: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New Heartbeat Task")
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            field("Task Name") {
                TextField("e.g., Daily Health Check", text: $name).heartbeatInput()
            }
            VStack(alignment: .leading, spacing: 4) {
                DatePicker("Schedule", selection: $time, displayedComponents: .hourAndMinute)
                    .font(.system(size: 14))
                Text("Runs every day at this time.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            field("Description / Actions") {
                TextField("What should the agent do?", text: $description, axis: .vertical)
                    .lineLimit(3...6)
                    .heartbeatInput()
            }
            Toggle("Enable this task", isOn: $enabled)
                .font(.system(size: 13))
                .tint(Color("AccentPrimary"))
            HStack(spacing: 8) {
                Spacer()
                Button("Cancel", action: onCancel)
                    .font(.system(size: 13))
                    .frame(minHeight: 36)
                Button {
                    isSaving = true
                    Task {
                        _ = await onAdd(trimmedName, timeValue, description.trimmingCharacters(in: .whitespacesAndNewlines), enabled)
                        isSaving = false
                    }
                } label: {
                    Text(isSaving ? "Adding…" : "Add Task")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("BgPanel"))
                        .padding(.horizontal, 12)
                        .frame(height: 36)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
                }
                .buttonStyle(.plain)
                .disabled(trimmedName.isEmpty || isSaving)
                .opacity(trimmedName.isEmpty ? 0.5 : 1)
            }
        }
        .padding(16)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func field<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            content()
        }
    }
}

/// A task's row opened: its name, schedule and actions.
private struct HeartbeatRowEditor: View {
    let isSynced: Bool
    let onSave: @MainActor (_ name: String, _ schedule: String, _ description: String) async -> Bool

    private let originalName: String
    private let originalSchedule: String
    private let originalDescription: String
    @State private var name: String
    @State private var schedule: String
    @State private var description: String
    @State private var isSaving = false

    init(name: String, schedule: String, description: String, isSynced: Bool,
         onSave: @escaping @MainActor (_ name: String, _ schedule: String, _ description: String) async -> Bool) {
        self.isSynced = isSynced
        self.onSave = onSave
        originalName = name
        originalSchedule = schedule
        originalDescription = description
        _name = State(initialValue: name)
        _schedule = State(initialValue: schedule)
        _description = State(initialValue: description)
    }

    private func clean(_ text: String) -> String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    private var hasChanges: Bool {
        clean(name) != originalName || clean(schedule) != originalSchedule || clean(description) != originalDescription
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            field("Task Name") {
                TextField("Task Name", text: $name).heartbeatInput()
            }
            field("Schedule") {
                TextField("09:00", text: $schedule)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .heartbeatInput()
            }
            field("Description / Actions") {
                TextField("What should the agent do?", text: $description, axis: .vertical)
                    .lineLimit(3...6)
                    .heartbeatInput()
            }
            HStack {
                Text("A time like 09:00, hourly, daily, weekly, or a cron.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                Button {
                    isSaving = true
                    Task {
                        _ = await onSave(clean(name), clean(schedule), clean(description))
                        isSaving = false
                    }
                } label: {
                    Text(isSaving ? "Saving…" : "Save")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("BgPanel"))
                        .padding(.horizontal, 12)
                        .frame(height: 36)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
                }
                .buttonStyle(.plain)
                .disabled(!hasChanges || isSaving || clean(name).isEmpty || clean(schedule).isEmpty)
                .opacity(hasChanges && !clean(name).isEmpty && !clean(schedule).isEmpty ? 1 : 0.5)
            }
        }
    }

    private func field<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            content()
        }
    }
}

private extension View {
    func heartbeatInput() -> some View {
        font(.system(size: 14))
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPrimary")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}
