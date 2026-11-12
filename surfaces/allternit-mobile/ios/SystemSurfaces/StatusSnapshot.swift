import Foundation

/// The runtime-owned `status.snapshot` (plan REVISION 2), as the cloud's
/// `GET /api/v1/status/summary` returns it. Every field is optional or
/// defaulted so an older or partial payload still decodes. Compiled into the
/// app and every extension: no app-only imports.
struct StatusSnapshot: Codable, Sendable, Equatable {
    var activeBots: Int = 0
    var attentionBots: Int = 0
    var pendingApprovals: Int = 0
    var paused: Bool = false
    var focus: Focus?
    var bots: [Bot] = []
    var needsYou: [String] = []
    var needsYouRows: [NeedsYouRow] = []
    var loops: [Loop] = []
    var lastRoutine: LastRoutine?
    /// The phone or computer run the live pill follows (live-status-pill contract); absent when
    /// no run is live or ended in the last 10 minutes.
    var phoneRun: PhoneRun?
    /// Set locally when the snapshot is stored; not part of the wire payload.
    var asOf: Date?

    struct Focus: Codable, Sendable, Equatable {
        var botId: String = ""
        var botName: String = ""
        var status: String = ""
        var step: String = ""
        var progress: Double?
        var inboxId: String?
        var sessionId: String?
        var question: String?
        var contentHash: String?
        var approveInline: Bool = false
        var `private`: Bool = false

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            botId = (try? c.decodeIfPresent(String.self, forKey: .botId)) ?? ""
            botName = (try? c.decodeIfPresent(String.self, forKey: .botName)) ?? ""
            status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? ""
            step = (try? c.decodeIfPresent(String.self, forKey: .step)) ?? ""
            progress = try? c.decodeIfPresent(Double.self, forKey: .progress)
            inboxId = try? c.decodeIfPresent(String.self, forKey: .inboxId)
            sessionId = try? c.decodeIfPresent(String.self, forKey: .sessionId)
            question = try? c.decodeIfPresent(String.self, forKey: .question)
            contentHash = try? c.decodeIfPresent(String.self, forKey: .contentHash)
            approveInline = (try? c.decodeIfPresent(Bool.self, forKey: .approveInline)) ?? false
            `private` = (try? c.decodeIfPresent(Bool.self, forKey: .private)) ?? false
        }
    }

    struct Bot: Codable, Sendable, Equatable {
        var botId: String = ""
        var name: String = ""
        var status: String = ""
        var step: String = ""

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            botId = (try? c.decodeIfPresent(String.self, forKey: .botId)) ?? ""
            name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
            status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? ""
            step = (try? c.decodeIfPresent(String.self, forKey: .step)) ?? ""
        }
    }

    struct NeedsYouRow: Codable, Sendable, Equatable {
        var id: String = ""
        var kind: String = ""
        var botName: String = ""
        var line: String = ""

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = (try? c.decodeIfPresent(String.self, forKey: .id)) ?? ""
            kind = (try? c.decodeIfPresent(String.self, forKey: .kind)) ?? ""
            botName = (try? c.decodeIfPresent(String.self, forKey: .botName)) ?? ""
            line = (try? c.decodeIfPresent(String.self, forKey: .line)) ?? ""
        }
    }

    struct Loop: Codable, Sendable, Equatable {
        var loopId: String = ""
        var command: String = ""
        var iterationsCompleted: Int = 0
        var maxIterations: Int?
        var state: String = ""

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            loopId = (try? c.decodeIfPresent(String.self, forKey: .loopId)) ?? ""
            command = (try? c.decodeIfPresent(String.self, forKey: .command)) ?? ""
            iterationsCompleted = (try? c.decodeIfPresent(Int.self, forKey: .iterationsCompleted)) ?? 0
            maxIterations = try? c.decodeIfPresent(Int.self, forKey: .maxIterations)
            state = (try? c.decodeIfPresent(String.self, forKey: .state)) ?? ""
        }
    }

    struct LastRoutine: Codable, Sendable, Equatable {
        var routineId: String = ""
        var title: String = ""
        var status: String = ""
        /// Unix seconds.
        var at: Double?

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            routineId = (try? c.decodeIfPresent(String.self, forKey: .routineId)) ?? ""
            title = (try? c.decodeIfPresent(String.self, forKey: .title)) ?? ""
            status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? ""
            at = try? c.decodeIfPresent(Double.self, forKey: .at)
        }
    }

    struct PhoneRun: Codable, Sendable, Equatable {
        var runId: String = ""
        /// `phone` or `computer`.
        var kind: String = "phone"
        var goal: String = ""
        /// A `phone_task` card status: running, waiting_approval, done, denied, approval_timeout, …
        var status: String = ""
        var step: Int = 0
        var stepLabel: String?
        /// The last 6 finished steps.
        var trail: [TrailStep] = []
        var summary: String?
        var inboxId: String?

        struct TrailStep: Codable, Sendable, Equatable {
            var n: Int = 0
            var label: String = ""
            var ok: Bool?

            init(n: Int, label: String, ok: Bool? = nil) { self.n = n; self.label = label; self.ok = ok }
            init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                n = (try? c.decodeIfPresent(Int.self, forKey: .n)) ?? 0
                label = (try? c.decodeIfPresent(String.self, forKey: .label)) ?? ""
                ok = try? c.decodeIfPresent(Bool.self, forKey: .ok)
            }
        }

        init() {}
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            runId = (try? c.decodeIfPresent(String.self, forKey: .runId)) ?? ""
            kind = (try? c.decodeIfPresent(String.self, forKey: .kind)) ?? "phone"
            goal = (try? c.decodeIfPresent(String.self, forKey: .goal)) ?? ""
            status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? ""
            step = (try? c.decodeIfPresent(Int.self, forKey: .step)) ?? 0
            stepLabel = try? c.decodeIfPresent(String.self, forKey: .stepLabel)
            // One malformed entry must not drop the whole trail.
            let raw = (try? c.decodeIfPresent([TrailStep?].self, forKey: .trail)) ?? []
            trail = raw.compactMap { $0 }.filter { !$0.label.isEmpty }.sorted { $0.n < $1.n }
            summary = try? c.decodeIfPresent(String.self, forKey: .summary)
            inboxId = try? c.decodeIfPresent(String.self, forKey: .inboxId)
        }
    }

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        activeBots = (try? c.decodeIfPresent(Int.self, forKey: .activeBots)) ?? 0
        attentionBots = (try? c.decodeIfPresent(Int.self, forKey: .attentionBots)) ?? 0
        pendingApprovals = (try? c.decodeIfPresent(Int.self, forKey: .pendingApprovals)) ?? 0
        paused = (try? c.decodeIfPresent(Bool.self, forKey: .paused)) ?? false
        focus = try? c.decodeIfPresent(Focus.self, forKey: .focus)
        bots = (try? c.decodeIfPresent([Bot].self, forKey: .bots)) ?? []
        needsYou = (try? c.decodeIfPresent([String].self, forKey: .needsYou)) ?? []
        needsYouRows = (try? c.decodeIfPresent([NeedsYouRow].self, forKey: .needsYouRows)) ?? []
        loops = (try? c.decodeIfPresent([Loop].self, forKey: .loops)) ?? []
        lastRoutine = try? c.decodeIfPresent(LastRoutine.self, forKey: .lastRoutine)
        phoneRun = try? c.decodeIfPresent(PhoneRun.self, forKey: .phoneRun)
        asOf = try? c.decodeIfPresent(Date.self, forKey: .asOf)
    }

    /// A copy safe to persist: a private focus never keeps its question text.
    func redactedForStorage() -> StatusSnapshot {
        var copy = self
        if copy.focus?.private == true { copy.focus?.question = nil }
        return copy
    }
}
