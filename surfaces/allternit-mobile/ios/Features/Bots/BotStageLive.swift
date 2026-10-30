import SwiftUI

// The live stage's working cards (allternit-ai src/components/bots/stage/
// live/stage-model.ts + BotLiveStage.tsx): the step list while the bot works,
// "Since you were last here", the montage of what it made, and the next
// routine. Worked out from the bot's threads, routines and saved artifacts.

/// When you last opened a bot's chat (web bot-activity-watermark: the
/// briefing covers what moved after it). Kept per bot on this phone.
enum BotStageSeen {
    private static func key(_ botId: String) -> String { "allternit.bot.seenAt.\(botId)" }

    /// Epoch ms, nil before the first visit (no briefing then, as on the web).
    static func seenAt(_ botId: String) -> Double? {
        let value = UserDefaults.standard.double(forKey: key(botId))
        if value > 0 { return value }
        #if DEBUG
        // The sample workspace has been away two hours, so the briefing shows.
        if SampleWorkspace.isEnabled { return (Date().timeIntervalSince1970 - 2 * 3600) * 1000 }
        #endif
        return nil
    }

    static func markSeen(_ botId: String) {
        UserDefaults.standard.set(Date().timeIntervalSince1970 * 1000, forKey: key(botId))
    }
}

/// "Since you were last here": the bot's other threads that moved, and its routine runs.
struct StageBriefing: Equatable {
    let line: String
    let threads: [BotThread]
}

/// One frame of the montage: a saved artifact, or a finished step.
struct StageMontageFrame: Identifiable, Equatable {
    let id: String
    let title: String
    let detail: String
    /// The artifact to open; nil for a finished step.
    var artifact: ArtifactV2Summary? = nil
    /// The thread it came from, when not this one.
    var thread: String? = nil
}

enum BotStageLiveModel {
    private static func ms(_ iso: String?) -> Double {
        guard let iso, let date = ArtifactV2Dates.parse(iso) else { return 0 }
        return date.timeIntervalSince1970 * 1000
    }

    /// The step list shows while something is unfinished (`visibleSteps`).
    static func visibleSteps(_ todo: [ThreadTodo]?) -> [ThreadTodo] {
        let steps = todo ?? []
        return steps.contains { $0.state != "done" } ? steps : []
    }

    /// `briefing`: threads (not this one) active after `since`, and routines that ran.
    static func briefing(threads: [BotThread], routines: [AutomationRoutine], since: Double?, currentThreadId: String?) -> StageBriefing? {
        guard let since else { return nil }
        let moved = threads
            .filter { $0.id != currentThreadId && !$0.incognito && ms($0.lastActivityAt) > since }
            .sorted { ms($0.lastActivityAt) > ms($1.lastActivityAt) }
        let ran = routines.filter { ms($0.lastRunAt) > since }
        guard !moved.isEmpty || !ran.isEmpty else { return nil }
        var parts: [String] = []
        if !moved.isEmpty { parts.append("worked on \(moved.count) \(moved.count == 1 ? "thread" : "threads")") }
        if !ran.isEmpty {
            let failed = ran.filter { ["failed", "error", "failure"].contains($0.recentRuns?.first?.status ?? "") }.count
            parts.append("ran \(ran.count) \(ran.count == 1 ? "routine" : "routines")\(failed > 0 ? " (\(failed) failed)" : "")")
        }
        return StageBriefing(line: "Since you were last here I \(parts.joined(separator: " and ")).", threads: Array(moved.prefix(4)))
    }

    private static func detail(_ kind: String) -> String {
        let k = kind.lowercased()
        if k.contains("image") || k.contains("svg") || k.contains("png") || k.contains("jpeg") { return "Image" }
        if k.contains("html") || k.contains("site") || k.contains("web") || k.contains("page") || k.contains("design") { return "Page" }
        if k.contains("slide") || k.contains("deck") { return "Slides" }
        if k.contains("sheet") || k.contains("csv") || k.contains("table") { return "Sheet" }
        if k.contains("code") || k.contains("script") { return "Code" }
        return "Document"
    }

    /// `montageFrames`: what the bot saved (oldest first), then the steps it finished; the last ten.
    static func montage(saved: [(artifact: ArtifactV2Summary, thread: String?)], todo: [ThreadTodo]?, max: Int = 10) -> [StageMontageFrame] {
        var frames = saved
            .sorted { ms($0.artifact.updatedAt) < ms($1.artifact.updatedAt) }
            .map { StageMontageFrame(id: "saved:\($0.artifact.id)", title: $0.artifact.title.isEmpty ? "Untitled" : $0.artifact.title,
                                     detail: detail($0.artifact.kind), artifact: $0.artifact, thread: $0.thread) }
        for step in todo ?? [] where step.state == "done" {
            frames.append(StageMontageFrame(id: "step:\(step.text)", title: step.text, detail: "Step done"))
        }
        return Array(frames.suffix(max))
    }

    /// `nextRoutineLabel`, capitalised for the stage: the nearest enabled routine.
    static func nextUp(_ routines: [AutomationRoutine], now: Date = Date()) -> String? {
        let next = routines
            .filter { $0.isEnabled && ms($0.nextRunAt) > 0 }
            .min { ms($0.nextRunAt) < ms($1.nextRunAt) }
        guard let next else { return nil }
        let delta = ms(next.nextRunAt) - now.timeIntervalSince1970 * 1000
        let label: String
        if delta <= 0 {
            label = "Routine \"\(next.name)\" due"
        } else {
            let minute = 60_000.0, hour = 60 * minute, day = 24 * hour
            let when = delta < minute ? "in <1m"
                : delta < hour ? "in \(Int((delta / minute).rounded()))m"
                : delta < day ? "in \(String(format: "%.1f", delta / hour))h"
                : "in \(Int((delta / day).rounded()))d"
            label = "Next routine \"\(next.name)\" \(when)"
        }
        return label
    }
}

/// What the stage loads beyond the threads the store already has: the bot's
/// routines and the artifacts saved from its sessions.
@MainActor
final class BotStageLiveStore: ObservableObject {
    @Published private(set) var routines: [AutomationRoutine] = []
    @Published private(set) var saved: [(artifact: ArtifactV2Summary, thread: String?)] = []
    /// Read once when the chat opens, before it's marked seen.
    @Published private(set) var seenAt: Double? = nil
    private var loadedBot: String? = nil

    func open(botId: String) {
        guard loadedBot != botId else { return }
        loadedBot = botId
        seenAt = BotStageSeen.seenAt(botId)
        BotStageSeen.markSeen(botId)
    }

    /// `sessions`: session id → the thread's title (nil for the thread on screen).
    func load(botId: String, sessions: [String: String?]) async {
        if let rows = try? await AutomationClient.shared.listRoutines(recentRuns: 1) {
            routines = rows.filter { $0.agentId == botId }
        }
        guard !sessions.isEmpty,
              let page = try? await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(scope: "mine", limit: 100)) else { return }
        saved = page.items
            .compactMap { item -> (artifact: ArtifactV2Summary, thread: String?)? in
                guard let session = item.origin?.sessionId, let thread = sessions[session] else { return nil }
                return (item, thread)
            }
            .prefix(12)
            .map { $0 }
    }
}

// MARK: - Cards

/// "Working · step 2 of 3" (or "Plan"): the checklist, ticked as it goes.
struct StageStepsCard: View {
    let steps: [ThreadTodo]
    let working: Bool
    let accent: Color

    private var label: String {
        guard working else { return "Plan" }
        let done = steps.filter { $0.state == "done" }.count
        return "Working · step \(min(done + 1, steps.count)) of \(steps.count)"
    }

    var body: some View {
        StageCard(label: label) {
            VStack(alignment: .leading, spacing: 7) {
                ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Group {
                            switch step.state {
                            case "done": Image(systemName: "checkmark.circle.fill").foregroundColor(accent)
                            case "active": Circle().fill(accent).frame(width: 8, height: 8).frame(width: 15)
                            default: Image(systemName: "circle").foregroundColor(Color("TextTertiary"))
                            }
                        }
                        .font(.system(size: 13))
                        Text(step.text)
                            .font(.system(size: 13.5, weight: step.state == "active" ? .semibold : .regular))
                            .foregroundColor(step.state == "done" ? Color("TextSecondary") : Color("TextPrimary"))
                            .strikethrough(step.state == "done", color: Color("TextTertiary"))
                    }
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// "Since you were last here", with the threads that moved.
struct StageBriefingCard: View {
    let briefing: StageBriefing
    let onOpen: (BotThread) -> Void

    var body: some View {
        StageCard(label: "Since you were last here") {
            Text(briefing.line)
                .font(.system(size: 13.5))
                .foregroundColor(Color("TextPrimary"))
            ForEach(briefing.threads) { thread in
                Button { onOpen(thread) } label: {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(thread.title).font(.system(size: 13.5, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                        Text(thread.statusLine ?? thread.summary ?? thread.status)
                            .font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(thread.currentSessionId == nil)
            }
        }
    }
}

/// The montage: what the bot made, one frame every 2.4 s with ticks under
/// it. A tap on a tick jumps there; a tap on a saved artifact opens it.
struct StageMontageCard: View {
    let frames: [StageMontageFrame]
    let working: Bool
    let onOpen: (ArtifactV2Summary) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var index = 0
    private static let interval: TimeInterval = 2.4

    private var frame: StageMontageFrame? { frames.isEmpty ? nil : frames[min(index, frames.count - 1)] }

    var body: some View {
        StageCard(label: working ? "So far" : "What I made") {
            if let frame {
                Button { if let artifact = frame.artifact { onOpen(artifact) } } label: {
                    HStack(spacing: 12) {
                        thumb(frame)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(frame.title).font(.system(size: 13.5, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(2)
                            Text(frame.thread.map { "\(frame.detail) · \($0)" } ?? frame.detail)
                                .font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                        }
                        Spacer(minLength: 0)
                    }
                    .id(frame.id)
                    .transition(reduceMotion ? .opacity : .opacity.combined(with: .scale(scale: 1.04)))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(frame.artifact == nil)
                .accessibilityLabel(frame.artifact != nil ? "Open \(frame.title)" : frame.title)
                if frames.count > 1 { ticks }
            }
        }
        .onChange(of: frames.map(\.id)) { _, _ in index = 0 }
        .task(id: "\(index)|\(frames.count)") {
            guard frames.count > 1 else { return }
            try? await Task.sleep(nanoseconds: UInt64(Self.interval * 1_000_000_000))
            guard !Task.isCancelled else { return }
            withAnimation(.easeOut(duration: reduceMotion ? 0.12 : 0.5)) { index = (index + 1) % frames.count }
        }
    }

    private func thumb(_ frame: StageMontageFrame) -> some View {
        Group {
            if let artifact = frame.artifact {
                Image(ArtifactV2Kind.iconAsset(for: artifact.kind)).resizable().frame(width: 30, height: 30)
            } else {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach([1.0, 0.8, 0.55], id: \.self) { w in
                        Capsule().fill(Color("TextTertiary").opacity(0.35)).frame(width: 30 * w, height: 3)
                    }
                }
            }
        }
        .frame(width: 52, height: 52)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private var ticks: some View {
        HStack(spacing: 4) {
            ForEach(Array(frames.enumerated()), id: \.element.id) { i, f in
                Button { withAnimation(.easeOut(duration: 0.2)) { index = i } } label: {
                    Capsule()
                        .fill(i <= index ? Color("TextPrimary").opacity(0.7) : Color("TextTertiary").opacity(0.3))
                        .frame(height: 3)
                        .frame(maxWidth: .infinity, minHeight: 20)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(f.title)
                .accessibilityAddTraits(i == index ? .isSelected : [])
            }
        }
    }
}

/// The next routine, under the montage ("Next routine "x" in 2h").
struct StageNextUpCard: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "calendar").font(.system(size: 13))
            Text(text).font(.system(size: 13)).lineLimit(2)
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle")))
    }
}
