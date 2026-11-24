import Foundation

/// Every failure AllternitFilesKit throws. Server errors keep the contract's
/// `ErrorBody`, so callers switch on `ErrorCode` instead of parsing text.
public enum FilesError: Error, Sendable, Equatable {
    /// No token in the shared keychain (the app has not set Files up, or the
    /// user signed out).
    case signedOut
    /// The runtime or cloud-api answered with a `files/2` `ErrorBody`.
    case api(status: Int, body: ErrorBody)
    /// Content needs the computer awake (409 `wake_consent_required`). Show
    /// the wake prompt from `hint`, then call `requestWake`, then retry.
    case wakeConsentRequired(hint: WakeHint?, state: ComputerState?)
    /// cloud-api's relay could not reach the runtime (`runtime_offline`).
    case unreachable
    /// cloud-api's relay is starting the computer (`runtime_warming`).
    case warming(retryAfter: TimeInterval)
    /// Endpoints listed no transport this client may use (for example a
    /// mesh-only computer seen from the relay-only File Provider extension).
    case noTransport(available: [TransportKind])
    /// A non-2xx answer with no `files/2` body.
    case http(status: Int, message: String?)
    /// The content changed on the computer while it was downloading.
    case revisionChanged(expected: Rev, served: Rev)
    /// A downloaded file's BLAKE3 did not match `content_hash`.
    case integrity(expected: Blake3, actual: Blake3)
    case invalidResponse(String)
    /// This device has no Files key that opens the data (not approved yet,
    /// or the data was sealed with another key). SPEC `key_unavailable`.
    case keyUnavailable(String)
    /// No replica could answer (none, not current, host asleep). SPEC
    /// `replica_unavailable`.
    case replicaUnavailable(String)
    /// 403 `whole_disk_off`: the path is outside the Files drive and the owner
    /// has not turned on whole-disk browsing for this computer. Carries the
    /// server's own plain message.
    case wholeDiskOff(message: String)

    /// The contract error code, when there is one.
    public var code: ErrorCode? {
        switch self {
        case .api(_, let body): return body.code
        case .wakeConsentRequired: return .wakeConsentRequired
        case .keyUnavailable: return .keyUnavailable
        case .replicaUnavailable: return .replicaUnavailable
        case .integrity: return .integrityFailed
        default: return nil
        }
    }

    /// The `ErrorBody` when the server sent one.
    public var body: ErrorBody? {
        if case .api(_, let body) = self { return body }
        return nil
    }

    /// True for 403 `whole_disk_off`: Files offers a way to the switch.
    public var isWholeDiskOff: Bool {
        if case .wholeDiskOff = self { return true }
        return false
    }

    /// True for auth failures the app fixes by minting a new Files token.
    public var isAuthFailure: Bool {
        switch self {
        case .signedOut: return true
        case .api(let status, let body): return status == 401 || body.code == .unauthorized
        case .http(let status, _): return status == 401
        default: return false
        }
    }

    /// True when the computer is asleep, waking or offline: show the asleep
    /// state, keep cached items, don't treat it as a failure.
    public var isComputerUnavailable: Bool {
        switch self {
        case .unreachable, .warming: return true
        case .api(_, let body):
            return [.asleep, .waking, .offline].contains(body.code)
        default: return false
        }
    }
}

extension FilesError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .signedOut: return "Sign in to the Allternit app to use Files."
        case .api(_, let body): return body.message
        case .wakeConsentRequired: return "This computer is asleep. Wake it to open this file."
        case .unreachable: return "This computer isn't responding."
        case .warming: return "This computer is waking up."
        case .noTransport: return "This computer can't be reached from here. Open it in the Allternit app."
        case .http(let status, let message): return message ?? "The server answered \(status)."
        case .revisionChanged: return "The file changed while it was downloading."
        case .integrity: return "The downloaded file didn't match its checksum."
        case .invalidResponse(let what): return "Unexpected answer from the server (\(what))."
        case .keyUnavailable(let why): return why
        case .replicaUnavailable(let why): return why
        case .wholeDiskOff(let message): return message
        }
    }
}

enum ErrorDecoding {
    /// Turns a non-2xx answer into a `FilesError`: a contract `ErrorBody`
    /// first, then cloud-api's own relay bodies
    /// (`{"error":"runtime_offline"}`, `{"error":"runtime_warming","retryAfterSeconds":N}`).
    static func error(status: Int, data: Data) -> FilesError {
        if status == 403,
           let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
           object["error"] as? String == "whole_disk_off" {
            let text = (object["message"] as? String) ?? ""
            return .wholeDiskOff(message: text.isEmpty
                ? "This folder is outside the Files drive. Turn on whole-disk browsing for this computer first."
                : text)
        }
        if let body = try? JSONDecoder().decode(ErrorBody.self, from: data) {
            if body.code == .wakeConsentRequired {
                return .wakeConsentRequired(hint: body.wake, state: body.computerState)
            }
            return .api(status: status, body: body)
        }
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let error = object?["error"] as? String
        switch error {
        case "runtime_offline", "runtime_unavailable":
            return .unreachable
        case "runtime_warming":
            let after = (object?["retryAfterSeconds"] as? Double) ?? 3
            return .warming(retryAfter: min(max(after, 1), 5))
        default:
            let message = (object?["message"] as? String) ?? (object?["detail"] as? String) ?? error
            return .http(status: status, message: message)
        }
    }
}
