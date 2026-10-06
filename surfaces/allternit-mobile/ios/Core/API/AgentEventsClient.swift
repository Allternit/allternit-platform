import Foundation

// -----------------------------------------------------------------------------
// AgentRunEvent — one frame of `GET /api/v1/agents/:id/events`
// (cmd/allternit-api/src/agent_routes.rs:84-138).
//
// The handler replays up to 50 recent Factory ledger events for the agent on
// connect, then polls the ledger every 2s for new ones. Each SSE `data:`
// frame is a JSON envelope:
//
//   {"event_type": "...", "agent_id": "...", "run_id": "..."|null,
//    "timestamp": "<rfc3339>", "data": { ...original event payload... }}
//
// (`timestamp` is the ledger's `Utc::now().to_rfc3339()` stamp —
// factory/engine/src/core/ledger — so it may carry fractional seconds.)
//
// Event types the backend actually emits with an `agent_id` payload today
// (agent_routes.rs):
//
//   agent.created        {agent_id, name, agent_type, model, provider, trust_tier}   (rs:684-713)
//   agent.run.started    {agent_id, run_id, model, provider}                          (rs:2180-2192)
//   agent.run.completed  {agent_id, run_id, duration_ms, output?}                     (rs:2457-2475)
//   agent.run.failed     {agent_id, run_id, duration_ms, error?}                      (rs:2457-2475)
//
// Four more arrive via `POST /api/v1/agents/:id/events/ingest`
// (agent_routes.rs `ingest_agent_event`), fed by gizzi-code's
// agent-event-bridge (cmd/gizzi-code/src/runtime/services/agent-event-bridge.ts)
// from the runtime's permission/question/session bus signals:
//
//   agent.run.waiting_approval   {agent_id, run_id?, request_id, permission, patterns, session_id}
//   agent.run.approval_resolved  {agent_id, run_id?, request_id, reply?, session_id}
//   agent.run.waiting_input      {agent_id, run_id?, request_id, questions, session_id}
//   agent.run.blocked            {agent_id, run_id?, error, message, session_id}
//
// Like the AgentChatEvent parser, unknown event types map to `.unknown`
// instead of failing the stream — the ledger can gain event types (gate/WIH
// events, other writers) at any time, and the feed should show them, not die.
// -----------------------------------------------------------------------------

/// A single parsed event from the agent events stream.
enum AgentRunEvent: Decodable, Sendable {
    /// `agent.created` — the bot's registry row was created.
    case agentCreated(Created)

    /// `agent.run.started` — a run began (model/provider the run uses).
    case runStarted(RunStarted)

    /// `agent.run.completed` — a run finished successfully. `output` is
    /// capped at 2000 chars server-side (agent_routes.rs:2462-2466).
    case runCompleted(RunCompleted)

    /// `agent.run.failed` — a run hit a terminal failure.
    case runFailed(RunFailed)

    /// `agent.run.waiting_approval` — the runtime is paused on a tool
    /// permission prompt (gizzi `permission.asked`, bridged via
    /// `POST /agents/:id/events/ingest`).
    case waitingApproval(WaitingApproval)

    /// `agent.run.approval_resolved` — a permission prompt (or input wait)
    /// was answered and the run resumed.
    case approvalResolved(ApprovalResolved)

    /// `agent.run.waiting_input` — the runtime is paused on a question
    /// prompt (gizzi `question.asked`).
    case waitingInput(WaitingInput)

    /// `agent.run.blocked` — the run hit a hard, non-retryable runtime error
    /// (gizzi `session.error`, minus user aborts).
    case blocked(Blocked)

    /// Any other ledger event type for this agent (gate, WIH, mail, …) —
    /// surfaced in the activity feed under its raw type name.
    case unknown(Unknown)

    /// Fields every envelope carries, pulled up for the feed/store.
    struct Envelope: Sendable {
        let agentId: String?
        /// Frame-level `run_id` (the ledger event's scope), falling back to
        /// the payload's own `run_id` — the handler sets both for run events.
        let runId: String?
        let timestamp: Date?
    }

    struct Created: Sendable {
        let envelope: Envelope
        let name: String?
        let agentType: String?
    }

    struct RunStarted: Sendable {
        let envelope: Envelope
        let model: String?
        let provider: String?
    }

    struct RunCompleted: Sendable {
        let envelope: Envelope
        let durationMs: Int?
        let output: String?
    }

    struct RunFailed: Sendable {
        let envelope: Envelope
        let durationMs: Int?
        let error: String?
    }

    struct WaitingApproval: Sendable {
        let envelope: Envelope
        let requestId: String?
        let permission: String?
    }

    struct ApprovalResolved: Sendable {
        let envelope: Envelope
        let requestId: String?
        /// Permission replies carry `reply` ("once"/"always"/"reject");
        /// question resolutions carry `resolution` ("answered"/"dismissed").
        let reply: String?
        let resolution: String?
    }

    struct WaitingInput: Sendable {
        let envelope: Envelope
        let requestId: String?
        let questions: [String]
    }

    struct Blocked: Sendable {
        let envelope: Envelope
        let error: String?
        let message: String?
    }

    struct Unknown: Sendable {
        let envelope: Envelope
        let type: String
    }

    var envelope: Envelope {
        switch self {
        case .agentCreated(let e): return e.envelope
        case .runStarted(let e): return e.envelope
        case .runCompleted(let e): return e.envelope
        case .runFailed(let e): return e.envelope
        case .waitingApproval(let e): return e.envelope
        case .approvalResolved(let e): return e.envelope
        case .waitingInput(let e): return e.envelope
        case .blocked(let e): return e.envelope
        case .unknown(let e): return e.envelope
        }
    }

    var timestamp: Date? { envelope.timestamp }

    /// Feed label for the Activity section.
    var label: String {
        switch self {
        case .agentCreated: return "Bot created"
        case .runStarted: return "Run started"
        case .runCompleted: return "Run completed"
        case .runFailed: return "Run failed"
        case .waitingApproval: return "Waiting for approval"
        case .approvalResolved: return "Approval resolved"
        case .waitingInput: return "Waiting for input"
        case .blocked: return "Blocked"
        case .unknown(let event): return event.type
        }
    }

    // MARK: - Decoding

    private enum CodingKeys: String, CodingKey {
        case eventType = "event_type"
        case agentId = "agent_id"
        case runId = "run_id"
        case timestamp
        case data
    }

    private struct CreatedData: Decodable {
        let name: String?
        let agentType: String?
        enum CodingKeys: String, CodingKey {
            case name
            case agentType = "agent_type"
        }
    }

    private struct RunStartedData: Decodable {
        let runId: String?
        let model: String?
        let provider: String?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case model, provider
        }
    }

    private struct RunFinishedData: Decodable {
        let runId: String?
        let durationMs: Int?
        let output: String?
        let error: String?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case durationMs = "duration_ms"
            case output, error
        }
    }

    private struct WaitingApprovalData: Decodable {
        let runId: String?
        let requestId: String?
        let permission: String?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case requestId = "request_id"
            case permission
        }
    }

    private struct ApprovalResolvedData: Decodable {
        let runId: String?
        let requestId: String?
        let reply: String?
        let resolution: String?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case requestId = "request_id"
            case reply, resolution
        }
    }

    private struct WaitingInputData: Decodable {
        let runId: String?
        let requestId: String?
        let questions: [String]?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case requestId = "request_id"
            case questions
        }
    }

    private struct BlockedData: Decodable {
        let runId: String?
        let error: String?
        let message: String?
        enum CodingKeys: String, CodingKey {
            case runId = "run_id"
            case error, message
        }
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decodeIfPresent(String.self, forKey: .eventType) ?? ""
        let agentId = try container.decodeIfPresent(String.self, forKey: .agentId)
        let scopeRunId = try container.decodeIfPresent(String.self, forKey: .runId)
        let timestamp = (try container.decodeIfPresent(String.self, forKey: .timestamp))
            .flatMap(Self.parseTimestamp)

        switch type {
        case "agent.created":
            let data = try? container.decodeIfPresent(CreatedData.self, forKey: .data)
            self = .agentCreated(Created(
                envelope: Envelope(agentId: agentId, runId: scopeRunId, timestamp: timestamp),
                name: data?.name,
                agentType: data?.agentType
            ))

        case "agent.run.started":
            let data = try? container.decodeIfPresent(RunStartedData.self, forKey: .data)
            self = .runStarted(RunStarted(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                model: data?.model,
                provider: data?.provider
            ))

        case "agent.run.completed":
            let data = try? container.decodeIfPresent(RunFinishedData.self, forKey: .data)
            self = .runCompleted(RunCompleted(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                durationMs: data?.durationMs,
                output: data?.output
            ))

        case "agent.run.failed":
            let data = try? container.decodeIfPresent(RunFinishedData.self, forKey: .data)
            self = .runFailed(RunFailed(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                durationMs: data?.durationMs,
                error: data?.error
            ))

        case "agent.run.waiting_approval":
            let data = try? container.decodeIfPresent(WaitingApprovalData.self, forKey: .data)
            self = .waitingApproval(WaitingApproval(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                requestId: data?.requestId,
                permission: data?.permission
            ))

        case "agent.run.approval_resolved":
            let data = try? container.decodeIfPresent(ApprovalResolvedData.self, forKey: .data)
            self = .approvalResolved(ApprovalResolved(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                requestId: data?.requestId,
                reply: data?.reply,
                resolution: data?.resolution
            ))

        case "agent.run.waiting_input":
            let data = try? container.decodeIfPresent(WaitingInputData.self, forKey: .data)
            self = .waitingInput(WaitingInput(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                requestId: data?.requestId,
                questions: data?.questions ?? []
            ))

        case "agent.run.blocked":
            let data = try? container.decodeIfPresent(BlockedData.self, forKey: .data)
            self = .blocked(Blocked(
                envelope: Envelope(agentId: agentId, runId: scopeRunId ?? data?.runId, timestamp: timestamp),
                error: data?.error,
                message: data?.message
            ))

        default:
            self = .unknown(Unknown(
                envelope: Envelope(agentId: agentId, runId: scopeRunId, timestamp: timestamp),
                type: type
            ))
        }
    }

    /// RFC3339 with optional fractional seconds (chrono `to_rfc3339()` omits
    /// them for whole-second stamps, so try the strict format first).
    private static func parseTimestamp(_ raw: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: raw) { return date }
        return ISO8601DateFormatter().date(from: raw)
    }
}

/// SSE client for `GET /api/v1/agents/:id/events` — the only server-backed
/// live source of per-bot activity (the web's bot-event-store is
/// browser-localStorage and not portable). Mirrors AgentChatClient's
/// `sendMessageStream` plumbing: awaited-auth request, `Accept:
/// text/event-stream`, line iteration over `URLSession.AsyncBytes`.
final class AgentEventsClient: @unchecked Sendable {
    static let shared = AgentEventsClient()

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// Streams ledger events for one agent until cancelled. The server never
    /// ends this stream on its own (2s poll loop, 15s keep-alive comments —
    /// agent_routes.rs:129-137), so a normal end-of-body is treated like an
    /// error by the consumer (BotStatusStore reconnects). Undecodable frames
    /// are skipped with a log, same posture as the chat stream parser.
    func eventStream(agentId: String) -> AsyncThrowingStream<AgentRunEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = try await client.authorizedRequest(
                        path: "agents/\(Self.escape(agentId))/events"
                    )
                    request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    // Long-lived stream: the default idle timeout would kill
                    // a quiet bot's stream between the 15s keep-alives.
                    request.timeoutInterval = 60

                    let (bytes, response) = try await client.sendStream(request)
                    try client.validate(response)

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        try Task.checkCancellation()

                        // SSE frames are `data: {json}`; blank lines and the
                        // axum keep-alive `:keepalive` comments are skipped.
                        guard line.hasPrefix("data: ") else { continue }
                        let payload = String(line.dropFirst(6))
                        guard !payload.isEmpty else { continue }

                        // OpenAI-style sentinel, tolerated like the web parser.
                        if payload == "[DONE]" {
                            continuation.finish()
                            return
                        }

                        guard let data = payload.data(using: .utf8),
                              let event = try? decoder.decode(AgentRunEvent.self, from: data) else {
                            print("agent-events: skipping undecodable frame: \(payload)")
                            continue
                        }

                        continuation.yield(event)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }

            // Cancelling the consumer cancels the Task, which cancels the
            // underlying URLSession stream.
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Same path-safety insurance as AgentChatClient.escape.
    private static func escape(_ agentId: String) -> String {
        agentId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? agentId
    }
}
