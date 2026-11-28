import Foundation
import Observation

// MARK: - Rules (pure)

/// Pure rules behind the "On your computer" rows and the terminal previews in
/// the Code sessions view: a port of allternit-ai
/// src/views/code/on-this-computer.ts and
/// src/components/session-overview/terminal-overview.ts. No views, no network.
enum OnYourComputer {
    enum RowAction: String, CaseIterable, Sendable {
        case move, moveWhenIdle, copy, new

        var label: String {
            switch self {
            case .move: return "Move here"
            case .moveWhenIdle: return "Move when idle"
            case .copy: return "Open a copy"
            case .new: return "New session here"
            }
        }

        /// The adopt `mode` the action sends.
        var mode: String {
            switch self {
            case .move, .moveWhenIdle: return "move"
            case .copy: return "copy"
            case .new: return "new"
            }
        }
    }

    struct ActionSet: Equatable, Sendable {
        var actions: [RowAction]
        /// open_elsewhere: Move is refused while more than one process holds the session.
        var openElsewhere: Bool
        /// no_resume: the CLI can't reopen a session by id.
        var noResume: Bool
    }

    /// The actions a row offers, from the runtime's `movable` and `canCopy`.
    /// `here` offers none (it is already an Allternit terminal).
    static func rowActions(_ movable: String?, canCopy: Bool) -> ActionSet {
        let copy: [RowAction] = canCopy ? [.copy] : []
        switch movable {
        case "now": return ActionSet(actions: [.move] + copy + [.new], openElsewhere: false, noResume: false)
        case "busy": return ActionSet(actions: [.moveWhenIdle] + copy + [.new], openElsewhere: false, noResume: false)
        case "open_elsewhere": return ActionSet(actions: copy + [.new], openElsewhere: true, noResume: false)
        case "no_resume": return ActionSet(actions: [.new], openElsewhere: false, noResume: true)
        case "here": return ActionSet(actions: [], openElsewhere: false, noResume: false)
        default:
            // A runtime that sends no `movable` can still start a fresh session in the folder.
            return ActionSet(actions: [.new], openElsewhere: false, noResume: false)
        }
    }

    /// Sessions running outside Allternit that can be brought in.
    static func externalSessions(_ sessions: [LiveSession]) -> [LiveSession] {
        sessions.filter { $0.external && $0.movable != "here" }
    }

    private static let plainArg = Set("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_./:=@+,".unicodeScalars)

    /// One argv as a shell line: an arg with a space or a shell character is single-quoted.
    static func shellLine(_ argv: [String]) -> String {
        argv.map { arg in
            if !arg.isEmpty, arg.unicodeScalars.allSatisfy({ plainArg.contains($0) }) { return arg }
            return "'" + arg.replacingOccurrences(of: "'", with: "'\\''") + "'"
        }
        .joined(separator: " ")
    }

    /// The command a Move runs, from the runtime's own argv (`moveCommand`),
    /// quoted for display. nil on a runtime that sends none.
    static func moveCommandLine(_ argv: [String]?) -> String? {
        guard let argv, !argv.isEmpty else { return nil }
        return shellLine(argv)
    }

    /// Carried flags as chips: a flag with its values is one chip (`--model opus`).
    static func flagChips(_ flags: [String]) -> [String] {
        var chips: [String] = []
        for flag in flags {
            if flag.hasPrefix("-") || chips.isEmpty { chips.append(flag) } else { chips[chips.count - 1] += " \(flag)" }
        }
        return chips
    }

    /// "Terminal (pid 4821)".
    static func holderLabel(app: String, pid: Int) -> String { "\(app) (pid \(pid))" }

    /// "Terminal · pid 4821" for the row's detail line.
    static func appLine(_ session: LiveSession) -> String? {
        if let holder = session.holders.first { return "\(holder.app) · pid \(holder.pid)" }
        return session.pid.map { "pid \($0)" }
    }

    /// The one-writer warning, naming every holder.
    static func openElsewhereText(_ holders: [LiveSessionHolder]) -> String {
        guard holders.count >= 2 else { return "Another process may hold this session. Close it to move the session." }
        let names = holders.map { holderLabel(app: $0.app, pid: $0.pid) }.joined(separator: ", ")
        return "Open in \(holders.count) places: \(names). Close one to move it."
    }

    /// Factory panes running another CLI: rows the runtime tags with a pane and
    /// either a terminal to open, a steer route or a blocked state. An older
    /// runtime sends none of those, so nothing new appears there.
    static func paneSessions(_ sessions: [LiveSession]) -> [LiveSession] {
        sessions.filter { !$0.external && $0.paneId != nil && (hasTerminal($0) || canSteer($0) || isBlocked($0)) }
    }

    /// The rows the section shows: external sessions, then panes.
    static func rowSessions(_ sessions: [LiveSession]) -> [LiveSession] {
        externalSessions(sessions) + paneSessions(sessions)
    }

    static func isBlocked(_ session: LiveSession) -> Bool { session.status == "blocked" }

    static func hasTerminal(_ session: LiveSession) -> Bool {
        !(session.terminalId ?? "").isEmpty
    }

    /// The steer field shows for `terminal` (needs a terminal id) and `delivery` (needs an address); never for `none`.
    static func canSteer(_ session: LiveSession) -> Bool {
        switch session.steer {
        case "terminal": return hasTerminal(session)
        case "delivery": return !session.address.isEmpty
        default: return false
        }
    }

    /// "blocked 3m" for the Needs you badge; "blocked just now" under a minute.
    static func blockedLabel(since: String?, now: Date = Date()) -> String {
        guard let since, let date = parseISO(since) else { return "blocked" }
        let seconds = max(0, Int(now.timeIntervalSince(date)))
        if seconds < 60 { return "blocked just now" }
        let minutes = seconds / 60
        if minutes < 60 { return "blocked \(minutes)m" }
        let hours = minutes / 60
        if hours < 48 { return "blocked \(hours)h" }
        return "blocked \(hours / 24)d"
    }

    private static func parseISO(_ text: String) -> Date? {
        let plain = ISO8601DateFormatter()
        if let d = plain.date(from: text) { return d }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: text)
    }

    /// Where a blocked push's `data.openUrl` leads: the terminal id, or the Factory address.
    enum PushTarget: Equatable, Sendable {
        case terminal(String)
        case factorySession(String)
    }

    /// `/terminal/<terminalId>` or `/factory/sessions/<address>` (the address has a slash: `dev/allternit-claude`).
    static func pushTarget(openUrl: String?) -> PushTarget? {
        guard let openUrl else { return nil }
        let path = openUrl.split(separator: "?", maxSplits: 1).first.map(String.init) ?? openUrl
        for (prefix, make) in [("/terminal/", { PushTarget.terminal($0) }), ("/factory/sessions/", { PushTarget.factorySession($0) })] as [(String, (String) -> PushTarget)] {
            guard path.hasPrefix(prefix) else { continue }
            let rest = String(path.dropFirst(prefix.count))
            let id = (rest.removingPercentEncoding ?? rest).trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            if !id.isEmpty { return make(id) }
        }
        return nil
    }

    static func statusWord(_ status: String) -> String {
        switch status {
        case "blocked": return "Needs you"
        case "busy": return "Working"
        case "idle": return "Idle"
        case "offline": return "Offline"
        default: return status
        }
    }

    /// The row's progress line while a Move job runs.
    static func jobStateText(_ state: String) -> String? {
        switch state {
        case "waiting_idle": return "Waiting for it to go idle…"
        case "stopping": return "Stopping…"
        case "starting": return "Starting…"
        default: return nil
        }
    }

    /// Shown while a move that could no longer be cancelled finishes.
    static let tooLateNotice = "Too late to cancel; finishing the move"

    /// The confirm sheet's sentence about the old window.
    static func confirmText(app: String?, holder: String?, whenIdle: Bool) -> String {
        let lead = whenIdle ? "When it goes idle, Allternit" : "Allternit"
        let where_ = holder ?? app ?? "its window"
        return "\(lead) stops it in \(where_) and resumes it here. The \(app ?? "old") window stays open and shows the exit."
    }

    struct MoveErrorText: Equatable, Sendable {
        var text: String
        /// The command to run by hand (start_failed, session_gone), and the folder to run it in.
        var command: String? = nil
        var cwd: String? = nil
    }

    /// Pulls the "Run it by hand in <cwd>: <command>" (or "Resume it in …") tail out of a runtime message.
    private static func commandFrom(_ message: String?) -> (cwd: String, command: String)? {
        guard let message,
              let regex = try? NSRegularExpression(pattern: #"(?:Run it by hand|Resume it) in (.*?): (.+)\z"#,
                                                   options: [.dotMatchesLineSeparators]),
              let match = regex.firstMatch(in: message, range: NSRange(message.startIndex..., in: message)),
              let cwd = Range(match.range(at: 1), in: message),
              let command = Range(match.range(at: 2), in: message) else { return nil }
        return (String(message[cwd]), message[command].trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// One plain sentence per adopt error code. Unknown codes fall back to the runtime's own words.
    static func moveErrorText(code: String?, message: String? = nil) -> MoveErrorText {
        let own = message?.trimmingCharacters(in: .whitespacesAndNewlines)
        switch code {
        case "session_open_elsewhere":
            return MoveErrorText(text: "This session is open in more than one place. Close all but one, then move it.")
        case "already_here":
            return MoveErrorText(text: "This session already runs in an Allternit terminal.")
        case "adopt_in_progress":
            return MoveErrorText(text: "This session is already being moved or opened. Wait for that to finish.")
        case "holder_changed":
            return MoveErrorText(text: "The process holding this session changed, so nothing was stopped. Try again.")
        case "holder_unverified":
            return MoveErrorText(text: "Allternit couldn't confirm which process holds this session, so nothing was stopped.")
        case "no_holder":
            return MoveErrorText(text: "No running process holds this session, so there is nothing to move.")
        case "not_movable":
            return MoveErrorText(text: "This session can't move right now.")
        case "still_busy":
            return MoveErrorText(text: "It was still working after 5 minutes. Nothing was stopped; try again when it is idle.")
        case "did_not_exit":
            return MoveErrorText(text: "It didn't exit when asked to stop. Nothing was started; quit it in its own window and try again.")
        case "session_gone":
            if let hand = commandFrom(message) {
                return MoveErrorText(text: "It stopped running before it was moved. Resume it by hand:", command: hand.command, cwd: hand.cwd)
            }
            return MoveErrorText(text: "It stopped running before it was moved. Nothing was started.")
        case "too_late":
            return MoveErrorText(text: tooLateNotice)
        case "stop_refused":
            return MoveErrorText(text: "Allternit couldn't stop its process. Nothing was started.")
        case "cancelled":
            return MoveErrorText(text: "Move cancelled. Nothing was stopped.")
        case "no_resume":
            return MoveErrorText(text: "This CLI can't reopen a session by id. Use New session here instead.")
        case "no_copy":
            return MoveErrorText(text: "This CLI can't open a copy of a session.")
        case "no_cwd":
            return MoveErrorText(text: "This session's folder isn't known, so it can't open in the same place.")
        case "start_failed":
            if let hand = commandFrom(message) {
                return MoveErrorText(text: "The new terminal didn't start. Run this by hand:", command: hand.command, cwd: hand.cwd)
            }
            return MoveErrorText(text: own?.isEmpty == false ? own! : "The new terminal didn't start.")
        case "unsupported":
            return MoveErrorText(text: "Move isn't available on this computer's system yet. Nothing was stopped.")
        case "unknown_harness":
            return MoveErrorText(text: "Allternit doesn't know how to reopen this CLI.")
        case "unreachable":
            return MoveErrorText(text: "Couldn't reach this computer. Check the session in its own window before trying again.")
        default:
            return MoveErrorText(text: own?.isEmpty == false ? own! : "Something went wrong. Check the session in its own window before trying again.")
        }
    }

    private static let harnessNames = [
        "claude": "Claude Code", "codex": "Codex", "kimi": "Kimi", "agy": "Antigravity", "antigravity": "Antigravity",
        "gizzi": "Gizzi", "opencode": "opencode", "grok": "Grok", "gemini": "Gemini CLI", "cursor": "Cursor",
    ]

    /// "claude" → "Claude Code".
    static func harnessName(_ harness: String) -> String {
        harnessNames[harness.lowercased()] ?? (harness.isEmpty ? "Agent" : harness)
    }

    /// The last path segment of a folder ("~/code/app" → "app").
    static func basename(_ path: String) -> String {
        let parts = path.split(separator: "/").map(String.init)
        return parts.last ?? (path == "/" ? "/" : "")
    }

    /// The row's name: the session's own, else its folder, else the CLI.
    static func displayName(_ session: LiveSession) -> String {
        let name = session.name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !name.isEmpty { return name }
        let folder = basename(session.cwd)
        return folder.isEmpty ? harnessName(session.harness) : folder
    }
}

/// Terminal preview rules (terminal-overview.ts).
enum TerminalPreview {
    /// Rows asked of the runtime per terminal (web TERMINAL_PREVIEW_ROWS).
    static let requestRows = 12

    /// Rows a preview shows per layout on the phone: List 2, Grid 5, Deck 8.
    static func rowCount(_ layout: OverviewLayout) -> Int {
        switch layout {
        case .list: return 2
        case .grid: return 5
        case .deck: return 8
        }
    }

    /// The screen rows a preview shows: the rows above the agent's own input
    /// box (the same crop the live terminal uses), blank rows at both ends
    /// dropped, the last `max` rows kept, trailing spaces trimmed.
    static func rows(_ rows: [String]?, title: String = "", max: Int = requestRows) -> [String] {
        guard let rows, !rows.isEmpty else { return [] }
        let frame = AgentFrameParser.parse(rows, title: title)
        let kept = frame.map { Array(rows.prefix(Swift.max(0, $0.hideFrom))) } ?? rows
        let shown = kept.map(trimTrailing)
        var end = shown.count
        while end > 0, shown[end - 1].isEmpty { end -= 1 }
        var start = 0
        while start < end, shown[start].isEmpty { start += 1 }
        return Array(shown[Swift.max(start, end - max)..<end])
    }

    private static func trimTrailing(_ s: String) -> String {
        var scalars = s.unicodeScalars
        while let last = scalars.last, CharacterSet.whitespacesAndNewlines.contains(last) { scalars.removeLast() }
        return String(scalars)
    }

    enum Tone: Equatable, Sendable { case working, waiting, idle, done }

    struct Status: Equatable, Sendable {
        var word: String
        var tone: Tone
        /// The coding agent on the screen, nil for a plain shell.
        var agent: String?
    }

    /// The word and tone from the agent on the screen; a plain shell is Running or Exited.
    static func status(_ terminal: TerminalListItem) -> Status {
        if let preview = terminal.preview, !preview.isEmpty,
           let frame = AgentFrameParser.parse(preview, title: terminal.title) {
            switch frame.status {
            case .working: return Status(word: "Working", tone: .working, agent: frame.agent)
            case .needsYou: return Status(word: "Needs you", tone: .waiting, agent: frame.agent)
            case .idle: return Status(word: "Idle", tone: .idle, agent: frame.agent)
            }
        }
        return terminal.running ? Status(word: "Running", tone: .idle, agent: nil) : Status(word: "Exited", tone: .done, agent: nil)
    }

    /// The row's title: the terminal's own, else its folder.
    static func title(_ terminal: TerminalListItem) -> String {
        let title = terminal.title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !title.isEmpty { return title }
        let folder = OnYourComputer.basename(terminal.cwd)
        return folder.isEmpty ? "Terminal" : folder
    }
}

/// Polling rules (terminal-sessions.store.ts): 2 s terminals, 5 s sessions,
/// only while on screen, 15 s after 3 failures in a row, stop on 404.
enum TerminalPoll {
    enum Support: Equatable, Sendable { case yes, no, unknown }

    static let terminalSeconds: Double = 2
    static let liveSessionSeconds: Double = 5
    static let backoffSeconds: Double = 15
    static let backoffAfterFailures = 3
    static let jobSeconds: Double = 1
    static let jobMaxErrors = 5

    /// Whether an endpoint should poll right now.
    static func shouldPoll(onScreen: Bool, active: Bool, support: Support) -> Bool {
        onScreen && active && support != .no
    }

    /// Seconds before the next poll of an endpoint, given its consecutive failures.
    static func nextDelay(base: Double, failures: Int) -> Double {
        failures >= backoffAfterFailures ? Swift.max(base, backoffSeconds) : base
    }
}

// MARK: - Store

/// Terminals and live agent sessions on the connected computer, for the Code
/// sessions view (web terminal-sessions.store.ts). It polls only while the
/// view is on screen and the app is active (`setActive`), and only over the
/// relay to the user's computer, where the terminal service and the
/// live-sessions routes live. Nothing here throws into a view: an unreachable
/// or older runtime leaves the lists empty and the sections hidden.
@MainActor
@Observable
final class TerminalSessionsStore {
    private(set) var terminals: [TerminalListItem] = []
    private(set) var liveSessions: [LiveSession] = []
    private(set) var listSupport: TerminalPoll.Support = .unknown
    private(set) var adoptSupport: TerminalPoll.Support = .unknown
    /// Move jobs started here, by live session id.
    private(set) var adoptJobs: [String: AdoptJob] = [:]
    /// Copy / New in flight, by live session id.
    private(set) var pending: [String: OnYourComputer.RowAction] = [:]
    private(set) var failedOnce = false
    /// A blocked push's target, until its row takes it.
    var pushTarget: OnYourComputer.PushTarget? = nil

    /// The computer's name for the section title, when the relay knows it.
    private(set) var machineName: String? = nil

    @ObservationIgnored private var terminalClient: TerminalServiceClient?
    @ObservationIgnored private var liveClient: LiveSessionsClient?
    @ObservationIgnored private var runtimeKey: String?
    @ObservationIgnored private var resolvedAt: Date?
    @ObservationIgnored private var terminalFailures = 0
    @ObservationIgnored private var liveFailures = 0
    @ObservationIgnored private var loops: [Task<Void, Never>] = []
    @ObservationIgnored private var jobTasks: [String: Task<Void, Never>] = [:]
    @ObservationIgnored private var active = false

    /// Phone terminal size for a new / moved session (web phone 48×30).
    static let phoneSize = (cols: 48, rows: 30)

    var externalSessions: [LiveSession] { OnYourComputer.externalSessions(liveSessions) }
    /// External sessions and Factory panes: every row the section shows.
    var rowSessions: [LiveSession] { OnYourComputer.rowSessions(liveSessions) }

    /// The row a blocked push points at (terminal id, or Factory address).
    func session(for target: OnYourComputer.PushTarget) -> LiveSession? {
        switch target {
        case .terminal(let id): return liveSessions.first { $0.terminalId == id }
        case .factorySession(let address): return liveSessions.first { $0.address == address }
        }
    }

    /// Sends text to a pane: into its terminal, or as a Factory delivery. Returns an error sentence, nil on success.
    func steer(_ session: LiveSession, text: String) async -> String? {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return "Type something to send." }
        do {
            switch session.steer {
            case "terminal":
                guard let id = session.terminalId, let client = terminalClient else { return "Couldn't reach this computer." }
                try await client.input(sessionId: id, text + "\r")
            case "delivery":
                guard let client = liveClient else { return "Couldn't reach this computer." }
                try await client.sendDelivery(to: session.address, text: text)
            default:
                return "This session can't be steered from here."
            }
            Task { await pollLiveSessions() }
            return nil
        } catch {
            return "Couldn't send. Try again."
        }
    }

    /// Start (or stop) polling: the view is on screen and the app is active.
    func setActive(_ on: Bool) {
        guard on != active else { return }
        active = on
        loops.forEach { $0.cancel() }
        loops = []
        guard on else { return }
        loops = [
            Task { [weak self] in await self?.loop(terminals: true) },
            Task { [weak self] in await self?.loop(terminals: false) },
        ]
    }

    /// Pull to refresh: both lists now.
    func refresh() async {
        guard await connect() else { return }
        async let a: Void = pollTerminals()
        async let b: Void = pollLiveSessions()
        _ = await (a, b)
    }

    private func loop(terminals: Bool) async {
        while !Task.isCancelled {
            let support = terminals ? listSupport : adoptSupport
            guard TerminalPoll.shouldPoll(onScreen: true, active: active, support: support) else { return }
            if await connect() {
                if terminals { await pollTerminals() } else { await pollLiveSessions() }
            } else {
                // No relayed computer: look again at the slow cadence.
                if terminals { terminalFailures = TerminalPoll.backoffAfterFailures } else { liveFailures = TerminalPoll.backoffAfterFailures }
            }
            let delay = TerminalPoll.nextDelay(base: terminals ? TerminalPoll.terminalSeconds : TerminalPoll.liveSessionSeconds,
                                               failures: terminals ? terminalFailures : liveFailures)
            try? await Task.sleep(for: .seconds(delay))
        }
    }

    /// Resolves the computer the Code terminal uses. Only a relayed computer
    /// serves `/terminal/*` (CodeTerminalStore makes the same choice), so any
    /// other target hides both sections.
    private func connect() async -> Bool {
        guard AuthManager.shared.isSignedIn else {
            clearLists()
            runtimeKey = nil
            return false
        }
        // Reuse the resolved computer for 30 s while calls succeed; resolving
        // can ask the cloud which computer is active.
        if runtimeKey != nil, let at = resolvedAt, Date().timeIntervalSince(at) < 30,
           terminalFailures == 0, liveFailures == 0 {
            return true
        }
        guard let resolved = await InstanceConnection.resolve(from: InstanceStore.shared), resolved.isRelay else {
            clearLists()
            runtimeKey = nil
            listSupport = .unknown
            adoptSupport = .unknown
            return false
        }
        resolvedAt = Date()
        let key = resolved.baseURL.absoluteString
        if key != runtimeKey {
            runtimeKey = key
            terminalClient = TerminalServiceClient(baseURL: resolved.baseURL)
            liveClient = LiveSessionsClient(baseURL: resolved.baseURL)
            terminalFailures = 0
            liveFailures = 0
            clearLists()
            if listSupport == .no { listSupport = .unknown }
            if adoptSupport == .no { adoptSupport = .unknown }
        }
        machineName = resolved.instanceName
        return true
    }

    private func clearLists() {
        if !terminals.isEmpty { terminals = [] }
        if !liveSessions.isEmpty { liveSessions = [] }
    }

    private func pollTerminals() async {
        guard let client = terminalClient else { return }
        do {
            switch try await client.list(preview: TerminalPreview.requestRows) {
            case .unsupported:
                terminals = []
                listSupport = .no
            case .items(let rows):
                terminalFailures = 0
                if rows != terminals { terminals = rows }
                listSupport = .yes
            }
        } catch {
            guard !Task.isCancelled else { return }
            terminalFailures += 1
            failedOnce = true
            if terminalFailures >= TerminalPoll.backoffAfterFailures { terminals = [] }
        }
    }

    private func pollLiveSessions() async {
        guard let client = liveClient else { return }
        do {
            switch try await client.list() {
            case .unsupported:
                liveSessions = []
                adoptSupport = .no
            case .items(let rows):
                liveFailures = 0
                if rows != liveSessions { liveSessions = rows }
                adoptSupport = .yes
                resumeJobsFromRuntime(rows)
            }
        } catch {
            guard !Task.isCancelled else { return }
            liveFailures += 1
            failedOnce = true
            if liveFailures >= TerminalPoll.backoffAfterFailures { liveSessions = [] }
        }
    }

    /// A move started elsewhere (Desktop, the web) shows its progress here too.
    private func resumeJobsFromRuntime(_ rows: [LiveSession]) {
        for session in rows {
            guard let job = session.move, !job.jobId.isEmpty, !job.finished,
                  adoptJobs[session.sessionId] == nil else { continue }
            adoptJobs[session.sessionId] = job
            follow(session.sessionId, jobId: job.jobId, open: nil)
        }
    }

    // MARK: Adopt

    /// Move, copy or new. Copy / new return the new terminal id; a move
    /// returns nil and is followed in `adoptJobs`, calling `open` with the
    /// new terminal when it is done.
    func startAdopt(_ session: LiveSession, action: OnYourComputer.RowAction,
                    open: (@MainActor (String) -> Void)?) async -> String? {
        guard let client = liveClient else { return nil }
        adoptJobs[session.sessionId] = nil
        let isMove = action == .move || action == .moveWhenIdle
        if !isMove { pending[session.sessionId] = action }
        defer { if !isMove { pending[session.sessionId] = nil } }
        do {
            let result = try await client.adopt(sessionId: session.sessionId, address: session.address,
                                                harness: session.harness, mode: action.mode,
                                                cols: Self.phoneSize.cols, rows: Self.phoneSize.rows)
            switch result {
            case .terminal(let id):
                Task { await pollTerminals() }
                return id
            case .job(let job):
                adoptJobs[session.sessionId] = job
                follow(session.sessionId, jobId: job.jobId, open: open)
            case .cancelled:
                break
            }
        } catch let error as LiveSessionError {
            if error.unsupported { adoptSupport = .no }
            adoptJobs[session.sessionId] = AdoptJob(jobId: "", state: "failed", code: error.code, error: error.message)
        } catch {
            adoptJobs[session.sessionId] = AdoptJob(jobId: "", state: "failed", code: "unreachable",
                                                    error: "Could not reach this computer")
        }
        return nil
    }

    /// Cancel a waiting move. A `too_late` answer keeps following the job.
    func cancelAdopt(_ session: LiveSession) async {
        if let job = adoptJobs[session.sessionId], let client = liveClient {
            do {
                _ = try await client.adopt(sessionId: session.sessionId, address: session.address,
                                           harness: session.harness, mode: "cancel", cols: 80, rows: 24)
            } catch let error as LiveSessionError where error.code == "too_late" {
                if let current = adoptJobs[session.sessionId], !current.finished {
                    var noted = current
                    noted.notice = OnYourComputer.tooLateNotice
                    adoptJobs[session.sessionId] = noted
                    if jobTasks[session.sessionId] == nil, !job.jobId.isEmpty {
                        follow(session.sessionId, jobId: job.jobId, open: nil)
                    }
                }
                return
            } catch {
                // Anything else: drop the row's progress, as the web does.
            }
        }
        jobTasks[session.sessionId]?.cancel()
        jobTasks[session.sessionId] = nil
        adoptJobs[session.sessionId] = nil
    }

    /// Clears a finished job (the row's Dismiss).
    func dismissJob(_ sessionId: String) {
        jobTasks[sessionId]?.cancel()
        jobTasks[sessionId] = nil
        adoptJobs[sessionId] = nil
    }

    private func follow(_ sessionId: String, jobId: String, open: (@MainActor (String) -> Void)?) {
        jobTasks[sessionId]?.cancel()
        jobTasks[sessionId] = Task { [weak self] in
            var errors = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(TerminalPoll.jobSeconds))
                guard let self, !Task.isCancelled, let client = self.liveClient else { return }
                do {
                    var job = try await client.job(jobId)
                    errors = 0
                    job.notice = self.adoptJobs[sessionId]?.notice
                    guard !Task.isCancelled else { return }
                    self.adoptJobs[sessionId] = job
                    if job.finished {
                        self.jobTasks[sessionId] = nil
                        if job.state == "done" {
                            // Done: the session is an Allternit terminal now; open it.
                            self.adoptJobs[sessionId] = nil
                            if let terminalId = job.terminalId { open?(terminalId) }
                        }
                        await self.pollTerminals()
                        await self.pollLiveSessions()
                        return
                    }
                } catch {
                    errors += 1
                    if errors >= TerminalPoll.jobMaxErrors {
                        self.jobTasks[sessionId] = nil
                        self.adoptJobs[sessionId] = AdoptJob(jobId: jobId, state: "failed", code: "unreachable",
                                                             error: "Lost contact with this computer")
                        return
                    }
                }
            }
        }
    }
}
