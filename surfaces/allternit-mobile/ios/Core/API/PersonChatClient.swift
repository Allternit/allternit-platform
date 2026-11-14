import Foundation

/// One chat message replayed to a person model (contract §9.3 D8: the whole
/// history goes every turn; there is no session on a computer).
struct PersonChatMessage: Equatable, Sendable, Encodable {
    let role: String
    let content: String
}

/// A person-model turn the server or OpenAI refused. Shown inline under the
/// reply in plain words, never as raw JSON.
struct PersonChatRefusal: Error, Equatable, Sendable {
    let code: String
    let message: String?
    let vendor: String

    /// Contract §9.3 / §9.2 codes, plus the on-device ChatGPT ones.
    var plainText: String {
        switch code {
        case "plan_person_only": return "Your \(vendor) plan only answers messages you send yourself in chat."
        case "plan_rate": return "You've sent a lot of messages to your \(vendor) plan in the last few minutes. Wait a little and send again."
        case "plan_turn_in_flight": return "Your \(vendor) plan is still answering another message. Wait for it to finish."
        case "plan_not_connected": return "Your \(vendor) plan isn't connected. Connect it in Settings › Connections."
        case "plan_no_tools": return "Your \(vendor) plan answers chat only, without tools."
        case "runner_busy": return "Busy. Try again"
        case "could_not_start": return "Could not start. Try again"
        case "login_expired": return "Your \(vendor) sign-in expired. Sign in again in Settings › Connections."
        case "vendor_limited": return "\(vendor) limited this plan."
        case "usage_limit": return "ChatGPT limit reached. It resets on OpenAI's schedule. Try again later, or pick another model."
        case "text_only": return "Your \(vendor) plan takes text only. Remove the attachment and send again."
        case "signed_out_allternit": return "Sign in to Allternit on this phone to use your \(vendor) plan."
        case "incomplete": return "\(vendor) stopped before the reply was finished\(message.map { " (\($0))" } ?? "")."
        default:
            if let message, !message.isEmpty { return message }
            return "\(vendor) didn't answer. Try again."
        }
    }

    var card: ChatError {
        var error = ChatError(name: code, message: plainText, raw: message.map { "\(code): \($0)" } ?? code)
        error.title = "\(vendor) didn't answer"
        error.detail = plainText
        return error
    }
}

enum PersonChatEvent: Equatable, Sendable {
    case delta(String)
    case done
}

/// Sends a person-model turn: plan and Copilot models to cloud-api
/// `POST /v1/chat/completions` with `X-Allternit-Turn: person`, and
/// `chatgpt/*` straight from the phone to OpenAI's Responses API. No tools,
/// no attachments, history replayed.
enum PersonChatClient {
    static func stream(model: String, messages: [PersonChatMessage]) -> AsyncThrowingStream<PersonChatEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    if PersonModels.isOnDevice(model) {
                        try await streamChatGPT(model: model, messages: messages, continuation)
                    } else {
                        try await streamCloud(model: model, messages: messages, continuation)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: - cloud-api (plan/*, copilot/*)

    static func cloudBody(model: String, messages: [PersonChatMessage]) throws -> Data {
        struct Body: Encodable {
            let model: String
            let messages: [PersonChatMessage]
            let stream: Bool
        }
        return try JSONEncoder().encode(Body(model: model, messages: messages, stream: true))
    }

    /// One `data:` payload of the OpenAI-style chunk stream: the text delta,
    /// or a coded error the server put in the stream.
    enum CloudChunk: Equatable {
        case delta(String)
        case error(code: String, message: String?)
        case none
    }

    static func parseCloudChunk(_ payload: String) -> CloudChunk {
        guard let data = payload.data(using: .utf8),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return .none }
        if let error = object["error"] as? [String: Any] {
            return .error(code: (error["code"] as? String) ?? "failed", message: error["message"] as? String)
        }
        let choice = (object["choices"] as? [[String: Any]])?.first
        if let text = (choice?["delta"] as? [String: Any])?["content"] as? String, !text.isEmpty { return .delta(text) }
        return .none
    }

    /// A non-2xx body: `{"error":{"code","message"}}` (contract §9.3).
    static func refusal(status: Int, body: Data, vendor: String) -> PersonChatRefusal {
        let object = (try? JSONSerialization.jsonObject(with: body)) as? [String: Any]
        let error = object?["error"] as? [String: Any]
        let code = (error?["code"] as? String) ?? (object?["error"] as? String) ?? Self.code(forStatus: status)
        return PersonChatRefusal(code: code, message: error?["message"] as? String, vendor: vendor)
    }

    private static func code(forStatus status: Int) -> String {
        switch status {
        case 401: return "signed_out_allternit"
        case 403: return "plan_person_only"
        case 429: return "plan_rate"
        case 503: return "runner_busy"
        default: return "failed"
        }
    }

    private static func streamCloud(model: String, messages: [PersonChatMessage],
                                    _ continuation: AsyncThrowingStream<PersonChatEvent, Error>.Continuation) async throws {
        let vendor = PersonModels.vendorName(for: model)
        guard let token = try await AuthManager.shared.getToken(), !token.isEmpty else {
            throw PersonChatRefusal(code: "signed_out_allternit", message: nil, vendor: vendor)
        }
        var request = URLRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent("v1/chat/completions"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        // Set only here, on a message the person sent from the composer (D7).
        request.setValue("person", forHTTPHeaderField: "X-Allternit-Turn")
        request.timeoutInterval = 600
        request.httpBody = try cloudBody(model: model, messages: messages)

        let (bytes, response) = try await URLSession.shared.bytes(for: request)
        guard let http = response as? HTTPURLResponse else { throw APIError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            throw refusal(status: http.statusCode, body: try await collect(bytes), vendor: vendor)
        }
        for try await line in bytes.lines {
            try Task.checkCancellation()
            guard line.hasPrefix("data:") else { continue }
            let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
            if payload == "[DONE]" { break }
            switch parseCloudChunk(payload) {
            case .delta(let text): continuation.yield(.delta(text))
            case .error(let code, let message): throw PersonChatRefusal(code: code, message: message, vendor: vendor)
            case .none: continue
            }
        }
        continuation.yield(.done)
    }

    // MARK: - OpenAI Responses (chatgpt/*), from the phone

    static func responsesBody(model: String, messages: [PersonChatMessage]) throws -> Data {
        struct Body: Encodable {
            let model: String
            let input: [PersonChatMessage]
            let store: Bool
            let stream: Bool
        }
        let slug = model.hasPrefix("chatgpt/") ? String(model.dropFirst("chatgpt/".count)) : model
        return try JSONEncoder().encode(Body(model: slug, input: messages, store: false, stream: true))
    }

    private static func streamChatGPT(model: String, messages: [PersonChatMessage],
                                      _ continuation: AsyncThrowingStream<PersonChatEvent, Error>.Continuation) async throws {
        let body = try responsesBody(model: model, messages: messages)
        let bytes: URLSession.AsyncBytes
        let http: HTTPURLResponse
        do {
            (bytes, http) = try await ChatGPTAuthService.shared.authorizedBytes { token in
                var request = URLRequest(url: URL(string: "https://api.openai.com/v1/responses")!)
                request.httpMethod = "POST"
                request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                request.timeoutInterval = 600
                request.httpBody = body
                return request
            }
        } catch ChatGPTAuthError.signedOut {
            await MainActor.run { ChatGPTDeviceStore.shared.markExpired() }
            throw PersonChatRefusal(code: "login_expired", message: nil, vendor: "ChatGPT")
        }
        guard (200...299).contains(http.statusCode) else {
            let data = try await collect(bytes)
            let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            let error = object?["error"] as? [String: Any]
            let code = error?["code"] as? String
            if let code, ResponsesStreamEvent.usageLimitCodes.contains(code) {
                await MainActor.run { ChatGPTDeviceStore.shared.markLimitReached() }
                throw PersonChatRefusal(code: "usage_limit", message: nil, vendor: "ChatGPT")
            }
            if http.statusCode == 401 {
                await MainActor.run { ChatGPTDeviceStore.shared.markExpired() }
                throw PersonChatRefusal(code: "login_expired", message: nil, vendor: "ChatGPT")
            }
            throw PersonChatRefusal(code: code ?? "failed", message: error?["message"] as? String, vendor: "ChatGPT")
        }
        var completed = false
        for try await line in bytes.lines {
            try Task.checkCancellation()
            guard line.hasPrefix("data:") else { continue }
            let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
            if payload == "[DONE]" { break }
            switch ResponsesStreamEvent.parse(payload) {
            case .delta(let text)?:
                continuation.yield(.delta(text))
            case .completed?:
                completed = true
            case .failed(let code, let message)?, .error(let code, let message)?:
                if let code, ResponsesStreamEvent.usageLimitCodes.contains(code) {
                    await MainActor.run { ChatGPTDeviceStore.shared.markLimitReached() }
                    throw PersonChatRefusal(code: "usage_limit", message: nil, vendor: "ChatGPT")
                }
                throw PersonChatRefusal(code: code ?? "failed", message: message, vendor: "ChatGPT")
            case .incomplete(let reason)?:
                throw PersonChatRefusal(code: "incomplete", message: reason, vendor: "ChatGPT")
            case .other?, nil:
                continue
            }
            if completed { break }
        }
        // A stream that ends without response.completed failed (Models and inference).
        guard completed else { throw PersonChatRefusal(code: "incomplete", message: nil, vendor: "ChatGPT") }
        await MainActor.run { ChatGPTDeviceStore.shared.markWorking() }
        continuation.yield(.done)
    }

    private static func collect(_ bytes: URLSession.AsyncBytes) async throws -> Data {
        var data = Data()
        for try await byte in bytes {
            data.append(byte)
            if data.count > 64 * 1024 { break }
        }
        return data
    }
}
