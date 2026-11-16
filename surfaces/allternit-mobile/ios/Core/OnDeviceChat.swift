import Foundation
import OnDeviceLLM
import os
#if canImport(UIKit)
import UIKit
#endif
import UserNotifications

// Lives in Core/, not Core/OnDevice/: the OnDeviceLab target compiles every
// file in Core/OnDevice and has none of the app types used here.

// W8 glue between the W7 runtime and the app's screens and chat:
// chat model ids for on-device and other-device models, the on-phone reply
// stream, the downloads the Models screens start, and "My other devices"
// (models the owner's other devices share through cloud-api's model-host
// relay; same behavior as allternit-ai src/lib/model-host/remote.ts).

private let glueLog = Logger(subsystem: "com.allternit.app", category: "ondevice")

// MARK: - Chat model ids

/// Chat ids for models that run on this iPhone:
/// `ondevice/<freeweights id>#<quant>` and `ondevice/apple-fm`.
/// They answer only the person's own chat messages (PersonModels), with no
/// tools, no attachments and no cloud storage of the turns.
enum OnDeviceModelId {
    static let prefix = "ondevice/"
    static let appleFM = "ondevice/apple-fm"

    static func isOnDevice(_ id: String?) -> Bool { id?.hasPrefix(prefix) ?? false }

    static func make(_ install: OnDeviceInstall) -> String { "\(prefix)\(install.modelId)#\(install.quant)" }

    /// `(modelId, quant)` for an installed-model id; nil for Apple FM or junk.
    static func parse(_ id: String) -> (modelId: String, quant: String)? {
        guard id.hasPrefix(prefix), id != appleFM else { return nil }
        let rest = id.dropFirst(prefix.count)
        guard let hash = rest.lastIndex(of: "#") else { return nil }
        let model = String(rest[..<hash]), quant = String(rest[rest.index(after: hash)...])
        return model.isEmpty || quant.isEmpty ? nil : (model, quant)
    }

    /// "Qwen3.5-0.8B" for the composer pill.
    static func displayName(_ id: String) -> String? {
        if id == appleFM { return "Apple Intelligence" }
        guard let p = parse(id) else { return nil }
        return p.modelId.split(separator: "/").last.map(String.init) ?? p.modelId
    }
}

/// Chat ids for models on the owner's other devices: `device/<host id>/<model id>`.
enum OtherDeviceModelId {
    static let prefix = "device/"

    static func isOtherDevice(_ id: String?) -> Bool { id?.hasPrefix(prefix) ?? false }

    static func make(hostId: String, modelId: String) -> String { "\(prefix)\(hostId)/\(modelId)" }

    /// `<host id>/<model id>`, what the relay's `model` field takes.
    static func relayModel(_ id: String) -> String { String(id.dropFirst(prefix.count)) }

    static func displayName(_ id: String) -> String? {
        guard isOtherDevice(id) else { return nil }
        let rest = relayModel(id)
        guard let slash = rest.firstIndex(of: "/") else { return nil }
        let model = String(rest[rest.index(after: slash)...])
        return model.split(separator: "/").last.map(String.init) ?? model
    }
}

// MARK: - Reply stream on this iPhone

enum OnDeviceChat {
    /// Streams a reply from an on-device model: loads it (one model at a
    /// time, never past the budget), then generates. Errors are
    /// PersonChatRefusal with plain words, shown inline under the reply.
    @MainActor
    static func stream(model: String, messages: [PersonChatMessage],
                       _ continuation: AsyncThrowingStream<PersonChatEvent, Error>.Continuation) async throws {
        let rt = OnDeviceRuntime.shared
        let vendor = "This iPhone"
        do {
            if model == OnDeviceModelId.appleFM {
                if rt.loaded?.engine != .appleFM { try await rt.loadAppleFoundationModel() }
            } else {
                guard let p = OnDeviceModelId.parse(model) else {
                    throw PersonChatRefusal(code: "ondevice_missing", message: "This model is not on this iPhone any more. Pick another model.", vendor: vendor)
                }
                rt.refreshInstalls()
                guard let install = rt.installs.first(where: { $0.modelId == p.modelId && $0.quant == p.quant }) else {
                    throw PersonChatRefusal(code: "ondevice_missing", message: "This model is not on this iPhone any more. Download it again in Models.", vendor: vendor)
                }
                try await rt.load(install)
            }
            let turns = messages.map { m -> ChatTurn in
                ChatTurn(m.role == "assistant" ? .assistant : m.role == "system" ? .system : .user, m.content)
            }
            for try await text in rt.generate(turns) {
                try Task.checkCancellation()
                continuation.yield(.delta(text))
            }
            continuation.yield(.done)
        } catch let refusal as PersonChatRefusal {
            throw refusal
        } catch is CancellationError {
            throw CancellationError()
        } catch let error as OnDeviceRuntimeError {
            throw PersonChatRefusal(code: "ondevice_failed", message: error.description, vendor: vendor)
        } catch {
            throw PersonChatRefusal(code: "ondevice_failed", message: String(describing: error), vendor: vendor)
        }
    }
}

// MARK: - Downloads started from the Models screens

/// Downloads the Models screens start and follow. The W7 downloader does the
/// work (background URLSession, any network, signed-manifest check); this
/// keeps the task, the plain-words error and the "ready" notification.
@MainActor
final class OnDeviceDownloads: ObservableObject {
    static let shared = OnDeviceDownloads()

    /// Plain-words failure per install key (`<modelId>#<quant>`).
    @Published private(set) var errors: [String: String] = [:]
    @Published private(set) var active: Set<String> = []
    private var tasks: [String: Task<Void, Never>] = [:]
    private var cancelled: Set<String> = []

    static func key(_ modelId: String, _ quant: String) -> String { "\(modelId)#\(quant)" }

    /// The build files come from the per-model device route (the index has
    /// no file lists), then the runtime installs the quant.
    func start(modelId: String, quant: Quant) {
        let key = Self.key(modelId, quant.name)
        guard tasks[key] == nil else { return }
        errors[key] = nil
        active.insert(key)
        cancelled.remove(key)
        // Main-actor task (this class is @MainActor): state updates need no hop.
        tasks[key] = Task {
            do {
                var full = quant
                if full.files?.isEmpty ?? true {
                    guard let data = await FreeweightsClient.shared.deviceData(id: modelId).value,
                          let match = data.quants.first(where: { $0.name == quant.name && $0.engine == quant.engine }) else {
                        throw InstallError.noFiles
                    }
                    full = match
                }
                _ = try await OnDeviceRuntime.shared.install(modelId: modelId, quant: full)
                await Self.notifyReady(modelId: modelId)
            } catch {
                if !cancelled.contains(key) {
                    errors[key] = Self.message(error)
                    glueLog.error("download \(key, privacy: .public) failed: \(String(describing: error), privacy: .public)")
                }
            }
            finish(key)
        }
    }

    func cancel(modelId: String, quant: String) {
        let key = Self.key(modelId, quant)
        cancelled.insert(key)
        OnDeviceRuntime.shared.cancelInstall(modelId: modelId, quant: quant)
        tasks[key]?.cancel()
        errors[key] = nil
    }

    func clearError(_ key: String) { errors[key] = nil }

    private func finish(_ key: String) {
        tasks[key] = nil
        active.remove(key)
    }

    /// Plain words for a failed download.
    static func message(_ error: Error) -> String {
        if case let InstallError.notEnoughSpace(needed, free) = error {
            return "Not enough storage. Needs \(FwText.deviceSize(Double(needed))), \(FwText.deviceSize(Double(free))) free."
        }
        if case let InstallError.http(code, _) = error, code == 404 {
            return "This build has no signed Freeweights manifest yet, so it can't be checked. Try another build."
        }
        if case InstallError.noFiles = error { return "Freeweights lists no files for this build yet." }
        if let e = error as? OnDeviceRuntimeError { return e.description }
        if let u = error as? URLError {
            switch u.code {
            case .notConnectedToInternet, .networkConnectionLost: return "You're offline. The download picks up where it stopped when you try again."
            case .cancelled: return "Download stopped."
            default: break
            }
        }
        let text = String(describing: error)
        if text.localizedCaseInsensitiveContains("sha256") || text.localizedCaseInsensitiveContains("size") {
            return "A file did not match the signed manifest, so it was deleted. Try again."
        }
        return "Download failed. Try again."
    }

    /// "Ready" notification when the download finishes in the background.
    private static func notifyReady(modelId: String) async {
        #if canImport(UIKit)
        guard UIApplication.shared.applicationState != .active else { return }
        #endif
        let content = UNMutableNotificationContent()
        content.title = "Model ready"
        content.body = "\(modelId.split(separator: "/").last.map(String.init) ?? modelId) is on this iPhone. Open Allternit to chat with it."
        try? await UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "ondevice-ready-\(modelId)", content: content, trigger: nil))
    }
}

// MARK: - My other devices (model-host relay)

struct RelayHostModel: Equatable, Sendable, Codable {
    var id: String
    var name: String
    var contextLength: Int
}

struct RelayHost: Equatable, Sendable, Codable, Identifiable {
    var hostId: String
    var name: String
    var models: [RelayHostModel]
    var id: String { hostId }
}

/// The owner's other devices that share models through cloud-api
/// (`GET /api/v1/model-hosts`). Hidden on any failure (signed out, no relay
/// on this cloud-api, offline): the picker never shows an error for it.
@MainActor
final class OtherDevicesStore: ObservableObject {
    static let shared = OtherDevicesStore()

    enum Status: Equatable { case idle, ok, hidden }

    @Published private(set) var status: Status = .idle
    @Published private(set) var online: [RelayHost] = []
    /// Seen before, not connected now.
    @Published private(set) var offline: [RelayHost] = []

    private static let seenKey = "allternit.modelHost.seen"
    private var inFlight = false

    /// The relay's host list, tolerant of missing or odd fields (remote.ts `parseHosts`).
    nonisolated static func parseHosts(_ data: Data) -> [RelayHost] {
        guard let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let raw = root["hosts"] as? [[String: Any]] else { return [] }
        return raw.compactMap { h in
            guard let id = h["host_id"] as? String, !id.isEmpty else { return nil }
            let models = ((h["models"] as? [[String: Any]]) ?? []).compactMap { m -> RelayHostModel? in
                guard let mid = m["id"] as? String, !mid.isEmpty else { return nil }
                let name = (m["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? mid
                return RelayHostModel(id: mid, name: name, contextLength: (m["context_length"] as? NSNumber)?.intValue ?? 0)
            }
            let name = (h["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Another device"
            return RelayHost(hostId: id, name: name, models: models)
        }
    }

    func refresh() async {
        guard !inFlight else { return }
        inFlight = true
        defer { inFlight = false }
        guard let token = try? await AuthManager.shared.getToken(), !token.isEmpty else {
            status = .hidden; online = []; offline = []
            return
        }
        var request = URLRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/model-hosts"))
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 8
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else {
            // Keep nothing as connected without an answer; hide the section.
            status = .hidden; online = []
            return
        }
        let fresh = Self.parseHosts(data)
        let seen = readSeen()
        let live = Set(fresh.map(\.hostId))
        online = fresh
        offline = seen.filter { !live.contains($0.hostId) }
        writeSeen(fresh + seen.filter { !live.contains($0.hostId) })
        status = .ok
    }

    func host(_ hostId: String) -> RelayHost? { (online + offline).first { $0.hostId == hostId } }

    private func readSeen() -> [RelayHost] {
        guard let data = UserDefaults.standard.data(forKey: Self.seenKey) else { return [] }
        return (try? JSONDecoder().decode([RelayHost].self, from: data)) ?? []
    }

    private func writeSeen(_ hosts: [RelayHost]) {
        if let data = try? JSONEncoder().encode(Array(hosts.prefix(8))) { UserDefaults.standard.set(data, forKey: Self.seenKey) }
    }

    /// Plain words for a relay failure, with the device named (remote.ts `relayErrorMessage`).
    nonisolated static func relayErrorMessage(_ device: String, status: Int, apiMessage: String?) -> String {
        switch status {
        case 401: return "Sign in to use your other devices."
        case 404: return "\(device) is offline or stopped sharing this model."
        case 502: return "\(device) went offline."
        case 504: return "\(device) did not answer in time."
        default: return apiMessage ?? "\(device) could not answer (\(status))."
        }
    }

    /// One reply from a model on another device: `POST
    /// /api/v1/model-hosts/v1/chat/completions` as an OpenAI SSE stream.
    static func stream(model: String, messages: [PersonChatMessage],
                       _ continuation: AsyncThrowingStream<PersonChatEvent, Error>.Continuation) async throws {
        let relayModel = OtherDeviceModelId.relayModel(model)
        let hostId = String(relayModel.split(separator: "/").first ?? "")
        let device = await MainActor.run { OtherDevicesStore.shared.host(hostId)?.name } ?? "Your other device"
        func refuse(_ text: String) -> PersonChatRefusal { PersonChatRefusal(code: "relay_failed", message: text, vendor: device) }
        guard let token = try await AuthManager.shared.getToken(), !token.isEmpty else {
            throw refuse("Sign in to use your other devices.")
        }
        var request = URLRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/model-hosts/v1/chat/completions"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 300
        struct Body: Encodable { let model: String; let messages: [PersonChatMessage]; let stream: Bool }
        request.httpBody = try JSONEncoder().encode(Body(model: relayModel, messages: messages, stream: true))
        let bytes: URLSession.AsyncBytes
        let response: URLResponse
        do {
            (bytes, response) = try await URLSession.shared.bytes(for: request)
        } catch let e as URLError where e.code == .cancelled {
            throw e
        } catch {
            throw refuse("Allternit Cloud could not be reached.")
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200...299).contains(status) else {
            var body = Data()
            for try await b in bytes { body.append(b); if body.count > 64_000 { break } }
            let api = ((try? JSONSerialization.jsonObject(with: body)) as? [String: Any]).flatMap { ($0["error"] as? [String: Any])?["message"] as? String }
            throw refuse(relayErrorMessage(device, status: status, apiMessage: api))
        }
        var finished = false
        do {
            for try await line in bytes.lines {
                try Task.checkCancellation()
                guard line.hasPrefix("data:") else { continue }
                let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                if payload == "[DONE]" { finished = true; break }
                switch PersonChatClient.parseCloudChunk(payload) {
                case .delta(let t): continuation.yield(.delta(t))
                case .error(_, let message): throw refuse(message ?? "\(device) stopped answering.")
                case .none: continue
                }
            }
        } catch let r as PersonChatRefusal {
            throw r
        } catch let e as URLError where e.code == .cancelled {
            throw e
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw refuse("The connection to \(device) dropped.")
        }
        if !finished { throw refuse("\(device) went offline before the reply finished.") }
        continuation.yield(.done)
    }
}
