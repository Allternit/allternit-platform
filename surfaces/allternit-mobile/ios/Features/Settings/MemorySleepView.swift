import SwiftUI

// MARK: - Rules

/// What the Sleep section shows, as plain rules (web
/// `memory-sleep-client.ts`: `sleepIsOn`, `sleepView`,
/// `sleepUnavailableFrom`, `sleepReportLines`, `cannotUndoLine`,
/// `canUndoCycle`). The view keeps no rules of its own.
enum MemorySleep {
    /// Stages a person may switch on, in plain words. `orchestrator` is the
    /// "Sleep when idle" switch at the top, so it is not repeated here.
    /// `pages`, `router` and `track1` are not built yet and stay hidden.
    struct StageOption: Identifiable, Equatable {
        let stage: String
        let label: String
        let hint: String
        var id: String { stage }
    }

    static let stageOptions: [StageOption] = [
        StageOption(stage: "labels_live", label: "Learn what helped", hint: "Notes which memories were useful in a chat, so the right ones come up next time."),
        StageOption(stage: "strengths", label: "Fade and strengthen", hint: "Memories you use stay strong; ones you never use slowly fade."),
        StageOption(stage: "disputes", label: "Memory conflicts", hint: "Keeps both memories when two disagree, and asks you when it matters."),
        StageOption(stage: "checks", label: "Check old memories", hint: "Re-checks old memories and lowers the ones that no longer hold up."),
        StageOption(stage: "intentions", label: "Reminders", hint: "Turns things you meant to do into reminders."),
        StageOption(stage: "rem", label: "REM ideas", hint: "Links related memories, and keeps a link only when it helps recall."),
    ]

    /// At this debt a nap is overdue (the runtime's `debt_pause_threshold`).
    static let debtBehind: Double = 1

    /// On means both the master switch and idle cycles are on.
    static func isOn(_ status: SleepStatus) -> Bool {
        status.isEnabled && status.stage("orchestrator")
    }

    enum Shown: Equatable {
        case off, sleeping, disputes, error, debt, report, empty
    }

    /// The one state the section leads with: a running cycle, then a
    /// conflict that needs the person, then a failed last sleep, then debt,
    /// then the last sleep's report; never slept is empty.
    static func shown(_ status: SleepStatus, disputesNeedingYou: Int) -> Shown {
        guard isOn(status) else { return .off }
        let last = status.lastCycle
        if last?.state == "running" { return .sleeping }
        if disputesNeedingYou > 0 { return .disputes }
        if last?.state == "failed" { return .error }
        if status.debtValue >= debtBehind { return .debt }
        if last != nil { return .report }
        return .empty
    }

    /// Why the section can't show Sleep at all, from the status call's failure.
    enum Unavailable: Equatable {
        case noComputer
        case oldRuntime
        case computerOffline
        case signedOut
        case error(String)
    }

    static func unavailable(from error: Error) -> Unavailable {
        if let sleep = error as? MemorySleepError {
            switch sleep.kind {
            case .noComputer: return .noComputer
            // A runtime from before Sleep has no such route.
            case .notFound: return .oldRuntime
            case .computerOffline: return .computerOffline
            case .signedOut: return .signedOut
            default: return .error(sleep.message)
            }
        }
        // An older runtime that answers the path with something that isn't
        // Sleep's status (a page, an old shape).
        if case APIError.decoding = error { return .oldRuntime }
        return .error(MemorySleepError.fallback[.server] ?? CalmCopy.fallback)
    }

    static let stageNames: [String: String] = [
        "capture": "saving what was open",
        "consolidation": "merging repeats",
        "fit": "fading and strengthening",
        "dream": "dreaming",
        "rem": "linking related memories",
        "curation": "tidying bot memory",
        "pages": "writing topic pages",
        "checks": "checking old memories",
        "sign": "signing",
    ]

    /// "merging repeats": what a cycle stage is doing, in plain words.
    static func stageName(_ stage: String?) -> String {
        guard let stage, !stage.isEmpty else { return "getting started" }
        return stageNames[stage] ?? stage.replacingOccurrences(of: "_", with: " ")
    }

    static func formatUsd(_ value: Double) -> String {
        guard value.isFinite, value > 0 else { return "$0" }
        return value < 0.01 ? "<$0.01" : String(format: "$%.2f", value)
    }

    private static func plural(_ n: Int, _ one: String, _ many: String) -> String {
        "\(n) \(n == 1 ? one : many)"
    }

    /// What one stage did, as plain lines. Stages that did nothing worth
    /// saying give none.
    static func stageLines(_ s: SleepStageReport) -> [String] {
        switch s.outcome ?? "" {
        case "failed": return ["Stopped while \(stageName(s.stage))"]
        case "skipped": return s.reason == "spend_cap" ? ["Skipped \(stageName(s.stage)): daily spend limit"] : []
        case "held": return ["Left \(stageName(s.stage)) for next time"]
        case "ran": break
        default: return []
        }
        switch s.stage {
        case "capture":
            let n = s.count("captured")
            return n > 0 ? ["Saved \(plural(n, "thing", "things")) that \(n == 1 ? "was" : "were") still open"] : []
        case "consolidation":
            var out: [String] = []
            if s.count("merged") > 0 { out.append("Merged \(plural(s.count("merged"), "repeated memory", "repeated memories"))") }
            if s.count("decayed") > 0 { out.append("Faded \(plural(s.count("decayed"), "unused memory", "unused memories"))") }
            return out
        case "fit":
            return s.count("fitted") > 0 ? ["Updated how fast unused memories fade"] : []
        case "dream":
            let n = s.count("merged") + s.count("resolved") + s.count("lessons") + s.count("pruned")
            return n > 0 ? ["Dream made \(plural(n, "change", "changes"))"] : []
        case "rem":
            let n = s.count("links_kept") + s.count("lessons_kept")
            return n > 0 ? ["Kept \(plural(n, "new link", "new links")) between memories"] : []
        case "curation":
            return s.count("bots") > 0 ? ["Tidied memory for \(plural(s.count("bots"), "bot", "bots"))"] : []
        case "checks":
            let n = s.count("checked")
            return n > 0 ? ["Checked \(plural(n, "old memory", "old memories"))"] : []
        default:
            return []
        }
    }

    /// The report card's lines: what each stage did, then what it cost.
    static func reportLines(_ report: SleepCycleReport?, openConflicts: Int = 0) -> [String] {
        guard let report else { return [] }
        var lines = (report.stages ?? []).flatMap(stageLines)
        if openConflicts > 0 { lines.append("\(plural(openConflicts, "conflict", "conflicts")) waiting for you") }
        if lines.isEmpty { lines.append("Nothing needed tidying") }
        let spend = report.spendUsd ?? 0
        lines.append(spend > 0 ? "Spent \(formatUsd(spend))" : "No paid steps")
        return lines
    }

    /// What an undo leaves in place, in plain words (`cannot_undo`).
    static func cannotUndoLine(_ note: String) -> String {
        if note.hasPrefix("consolidation") { return "Merged and faded memories stay as they are." }
        if note.hasPrefix("curation") { return "Bot memory tidying stays." }
        let rest: String
        if let colon = note.firstIndex(of: ":") {
            rest = String(note[note.index(after: colon)...]).trimmingCharacters(in: .whitespaces)
        } else {
            rest = note.trimmingCharacters(in: .whitespaces)
        }
        guard let first = rest.first else { return "" }
        let ends = rest.last.map { ".!?".contains($0) } ?? false
        return first.uppercased() + String(rest.dropFirst()) + (ends ? "" : ".")
    }

    /// True when this cycle can still be undone from the report card.
    static func canUndo(_ cycle: SleepCycleSummary, report: SleepCycleReport?) -> Bool {
        if cycle.undoneAt != nil || cycle.state == "undone" { return false }
        guard cycle.state == "finished" || cycle.state == "stopped" else { return false }
        return report?.undo?.available != false
    }

    // MARK: Time

    /// Runtime timestamps are UTC, sometimes without the `T`/`Z`
    /// ("2026-11-19 10:00:00").
    static func parseTime(_ value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = iso.date(from: value) { return d }
        iso.formatOptions = [.withInternetDateTime]
        if let d = iso.date(from: value) { return d }
        let plain = DateFormatter()
        plain.locale = Locale(identifier: "en_US_POSIX")
        plain.timeZone = TimeZone(identifier: "UTC")
        for format in ["yyyy-MM-dd HH:mm:ss.SSSSSS", "yyyy-MM-dd HH:mm:ss", "yyyy-MM-dd'T'HH:mm:ss.SSSSSS", "yyyy-MM-dd'T'HH:mm:ss"] {
            plain.dateFormat = format
            if let d = plain.date(from: value) { return d }
        }
        return nil
    }

    static func timeOnly(_ date: Date) -> String {
        date.formatted(date: .omitted, time: .shortened)
    }

    /// "3:10 PM" today, "Nov 18, 3:10 PM" otherwise.
    static func timeLabel(_ value: String?, now: Date = Date()) -> String {
        guard let d = parseTime(value) else { return "" }
        let time = timeOnly(d)
        if Calendar.current.isDate(d, inSameDayAs: now) { return time }
        return "\(d.formatted(.dateTime.month(.abbreviated).day())), \(time)"
    }

    static func whenLine(_ cycle: SleepCycleSummary) -> String {
        let start = timeLabel(cycle.startedAt)
        guard !start.isEmpty else { return "Last sleep" }
        guard let end = parseTime(cycle.finishedAt) else { return "Last sleep, \(start)" }
        let endText = timeOnly(end)
        return start.hasSuffix(endText) ? "Last sleep, \(start)" : "Last sleep, \(start) to \(endText)"
    }

    static func stateLine(_ cycle: SleepCycleSummary) -> String {
        if cycle.undoneAt != nil || cycle.state == "undone" { return "Undone. Memory is back to how it was before." }
        if cycle.state == "stopped" { return "Stopped when you came back. The rest runs next time you step away." }
        return "Finished"
    }
}

// MARK: - Section

/// Settings → Memory → Sleep (between Memories and Drive), the phone shape of
/// web option A (allternit-ai `SleepSection.tsx`). Allternit Sleep tidies
/// memory on the person's own computer while they are away; this section
/// shows its real state and its switches. No new tab: it lives in Memory.
///
/// States, from `GET /memory/sleep/status`: loading, signed out, no computer,
/// old runtime (404), computer offline, error with retry, off, sleeping now,
/// conflicts that need you, last sleep failed, sleep debt, last-sleep report
/// with Undo, and empty (on, never slept). "What Sleep may do" holds the
/// stage switches that exist and the daily spend limit.
///
/// No standing poller: status reloads after every action, on pull to
/// refresh, and every 10 s only while a sleep runs and the app is active.
struct MemorySleepSection: View {
    let refreshToken: Int
    @EnvironmentObject private var authManager: AuthManager
    @Environment(\.scenePhase) private var scenePhase

    @State private var status: SleepStatus? = nil
    @State private var unavailable: MemorySleep.Unavailable? = nil
    @State private var disputes: [MemoryDispute] = []
    @State private var detail: SleepCycleDetail? = nil
    @State private var actionError: String? = nil
    /// The switch being saved (`enabled`, a stage name, or `cap`).
    @State private var saving: String? = nil
    @State private var running = false
    @State private var isConnectPresented = false

    private let client = MemoryClient()
    private static let runningPollSeconds: UInt64 = 10

    private var isRunning: Bool { status?.lastCycle?.state == "running" }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Sleep")
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                Text("While you are away, Allternit tidies your memory on your computer: it merges repeats, and with the switches below it fades what you never use and flags memories that disagree. You can undo a sleep.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !authManager.canCallAccountAPIs {
                SettingsSignedOutView(message: "Sign in to use Sleep.")
            } else {
                content
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: refreshToken) { await load() }
        .onChange(of: authManager.isSignedIn) { _, _ in Task { await load() } }
        // While a sleep runs, check again until it ends. Nothing polls otherwise.
        .task(id: isRunning && scenePhase == .active) {
            guard isRunning, scenePhase == .active else { return }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: Self.runningPollSeconds * 1_000_000_000)
                guard !Task.isCancelled else { return }
                await load()
            }
        }
        .sheet(isPresented: $isConnectPresented, onDismiss: { Task { await load() } }) {
            ConnectComputerSheet()
        }
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        switch unavailable {
        case .noComputer:
            sleepToggle(isOn: false, description: "Needs a computer.", disabled: true)
            VStack(alignment: .leading, spacing: 10) {
                Text("Memory and Sleep run on your computer. Open Allternit Desktop or pair a computer to use them.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                Button("Add a computer") {
                    Haptics.light()
                    isConnectPresented = true
                }
                .buttonStyle(.bordered)
                .frame(minHeight: 44)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgSecondary"))
            .cornerRadius(10)
        case .oldRuntime:
            mutedLine("This computer's Allternit is too old for Sleep. Update it.")
        case .signedOut:
            SettingsSignedOutView(message: "Sign in to use Sleep.")
        case .computerOffline:
            FriendlyInlineStateView(
                style: .offline,
                icon: "desktopcomputer.trianglebadge.exclamationmark",
                title: "Your computer is offline",
                message: MemorySleepError.fallback[.computerOffline] ?? "",
                actionTitle: "Try again",
                action: { Task { await load() } }
            )
        case .error(let message):
            FriendlyInlineStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load Sleep",
                message: message,
                actionTitle: "Try again",
                action: { Task { await load() } }
            )
        case nil:
            if let status {
                if status.isAvailable {
                    ready(status)
                } else {
                    mutedLine("Sleep isn't available on this computer yet. Update Allternit to turn it on.")
                }
            } else {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading Sleep…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    @ViewBuilder
    private func ready(_ status: SleepStatus) -> some View {
        let on = MemorySleep.isOn(status)
        sleepToggle(isOn: on, description: "Runs on your computer when you step away.", disabled: saving == "enabled") { value in
            Task {
                await save("enabled", value
                    ? SleepSettingsUpdate(enabled: true, stages: ["orchestrator": true])
                    : SleepSettingsUpdate(enabled: false))
            }
        }
        stateView(status)
        if let actionError {
            Text(actionError)
                .font(.caption)
                .foregroundColor(Theme.statusError)
                .fixedSize(horizontal: false, vertical: true)
        }
        if on {
            SleepStageList(status: status, saving: saving) { key, update in
                await save(key, update)
            }
        }
    }

    @ViewBuilder
    private func stateView(_ status: SleepStatus) -> some View {
        let last = status.lastCycle
        switch MemorySleep.shown(status, disputesNeedingYou: disputes.count) {
        case .off:
            mutedLine("Sleep is off. Memory still saves and recalls as before.")
        case .sleeping:
            if let last { sleepingCard(status, last) }
        case .disputes:
            SleepDisputeCard(client: client, disputes: disputes) { await load() }
            reportCard(status)
        case .error:
            if let last { failedCard(last) }
        case .debt:
            debtView(status)
        case .report:
            reportCard(status)
        case .empty:
            mutedLine("No sleeps yet. The first one runs the next time you step away from your computer. You will see what it did here.")
        }
    }

    @ViewBuilder
    private func reportCard(_ status: SleepStatus) -> some View {
        if let last = status.lastCycle, last.state != "running", last.state != "failed" {
            SleepReportCard(
                client: client,
                cycle: last,
                report: detail?.cycle.id == last.id ? detail?.report : nil,
                openConflicts: disputes.count
            ) { await load() }
        }
    }

    private func sleepingCard(_ status: SleepStatus, _ last: SleepCycleSummary) -> some View {
        let spendNote = status.dailyCap > 0
            ? "Used \(MemorySleep.formatUsd(status.spendToday)) of \(MemorySleep.formatUsd(status.dailyCap)) today."
            : "It stops by itself when you come back."
        let started = MemorySleep.timeLabel(last.startedAt)
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                ProgressView()
                Text("Sleeping now. \(capitalized(MemorySleep.stageName(last.stage))).")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(started.isEmpty ? spendNote : "Started \(started). \(spendNote)")
                .font(.caption)
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
        .accessibilityElement(children: .combine)
    }

    private func failedCard(_ last: SleepCycleSummary) -> some View {
        let why = detail?.cycle.id == last.id ? detail?.report?.error?.trimmingCharacters(in: .whitespacesAndNewlines) : nil
        return VStack(alignment: .leading, spacing: 8) {
            Text("Last sleep stopped while \(MemorySleep.stageName(last.stage)).\(why.map { $0.isEmpty ? "" : " \($0)" } ?? "")")
                .font(.subheadline)
                .foregroundColor(Theme.statusError)
                .fixedSize(horizontal: false, vertical: true)
            let when = MemorySleep.timeLabel(last.startedAt)
            if !when.isEmpty {
                Text(when).font(.caption).foregroundColor(Color("TextTertiary"))
            }
            sleepNowButton(title: "Try again")
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
    }

    private func debtView(_ status: SleepStatus) -> some View {
        let far = status.debtValue >= MemorySleep.debtBehind * 2
        let fill = min(1, max(0.08, status.debtValue / (MemorySleep.debtBehind * 2)))
        return VStack(alignment: .leading, spacing: 8) {
            Text("Allternit has taken in a lot since it last slept.")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            ProgressView(value: fill)
                .tint(Color("AccentPrimary"))
                .accessibilityLabel("Sleep debt")
                .accessibilityValue(far ? "Far behind" : "Behind")
            Text("\(far ? "Far behind." : "Behind.") Old memories stop fading until it catches up. Next sleep when you step away.")
                .font(.caption)
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
            sleepNowButton(title: "Sleep now")
        }
    }

    private func sleepNowButton(title: String) -> some View {
        Button {
            Haptics.light()
            Task { await sleepNow() }
        } label: {
            HStack(spacing: 6) {
                if running { ProgressView() }
                Text(running ? "Starting…" : title)
            }
        }
        .buttonStyle(.bordered)
        .frame(minHeight: 44)
        .disabled(running)
    }

    private func sleepToggle(isOn: Bool, description: String, disabled: Bool, onChange: ((Bool) -> Void)? = nil) -> some View {
        Toggle(isOn: Binding(get: { isOn }, set: { onChange?($0) })) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Sleep when idle")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Text(description)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .tint(Color("AccentPrimary"))
        .disabled(disabled || onChange == nil)
        .accessibilityHint(description)
    }

    private func mutedLine(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func capitalized(_ s: String) -> String {
        guard let first = s.first else { return s }
        return first.uppercased() + String(s.dropFirst())
    }

    // MARK: Loading and actions

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        do {
            let s = try await client.sleepStatus()
            let last = s.lastCycle
            let cycleId = last?.state == "running" ? nil : last?.id
            // Side reads: a failure leaves the list empty or the report on its
            // one-line summary; the status above is what decides the state.
            async let ds: [MemoryDispute] = MemorySleep.isOn(s) ? neededDisputes() : []
            async let d = cycleDetail(cycleId)
            let (list, loadedDetail) = await (ds, d)
            status = s
            disputes = list.sorted { ($0.createdAt ?? "") < ($1.createdAt ?? "") }
            detail = loadedDetail
            unavailable = nil
        } catch is CancellationError {
            // View went away mid-flight: keep what is shown.
        } catch {
            unavailable = MemorySleep.unavailable(from: error)
        }
    }

    private func neededDisputes() async -> [MemoryDispute] {
        (try? await client.memoryDisputes(state: "needs_user")) ?? []
    }

    private func cycleDetail(_ id: String?) async -> SleepCycleDetail? {
        guard let id else { return nil }
        return try? await client.sleepCycle(id: id)
    }

    @MainActor
    private func save(_ key: String, _ update: SleepSettingsUpdate) async {
        guard let current = status else { return }
        saving = key
        actionError = nil
        defer { saving = nil }
        do {
            let next = try await client.updateSleepSettings(update)
            var merged = current
            merged.enabled = next.enabled ?? current.enabled
            merged.stages = next.stages ?? current.stages
            merged.dailySpendCapUsd = next.dailySpendCapUsd ?? current.dailySpendCapUsd
            status = merged
            Haptics.selection()
            await load()
        } catch is CancellationError {
        } catch {
            actionError = error.localizedDescription
            Haptics.error()
        }
    }

    @MainActor
    private func sleepNow() async {
        running = true
        actionError = nil
        defer { running = false }
        do {
            try await client.runSleep(kind: "nap")
            await load()
        } catch is CancellationError {
        } catch {
            actionError = error.localizedDescription
            Haptics.error()
        }
    }
}

// MARK: - What Sleep may do

/// The stage switches that exist, and the daily spend limit.
private struct SleepStageList: View {
    let status: SleepStatus
    let saving: String?
    let onSave: (String, SleepSettingsUpdate) async -> Void

    @State private var expanded = false
    @State private var cap = ""
    @State private var capError: String? = nil
    @FocusState private var capFocused: Bool

    private static func capText(_ v: Double) -> String {
        v.rounded() == v ? String(Int(v)) : String(format: "%.2f", v)
    }

    private var capChanged: Bool {
        cap.trimmingCharacters(in: .whitespaces) != Self.capText(status.dailyCap)
    }

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(MemorySleep.stageOptions) { option in
                    Toggle(isOn: Binding(
                        get: { status.stage(option.stage) },
                        set: { value in Task { await onSave(option.stage, SleepSettingsUpdate(stages: [option.stage: value])) } }
                    )) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(option.label)
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                            Text(option.hint)
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .tint(Color("AccentPrimary"))
                    .disabled(saving == option.stage)
                    .padding(.vertical, 10)
                    Divider().background(Color("BorderSubtle"))
                }
                capRow
            }
            .padding(.top, 4)
        } label: {
            Text("What Sleep may do")
                .font(.subheadline.weight(.medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44, alignment: .leading)
        }
        .tint(Color("TextSecondary"))
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
        .onAppear { cap = Self.capText(status.dailyCap) }
        .onChange(of: status.dailyCap) { _, new in
            if !capFocused { cap = Self.capText(new) }
        }
        .onChange(of: capFocused) { _, focused in
            if !focused { saveCap() }
        }
    }

    private var capRow: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Daily spend limit")
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Text("Used today: \(MemorySleep.formatUsd(status.spendToday)). \(status.dailyCap > 0 ? "Sleep skips paid steps once it reaches the limit." : "At $0, Sleep skips every step that uses your model quota.")")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 10) {
                HStack(spacing: 2) {
                    Text("$").foregroundColor(Color("TextSecondary"))
                    TextField("0", text: $cap)
                        .keyboardType(.decimalPad)
                        .focused($capFocused)
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityLabel("Daily spend limit in dollars")
                        .onSubmit { saveCap() }
                }
                .font(.subheadline)
                .padding(.horizontal, 10)
                .frame(width: 110, height: 44)
                .background(Color("BgPrimary"))
                .cornerRadius(8)
                .disabled(saving == "cap")
                if capChanged {
                    Button("Save") {
                        capFocused = false
                        saveCap()
                    }
                    .buttonStyle(.bordered)
                    .disabled(saving == "cap")
                }
                if saving == "cap" { ProgressView() }
            }
            if let capError {
                Text(capError).font(.caption).foregroundColor(Theme.statusError)
            }
        }
        .padding(.vertical, 10)
    }

    private func saveCap() {
        let text = cap.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: "$", with: "")
        guard let value = Double(text.replacingOccurrences(of: ",", with: ".")), value.isFinite, value >= 0, value <= 1000 else {
            capError = "Enter an amount from $0 to $1000."
            return
        }
        capError = nil
        let rounded = (value * 100).rounded() / 100
        guard rounded != status.dailyCap else { return }
        Task { await onSave("cap", SleepSettingsUpdate(dailySpendCapUsd: rounded)) }
    }
}

// MARK: - Memory conflicts

/// Memory conflicts that need the person, one card at a time: both memories
/// and four picks (keep older, keep newer, both by context, neither). A pick
/// calls `POST /memory/disputes/:id/resolve`; the card says what was kept and
/// the next conflict takes its place.
private struct SleepDisputeCard: View {
    let client: MemoryClient
    /// Conflicts that need the person, oldest first.
    let disputes: [MemoryDispute]
    let onChanged: () async -> Void

    @State private var busy: MemoryDisputeChoice? = nil
    @State private var error: String? = nil
    @State private var saved: String? = nil

    private struct Choice: Identifiable {
        let choice: MemoryDisputeChoice
        let label: String
        let done: String
        var id: String { choice.rawValue }
    }

    private static let choices: [Choice] = [
        Choice(choice: .a, label: "Keep older", done: "Kept the older memory."),
        Choice(choice: .b, label: "Keep newer", done: "Kept the newer memory."),
        Choice(choice: .bothScoped, label: "Both, by context", done: "Kept both, each in its own context."),
        Choice(choice: .neither, label: "Neither", done: "Dropped both."),
    ]

    var body: some View {
        if let d = disputes.first {
            VStack(alignment: .leading, spacing: 10) {
                if let saved {
                    Label("Saved. \(saved)", systemImage: "checkmark")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text("These memories disagree. Which is right?")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Text(subtitle(d))
                        .font(.caption)
                        .foregroundColor(Color("TextTertiary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                side("Older", d.sideA)
                side("Newer", d.sideB)
                VStack(spacing: 8) {
                    ForEach(Self.choices) { item in
                        Button {
                            Haptics.light()
                            Task { await pick(d, item.choice, done: item.done) }
                        } label: {
                            HStack(spacing: 6) {
                                if busy == item.choice { ProgressView() }
                                Text(busy == item.choice ? "Saving…" : item.label)
                            }
                            .frame(maxWidth: .infinity, minHeight: 34)
                        }
                        .buttonStyle(.bordered)
                        .disabled(busy != nil)
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Which is right?")
                if let error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
                if disputes.count > 1 {
                    Text("1 of \(disputes.count) open. The others wait here until you get to them.")
                        .font(.caption)
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
        } else if let saved {
            Label("Saved. \(saved)", systemImage: "checkmark")
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
        }
    }

    private func subtitle(_ d: MemoryDispute) -> String {
        let reason = d.reason?.trimmingCharacters(in: .whitespacesAndNewlines)
        let why = (reason?.isEmpty == false ? reason! : "They are too close to call, so you decide.")
        let when = MemorySleep.timeLabel(d.createdAt)
        return when.isEmpty ? why : "\(why) Found \(when)."
    }

    private func side(_ chip: String, _ side: MemoryDisputeSide) -> some View {
        let text = side.text?.trimmingCharacters(in: .whitespacesAndNewlines)
        let shown = (text?.isEmpty == false ? text! : "This memory has no text.")
        return VStack(alignment: .leading, spacing: 6) {
            Text(chip)
                .font(.caption2.weight(.semibold))
                .padding(.horizontal, 7).padding(.vertical, 2)
                .background(Color("FillNeutral"))
                .foregroundColor(Color("TextSecondary"))
                .clipShape(Capsule())
            Text(shown)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            if side.current == false {
                Text("Replaced since").font(.caption).foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .cornerRadius(8)
        .accessibilityElement(children: .combine)
    }

    @MainActor
    private func pick(_ d: MemoryDispute, _ choice: MemoryDisputeChoice, done: String) async {
        busy = choice
        error = nil
        defer { busy = nil }
        do {
            try await client.resolveMemoryDispute(id: d.id, choice: choice)
            saved = done
            Haptics.success()
            await onChanged()
        } catch is CancellationError {
        } catch let err as MemorySleepError where err.reason == "already_settled" {
            // Settled elsewhere (another device, or your own correction): move on.
            await onChanged()
        } catch {
            self.error = error.localizedDescription
            Haptics.error()
        }
    }
}

// MARK: - Last sleep

/// The last sleep's report card: when it ran, what each stage did in plain
/// lines, what it cost, and Undo behind a confirm that says what an undo
/// leaves in place. A 409 from later edits lists the files.
private struct SleepReportCard: View {
    let client: MemoryClient
    let cycle: SleepCycleSummary
    /// nil while the report is loading or could not be read.
    let report: SleepCycleReport?
    let openConflicts: Int
    let onChanged: () async -> Void

    @State private var asking = false
    @State private var working = false
    @State private var error: String? = nil
    @State private var conflicts: [String] = []

    private var lines: [String] {
        if report != nil { return MemorySleep.reportLines(report, openConflicts: openConflicts) }
        if let summary = cycle.summary, !summary.isEmpty { return [summary] }
        return []
    }

    private var keeps: [String] {
        (report?.undo?.cannotUndo ?? []).map(MemorySleep.cannotUndoLine).filter { !$0.isEmpty }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(MemorySleep.whenLine(cycle))
                        .font(.subheadline.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text(MemorySleep.stateLine(cycle))
                        .font(.caption)
                        .foregroundColor(Color("TextTertiary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                .accessibilityElement(children: .combine)
                Spacer(minLength: 0)
                if MemorySleep.canUndo(cycle, report: report) {
                    Button {
                        Haptics.light()
                        asking = true
                    } label: {
                        HStack(spacing: 4) {
                            if working { ProgressView() } else { Image(systemName: "arrow.uturn.backward") }
                            Text(working ? "Undoing…" : "Undo")
                        }
                    }
                    .buttonStyle(.bordered)
                    .disabled(working)
                    .accessibilityLabel("Undo this sleep")
                }
            }
            if !lines.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(lines, id: \.self) { line in
                        Text(line)
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("What this sleep did: \(lines.joined(separator: ". "))")
            }
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
                if !conflicts.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(conflicts, id: \.self) { path in
                            Text(path)
                                .font(.caption.monospaced())
                                .foregroundColor(Color("TextSecondary"))
                                .textSelection(.enabled)
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("Files you edited since: \(conflicts.joined(separator: ", "))")
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
        .alert("Undo this sleep?", isPresented: $asking) {
            Button("Undo sleep", role: .destructive) { Task { await undo() } }
            Button("Keep", role: .cancel) {}
        } message: {
            Text(keeps.isEmpty
                 ? "Put memory back to how it was before this sleep? New memories you added since stay."
                 : "Put memory back to how it was before this sleep? New memories you added since stay.\n\nCan't be undone: \(keeps.joined(separator: " "))")
        }
    }

    @MainActor
    private func undo() async {
        working = true
        error = nil
        conflicts = []
        defer { working = false }
        do {
            try await client.undoSleepCycle(id: cycle.id)
            Haptics.success()
            await onChanged()
        } catch is CancellationError {
        } catch let err as MemorySleepError {
            if err.reason == "already_undone" {
                await onChanged()
                return
            }
            error = err.message
            conflicts = err.conflicts
            Haptics.error()
        } catch {
            self.error = error.localizedDescription
            Haptics.error()
        }
    }
}
