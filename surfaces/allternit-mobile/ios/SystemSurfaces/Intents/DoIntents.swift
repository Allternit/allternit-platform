import AppIntents
import Foundation

// "Do with Allternit" (Island bar + Team map, lane 34): run one tool of a
// connected app from Siri, Shortcuts, Spotlight or iOS 27 "Search or Ask".
// Same flow as the in-app bar: intent -> review -> run (CONTRACT.md 1-5).
//
// - The first three required plain blanks (text, number, choice, yes/no)
//   are asked here (`value1`...`value3`). Anything else (more blanks, an
//   app picker, a blank type Siri cannot ask for, or "Review in Allternit"
//   turned on) opens the in-app bar with what is known filled in
//   (`allternit://bar?app=&tool=&bot=&v.<key>=`).
// - Every write is confirmed on the review card first. `full` level still
//   confirms (contract section 8).
// - A rule that routes the run to someone else ends here with "Sent to Noah
//   to approve"; the Live Activity follows it from there.
// - No computer (`no_runtime`): says so and opens Computers to connect one.
//
// SystemSurfaces code: system frameworks and SystemSurfaces types only. The
// calls go through `BarIntentServicing`, which the app implements over
// `APIClient` (Core/BarIntentServices.swift).

// MARK: - Wording (testable)

enum DoWording {
    static let noComputer = "Connect a computer to run this. Opening Allternit so you can connect one under Computers."
    static let notRunnable = "Apps made from artifacts can't run tools yet."
    static let noTools = "That app has no tools you can run."
    static let finishInApp = "Opening Allternit to fill in the rest."
    static let followInApp = "Follow it in Allternit."
    /// Computers (Settings > Devices), where a computer is connected.
    static let computersLink = URL(string: "allternit://computers")!

    /// The line under the review values: the rule as sent, then who acts next.
    static func ruleNote(_ review: BarReviewInfo) -> String? {
        let rule = review.ruleText?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        var next: String?
        if let name = review.approverName, !name.isEmpty, !review.approverIsYou {
            next = "You review first, then it waits for \(name)."
        }
        let parts = [rule.isEmpty ? nil : rule, next].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// The confirmation prompt.
    static func confirm(toolTitle: String, appName: String, review: BarReviewInfo) -> String {
        if let name = review.approverName, !name.isEmpty, !review.approverIsYou {
            return "\(toolTitle) on \(appName)? It goes to \(name) to approve."
        }
        return "\(toolTitle) on \(appName)?"
    }

    /// What Siri says and the result card shows.
    static func outcome(_ outcome: BarRunOutcome) -> String {
        switch outcome {
        case .completed(let receipt): return receipt
        case .working: return "Working. \(followInApp)"
        case .waitingApproval(let name):
            guard let name, !name.isEmpty else { return "Sent for approval." }
            return "Sent to \(name) to approve."
        case .needsInput: return "It needs more from you. Open Allternit to answer."
        case .failed(let why): return why.isEmpty ? "Failed." : "Failed: \(why)"
        case .cancelled: return "Cancelled."
        }
    }

    /// The question for a blank Siri asks for.
    static func ask(_ blank: BarBlankInfo) -> String {
        let label = blank.label.isEmpty ? blank.key : blank.label
        switch blank.type {
        case "number", "integer":
            if let unit = blank.unit, !unit.isEmpty { return "What \(label.lowercased()), in \(unit)?" }
            return "What \(label.lowercased())?"
        case "boolean": return "\(label)? Yes or no."
        case "enum":
            let choices = blank.options.map(\.label).joined(separator: ", ")
            return choices.isEmpty ? "Which \(label.lowercased())?" : "Which \(label.lowercased())? \(choices)."
        default: return "What \(label.lowercased())?"
        }
    }

    /// Turns spoken or typed text into the blank's typed value; nil when it
    /// does not fit (not a number, not one of the choices).
    static func parse(_ raw: String, for blank: BarBlankInfo) -> BarValue? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return nil }
        switch blank.type {
        case "number", "integer":
            let cleaned = text.filter { $0.isNumber || $0 == "." || $0 == "-" }
            return Double(cleaned).map(BarValue.number)
        case "boolean":
            switch text.lowercased() {
            case "yes", "y", "true", "on", "1": return .bool(true)
            case "no", "n", "false", "off", "0": return .bool(false)
            default: return nil
            }
        case "enum":
            let n = text.lowercased()
            let hit = blank.options.first { $0.value.lowercased() == n || $0.label.lowercased() == n }
                ?? blank.options.first { $0.label.lowercased().contains(n) || n.contains($0.label.lowercased()) }
            return hit.map { .text($0.value) }
        default:
            return .text(text)
        }
    }

    /// The in-app bar link with the app, tool and known values.
    static func barLink(app: BarAppInfo, tool: String?, values: [String: BarValue], botId: String? = nil) -> URL {
        var c = URLComponents()
        c.scheme = "allternit"
        c.host = "bar"
        // `queryItems` leaves `&`, `=` and `+` unencoded in values, so encode by hand.
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&=+#")
        func item(_ name: String, _ value: String) -> URLQueryItem {
            URLQueryItem(name: name.addingPercentEncoding(withAllowedCharacters: allowed) ?? name,
                         value: value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)
        }
        var items = [item("app", app.id)]
        if let tool, !tool.isEmpty { items.append(item("tool", tool)) }
        if let botId, !botId.isEmpty { items.append(item("bot", botId)) }
        for key in values.keys.sorted() {
            if let v = values[key] { items.append(item("v.\(key)", v.display)) }
        }
        c.percentEncodedQueryItems = items
        return c.url ?? IntentLinks.home
    }
}

// MARK: - Tool options (depend on the chosen app)

@available(iOSApplicationExtension, unavailable)
struct BarToolOptionsProvider: DynamicOptionsProvider {
    @IntentParameterDependency<DoWithAllternitIntent>(\.$app)
    var intent

    func results() async throws -> ItemCollection<String> {
        guard let app = intent?.app,
              let svc = IntentServiceRegistry.current as? any BarIntentServicing,
              let tools = try? await svc.listBarTools(appKind: app.kind, appRef: app.ref) else {
            return ItemCollection(sections: [])
        }
        let items = tools.map { tool in
            IntentItem<String>(tool.name, title: "\(tool.title)",
                               subtitle: tool.readOnly ? "Reads only" : nil)
        }
        return ItemCollection(sections: [IntentItemSection(items: items)])
    }
}

// MARK: - Do with Allternit

@available(iOSApplicationExtension, unavailable)
struct DoWithAllternitIntent: AppIntent, ForegroundContinuableIntent {
    static let title: LocalizedStringResource = "Do with Allternit"
    static let description = IntentDescription(
        "Runs a tool in one of your connected apps, like a refund in Stripe. You see a review first, and team rules still apply.",
        categoryName: "Apps",
        searchKeywords: ["run", "tool", "connection", "app", "do"],
        resultValueName: "Result")
    static let authenticationPolicy: IntentAuthenticationPolicy = .requiresAuthentication

    @Parameter(title: "App", description: "The connected app to use.")
    var app: ConnectionEntity

    @Parameter(title: "Bot", description: "The bot that does it. Leave empty to do it yourself.")
    var bot: BotEntity?

    @Parameter(title: "Tool", description: "What to do in the app.",
               optionsProvider: BarToolOptionsProvider())
    var tool: String?

    @Parameter(title: "First value", description: "The first thing the tool needs, like an order number.")
    var value1: String?

    @Parameter(title: "Second value", description: "The second thing the tool needs, like an amount.")
    var value2: String?

    @Parameter(title: "Third value", description: "The third thing the tool needs, like a reason.")
    var value3: String?

    @Parameter(title: "Review in Allternit", description: "Open the bar in Allternit to check and run it there.",
               default: false)
    var reviewInApp: Bool

    static var parameterSummary: some ParameterSummary {
        Summary("Do \(\.$tool) with \(\.$app)") {
            \.$bot
            \.$value1
            \.$value2
            \.$value3
            \.$reviewInApp
        }
    }

    init() {}

    static let maxAskedBlanks = 3

    func perform() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog & ShowsSnippetView {
        do {
            return try await run()
        } catch let failure as IntentFailure {
            throw continueInApp(for: failure)
        }
    }

    private func run() async throws -> some IntentResult & ReturnsValue<String> & ProvidesDialog & ShowsSnippetView {
        try IntentGate.check(needsComputer: false)
        guard let svc = IntentServiceRegistry.current as? any BarIntentServicing else {
            throw IntentFailure.appRequired
        }
        let appInfo = app.info
        guard appInfo.isRunnable else { throw IntentFailure.failed(DoWording.notRunnable) }

        // 1. The tool: as given, else pick from the app's tools.
        let picked = try await resolveTool(svc, app: appInfo)
        let toolName = picked.name
        if reviewInApp {
            throw openBar(app: appInfo, tool: toolName, values: givenValues(blanks: nil))
        }

        // 2. The sentence and its blanks.
        let sentence = try await svc.barIntent(appKind: appInfo.kind, appRef: appInfo.ref, tool: toolName, botId: bot?.id)
        let required = sentence.blanks.filter(\.required)
        let known = givenValues(blanks: required)
        if required.count > Self.maxAskedBlanks || required.contains(where: { !$0.isSimple }) {
            throw openBar(app: appInfo, tool: toolName, values: known)
        }
        var values: [String: BarValue] = [:]
        for (index, blank) in required.enumerated() {
            values[blank.key] = try await value(at: index, for: blank)
        }

        // 3. Review, confirm, run.
        let review: BarReviewInfo
        do {
            review = try await svc.barReview(intentId: sentence.intentId, values: values, botId: bot?.id)
        } catch IntentFailure.failed {
            // `missing_fields` or a value the server refused: finish in the bar.
            throw openBar(app: appInfo, tool: toolName, values: values)
        }
        let appName = sentence.app.name.isEmpty ? appInfo.name : sentence.app.name
        let toolTitle = sentence.toolTitle.isEmpty ? picked.title : sentence.toolTitle
        if review.needsConfirmation || !picked.readOnly {
            try await confirm(svc, review: review, appName: appName, toolTitle: toolTitle)
        }

        let outcome = try await svc.barRun(jobId: review.jobId, wait: 20)
        let text = DoWording.outcome(outcome)
        return .result(value: text, dialog: IntentSpeech.dialog(text),
                       view: DoResultSnippet(appName: appName, toolTitle: toolTitle, outcome: outcome))
    }

    private func resolveTool(_ svc: any BarIntentServicing, app: BarAppInfo) async throws -> BarToolInfo {
        let given = tool?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let tools = try await svc.listBarTools(appKind: app.kind, appRef: app.ref)
        guard !tools.isEmpty else { throw IntentFailure.failed(DoWording.noTools) }
        if !given.isEmpty {
            let n = given.lowercased()
            if let hit = tools.first(where: { $0.name.lowercased() == n || $0.title.lowercased() == n })
                ?? tools.first(where: { $0.title.lowercased().contains(n) }) {
                return hit
            }
        }
        if tools.count == 1 { return tools[0] }
        let titles = tools.map(\.title)
        let picked = try await $tool.requestDisambiguation(
            among: titles, dialog: IntentSpeech.dialog("What should I do in \(app.name)?"))
        guard let hit = tools.first(where: { $0.title == picked || $0.name == picked }) else {
            throw IntentFailure.notFound("that tool")
        }
        return hit
    }

    /// The values the person already gave, matched to blanks in order.
    private func givenValues(blanks: [BarBlankInfo]?) -> [String: BarValue] {
        guard let blanks else { return [:] }
        var out: [String: BarValue] = [:]
        for (index, blank) in blanks.prefix(Self.maxAskedBlanks).enumerated() {
            guard let raw = raw(at: index), let v = DoWording.parse(raw, for: blank) else { continue }
            out[blank.key] = v
        }
        return out
    }

    private func raw(at index: Int) -> String? {
        let v: String?
        switch index {
        case 0: v = value1
        case 1: v = value2
        case 2: v = value3
        default: v = nil
        }
        let t = v?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return t.isEmpty ? nil : t
    }

    /// The blank's value: as given, else asked for (twice at most).
    private func value(at index: Int, for blank: BarBlankInfo) async throws -> BarValue {
        if let raw = raw(at: index), let v = DoWording.parse(raw, for: blank) { return v }
        for attempt in 0..<2 {
            let prompt = attempt == 0 ? DoWording.ask(blank) : "I didn't get that. \(DoWording.ask(blank))"
            let answer: String
            if blank.type == "enum", !blank.options.isEmpty {
                answer = try await parameter(at: index).requestDisambiguation(
                    among: blank.options.map(\.label), dialog: IntentSpeech.dialog(prompt))
            } else {
                answer = try await parameter(at: index).requestValue(IntentSpeech.dialog(prompt))
            }
            if let v = DoWording.parse(answer, for: blank) { return v }
        }
        throw IntentFailure.failed("That isn't a valid \(blank.label.lowercased()).")
    }

    private func parameter(at index: Int) -> IntentParameter<String?> {
        switch index {
        case 0: return $value1
        case 1: return $value2
        default: return $value3
        }
    }

    private func confirm(_ svc: any BarIntentServicing, review: BarReviewInfo,
                         appName: String, toolTitle: String) async throws {
        let prompt = IntentSpeech.dialog(DoWording.confirm(toolTitle: toolTitle, appName: appName, review: review))
        let card = DoReviewSnippet(appName: appName, toolTitle: toolTitle, level: app.level, review: review)
        do {
            if #available(iOS 18.0, *) {
                try await requestConfirmation(actionName: .go, dialog: prompt) { card }
            } else {
                try await requestConfirmation(result: .result(dialog: prompt, view: card),
                                              confirmationActionName: .go)
            }
        } catch {
            // Declined or dismissed: the reviewed job must not linger.
            await svc.barCancel(jobId: review.jobId)
            throw error
        }
    }

    private func openBar(app: BarAppInfo, tool: String?, values: [String: BarValue]) -> Error {
        let link = DoWording.barLink(app: app, tool: tool, values: values, botId: bot?.id)
        return needsToContinueInForegroundError(IntentSpeech.dialog(DoWording.finishInApp)) {
            if let svc = IntentServiceRegistry.current { _ = await svc.openLink(link) }
        }
    }

    /// No computer opens Computers; signed out and app-only cases open the
    /// app; the rest are their own dialog.
    private func continueInApp(for failure: IntentFailure) -> Error {
        switch failure {
        case .noComputer:
            return needsToContinueInForegroundError(IntentSpeech.dialog(DoWording.noComputer)) {
                if let svc = IntentServiceRegistry.current { _ = await svc.openLink(DoWording.computersLink) }
            }
        case _ where failure.opensApp:
            return needsToContinueInForegroundError(IntentSpeech.dialog(failure.dialog))
        default:
            return failure
        }
    }
}
