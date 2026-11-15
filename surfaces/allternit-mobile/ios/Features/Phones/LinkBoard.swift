import Foundation

/// Allternit Link board status, as Desktop builds it and allternit-api serves
/// it: `GET /api/v1/phones/boards` → `{ desktop, boards: LinkBoardStatus[] }`
/// (hardware/link/README.md section 6). Read-only here: nothing in this file
/// can arm a board. Only the board's own button arms it.

/// A board state. Values outside the contract decode to `.other` instead of
/// failing the whole list.
enum LinkBoardState: Equatable, Sendable {
    case boardOffline, phoneNotConnected, phoneLocked, noScreen, notCalibrated, disarmed, ready
    case other(String)

    init(raw: String) {
        switch raw {
        case "board_offline": self = .boardOffline
        case "phone_not_connected": self = .phoneNotConnected
        case "phone_locked": self = .phoneLocked
        case "no_screen": self = .noScreen
        case "not_calibrated": self = .notCalibrated
        case "disarmed": self = .disarmed
        case "ready": self = .ready
        default: self = .other(raw)
        }
    }

    /// Plain words for the person.
    var label: String {
        switch self {
        case .boardOffline: return "Board offline"
        case .phoneNotConnected: return "Phone not connected"
        case .phoneLocked: return "Phone locked"
        case .noScreen: return "No screen yet"
        case .notCalibrated: return "Not calibrated"
        case .disarmed: return "Disarmed"
        case .ready: return "Ready"
        case .other: return "Unknown state"
        }
    }
}

enum LinkBoardLink: Equatable, Sendable {
    case lan, cloud, other(String)

    var label: String {
        switch self {
        case .lan: return "Same Wi-Fi"
        case .cloud: return "Cloud"
        case .other: return "Connected"
        }
    }
}

struct LinkBoard: Decodable, Equatable, Identifiable, Sendable {
    let boardId: String
    let name: String
    let online: Bool
    let armed: Bool
    let status: LinkBoardState
    let link: LinkBoardLink?
    /// The paired app phone's serial, `app:<uuid>`.
    let phone: String?

    var id: String { boardId }

    private enum CodingKeys: String, CodingKey {
        case boardId, name, online, armed, status, link, phone
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        boardId = try c.decode(String.self, forKey: .boardId)
        let rawName = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        name = rawName.isEmpty ? boardId : rawName
        online = try c.decodeIfPresent(Bool.self, forKey: .online) ?? false
        armed = try c.decodeIfPresent(Bool.self, forKey: .armed) ?? false
        status = LinkBoardState(raw: try c.decodeIfPresent(String.self, forKey: .status) ?? "")
        link = try c.decodeIfPresent(String.self, forKey: .link).map {
            switch $0 {
            case "lan": return .lan
            case "cloud": return .cloud
            default: return .other($0)
            }
        }
        phone = try c.decodeIfPresent(String.self, forKey: .phone)
    }

    init(boardId: String, name: String, online: Bool, armed: Bool, status: LinkBoardState,
         link: LinkBoardLink?, phone: String?) {
        self.boardId = boardId; self.name = name; self.online = online; self.armed = armed
        self.status = status; self.link = link; self.phone = phone
    }
}

struct LinkBoardList: Decodable, Equatable, Sendable {
    let desktop: Bool
    let boards: [LinkBoard]

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        desktop = try c.decodeIfPresent(Bool.self, forKey: .desktop) ?? true
        // One malformed row must not hide the others.
        boards = (try c.decodeIfPresent([Lossy<LinkBoard>].self, forKey: .boards) ?? []).compactMap(\.value)
    }

    init(desktop: Bool, boards: [LinkBoard]) { self.desktop = desktop; self.boards = boards }

    private enum CodingKeys: String, CodingKey { case desktop, boards }

    private struct Lossy<T: Decodable>: Decodable {
        let value: T?
        init(from decoder: Decoder) throws { value = try? T(from: decoder) }
    }
}

/// What a Stop tap ended as. `uncertain` is never shown as stopped.
enum LinkBoardStopResult: Equatable, Sendable {
    case stopped
    case uncertain
    case boardOffline
    case desktopOffline
    case failed(String)

    static func from(status: Int, data: Data) -> LinkBoardStopResult {
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let code = obj?["error"] as? String
        switch (status, code) {
        case (200..<300, _): return .stopped
        case (_, "uncertain"): return .uncertain
        case (_, "board_offline"): return .boardOffline
        case (_, "desktop_offline"): return .desktopOffline
        default: return status == 502 ? .uncertain : .failed(obj?["message"] as? String ?? "Stop failed (HTTP \(status)).")
        }
    }
}

enum LinkBoardsOutcome: Equatable, Sendable {
    case boards([LinkBoard])
    /// An older runtime answered 404: hide the section.
    case unsupported
    /// Desktop is not running, or the runtime did not answer.
    case unreachable
}

/// Reaches the boards through the runtime this phone is paired with, the way
/// `PhoneTaskClient` reaches the phone routes.
final class LinkBoardsClient: @unchecked Sendable {
    static let shared = LinkBoardsClient()
    private let api: APIClient
    init(api: APIClient = .shared) { self.api = api }

    func boards() async -> LinkBoardsOutcome {
        guard let (data, status) = try? await send("phones/boards", method: "GET") else { return .unreachable }
        switch status {
        case 404: return .unsupported
        case 200..<300:
            guard let list = try? JSONDecoder().decode(LinkBoardList.self, from: data) else { return .unreachable }
            return list.desktop ? .boards(list.boards) : .unreachable
        default: return .unreachable
        }
    }

    func stop(boardId: String) async -> LinkBoardStopResult {
        let id = boardId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(["/"])) ?? boardId
        guard let (data, status) = try? await send("phones/boards/\(id)/stop", method: "POST") else {
            // No answer: we do not know whether it stopped.
            return .uncertain
        }
        return LinkBoardStopResult.from(status: status, data: data)
    }

    private func send(_ path: String, method: String) async throws -> (Data, Int) {
        var request = try await api.authorizedRequest(path: path, method: method)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 20
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: api.session)
        return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
    }
}
