import Foundation
import SwiftUI

/// `bar.island` (CONTRACT section 6), default off. iOS has no remote flag
/// store, so the pill shows when the `bar.island` default is on or the app
/// is launched with `-bar-island` (the train's screenshots). `allternit://bar`
/// and "Do with Allternit" open the bar whatever the flag says: they are an
/// explicit request, and nothing runs before the review card.
enum IslandBarFlag {
    static let key = "bar.island"
    static var isOn: Bool { UserDefaults.standard.bool(forKey: key) || launchArgumentEnabled("bar-island") }
}

/// The in-app bar (Island bar + Team map, lane 32): sentence -> app chip ->
/// blanks -> review -> run -> live island -> receipt.
@MainActor
final class IslandBarModel: ObservableObject {
    static let shared = IslandBarModel()

    enum Step: Equatable {
        /// Typing the sentence; the card lists matching apps.
        case apps
        /// App picked; choose what to do in it.
        case tools
        /// Filling the blanks one at a time.
        case blanks
        /// The review card (level, rule, approver).
        case review
        /// Running or waiting: the live island.
        case live
        /// Finished: receipt, failure or cancelled.
        case done
    }

    @Published var isOpen = false
    @Published private(set) var step: Step = .apps
    @Published var query = ""
    @Published private(set) var apps: [BarAppInfo] = []
    @Published private(set) var app: BarAppInfo?
    @Published private(set) var tools: [BarToolInfo] = []
    @Published private(set) var intent: IslandIntent?
    @Published var values: [String: BarValue] = [:]
    @Published var focusedBlank: String?
    @Published private(set) var review: BarReviewInfo?
    @Published private(set) var botId: String?
    @Published private(set) var loading = false
    /// Plain wording, never a raw code.
    @Published private(set) var errorText: String?
    @Published private(set) var noComputer = false

    // Live job.
    @Published private(set) var jobId: String?
    @Published private(set) var jobState: String?
    @Published private(set) var progressLine: String?
    @Published private(set) var receipt: String?
    @Published private(set) var jobError: String?

    private let client = IslandBarClient()
    private var follow: Task<Void, Never>?
    /// Values from a link, applied once the intent's blanks are known.
    private var pendingValues: [String: String] = [:]

    private init() {}

    // MARK: Open / close

    func open() {
        isOpen = true
        if step == .apps, apps.isEmpty { Task { await loadApps() } }
    }

    /// Closes the bar. A running job keeps its live island (pill) and Live Activity.
    func close() {
        isOpen = false
        if step == .done { reset() }
    }

    /// Back to an empty sentence. Never touches a running job.
    func reset() {
        guard step != .live else { return }
        step = .apps
        query = ""
        app = nil
        tools = []
        intent = nil
        values = [:]
        focusedBlank = nil
        review = nil
        botId = nil
        errorText = nil
        noComputer = false
        jobId = nil
        jobState = nil
        progressLine = nil
        receipt = nil
        jobError = nil
        pendingValues = [:]
    }

    /// `allternit://bar?app=&tool=&bot=&v.<key>=` (DeepLinkRouter.barRequest).
    func open(with request: BarLinkRequest) {
        if step == .live {
            // A job is running: show it, don't drop it for the link.
            isOpen = true
            return
        }
        reset()
        isOpen = true
        botId = request.bot
        pendingValues = request.values
        Task {
            await loadApps()
            guard let id = request.app else { return }
            let picked = apps.first { $0.id == id }
                ?? BarAppInfo.split(id).map { BarAppInfo(kind: $0.kind, ref: $0.ref, name: $0.ref, level: "", toolCount: 0) }
            guard let picked else { return }
            await pick(app: picked, tool: request.tool)
        }
    }

    // MARK: Steps

    var matchingApps: [BarAppInfo] {
        let words = query.lowercased().split(separator: " ").map(String.init).filter { $0.count > 1 }
        guard !words.isEmpty else { return apps }
        let hits = apps.filter { a in words.contains { a.name.lowercased().contains($0) || $0.contains(a.name.lowercased()) } }
        return hits.isEmpty ? apps : hits
    }

    func loadApps() async {
        loading = true
        defer { loading = false }
        do {
            apps = try await client.apps()
            errorText = nil
        } catch {
            show(error)
        }
    }

    func pick(app: BarAppInfo, tool: String? = nil) async {
        guard app.isRunnable else {
            errorText = DoWording.notRunnable
            return
        }
        self.app = app
        errorText = nil
        loading = true
        defer { loading = false }
        do {
            let list = try await client.tools(app)
            tools = list
            let given = tool?.lowercased()
            let hit = given.flatMap { g in list.first { $0.name.lowercased() == g || $0.title.lowercased() == g } }
                ?? (list.count == 1 ? list[0] : nil)
                ?? bestTool(in: list)
            if let hit {
                await pick(tool: hit)
            } else {
                step = list.isEmpty ? .apps : .tools
                if list.isEmpty { errorText = DoWording.noTools }
            }
        } catch {
            show(error)
        }
    }

    /// The typed sentence names a tool ("refund 320 on stripe" -> Refund payment).
    private func bestTool(in list: [BarToolInfo]) -> BarToolInfo? {
        let words = Set(query.lowercased().split(separator: " ").map(String.init))
        guard !words.isEmpty else { return nil }
        return list.first { t in
            t.title.lowercased().split(separator: " ").map(String.init).contains { words.contains($0) }
        }
    }

    func pick(tool: BarToolInfo) async {
        guard let app else { return }
        loading = true
        defer { loading = false }
        do {
            let intent = try await client.intent(app: app, tool: tool.name, botId: botId)
            self.intent = intent
            // Link values first, then a number typed in the sentence for the first number blank.
            for blank in intent.blanks {
                if let raw = pendingValues[blank.key], let v = DoWording.parse(raw, for: blank) { values[blank.key] = v }
            }
            pendingValues = [:]
            if let n = query.split(separator: " ").compactMap({ Double($0.filter { $0.isNumber || $0 == "." }) }).first,
               let blank = intent.blanks.first(where: { ["number", "integer"].contains($0.type) && values[$0.key] == nil }) {
                values[blank.key] = .number(n)
            }
            step = .blanks
            focusedBlank = firstEmptyBlank ?? intent.blanks.first?.key
            errorText = nil
        } catch {
            show(error)
        }
    }

    var firstEmptyBlank: String? {
        intent?.blanks.first { $0.required && values[$0.key] == nil }?.key
    }

    var canReview: Bool { intent != nil && firstEmptyBlank == nil }

    func set(_ value: BarValue?, for key: String) {
        values[key] = value
        if value != nil, focusedBlank == key {
            focusedBlank = firstEmptyBlank ?? key
        }
    }

    func goReview() async {
        guard let intent else { return }
        loading = true
        defer { loading = false }
        do {
            review = try await client.review(intent, values: values, botId: botId)
            step = .review
            errorText = nil
        } catch {
            // `missing_fields` reads "Fill in X first."; stay on the blanks.
            show(error)
        }
    }

    func back() {
        errorText = nil
        switch step {
        case .tools: step = .apps; app = nil
        case .blanks: step = tools.count > 1 ? .tools : .apps; intent = nil; values = [:]; if step == .apps { app = nil }
        case .review: step = .blanks
        default: break
        }
    }

    // MARK: Run and follow

    func run() async {
        guard let review else { return }
        let approver = review.approverIsYou ? nil : review.approverName
        jobId = review.jobId
        loading = true
        defer { loading = false }
        do {
            let update = try await client.run(jobId: review.jobId, approverName: approver)
            step = .live
            apply(update ?? IslandJobUpdate(state: "working"))
        } catch let gate as BarWire.Gate {
            // A rule sent it to the approver; follow it from there.
            step = .live
            let name = gate.approverName ?? approver
            if let update = try? await client.job(review.jobId, approverName: name) {
                apply(update)
            } else {
                jobState = "waiting_approval"
            }
        } catch {
            show(error)
            return
        }
        startFollowing()
    }

    func stop() async {
        guard let jobId else { return }
        await client.cancel(jobId: jobId)
        if let update = try? await client.job(jobId, approverName: approverName) { apply(update) }
    }

    var approverName: String? {
        guard let review, !review.approverIsYou else { return nil }
        return review.approverName
    }

    private func startFollowing() {
        follow?.cancel()
        guard let jobId else { return }
        let approver = approverName
        follow = Task { [weak self] in
            guard let self else { return }
            do {
                for try await event in client.events(jobId: jobId, approverName: approver) {
                    self.handle(event)
                    if self.isTerminal { return }
                }
            } catch {
                // The stream dropped (relay, sleep): fall back to polling.
            }
            await self.poll(jobId: jobId, approverName: approver)
        }
    }

    private func poll(jobId: String, approverName: String?) async {
        while !Task.isCancelled, !isTerminal, self.jobId == jobId {
            try? await Task.sleep(for: .seconds(2))
            if let update = try? await client.job(jobId, approverName: approverName) { apply(update) }
        }
    }

    private func handle(_ event: IslandJobEvent) {
        switch event {
        case .job(let update): apply(update)
        case .task(let status, let message):
            if status == "working" { progressLine = message ?? progressLine }
            if jobState == "waiting_approval", status == "working" { jobState = "working" }
        case .needsInput: jobState = "input_required"
        case .inputResolved: if jobState == "input_required" { jobState = "working" }
        }
    }

    private func apply(_ update: IslandJobUpdate) {
        jobState = update.state
        if let r = update.receipt { receipt = r }
        jobError = update.error
        if isTerminal {
            step = .done
            follow?.cancel()
            Haptics.success()
        }
    }

    var isTerminal: Bool { ["completed", "failed", "cancelled"].contains(jobState ?? "") }

    /// CONTRACT section 5 wording for the live island.
    var stateLine: String {
        var content = TaskActivityAttributes.ContentState(state: jobState ?? "working")
        content.approverName = approverName
        content.receipt = receipt
        content.error = jobError
        return content.stateLine
    }

    // MARK: Errors

    private func show(_ error: Error) {
        switch error {
        case IntentFailure.noComputer:
            noComputer = true
            errorText = CalmCopy.noComputer
        case let failure as IntentFailure:
            switch failure {
            case .failed(let why): errorText = why
            default: errorText = failure.dialog
            }
        default:
            errorText = CalmCopy.fallback
        }
    }
}

extension IslandJobUpdate {
    /// A state with nothing else known (the run answered without a job body).
    init(state: String) {
        self.state = state
        receipt = nil
        error = nil
        snapshot = nil
    }
}
