import Foundation

// MARK: - Capability kind

/// Kind of a Fabric capability, matching the server-side
/// `fabricCapabilityKindSchema` (`cmd/gizzi-code/src/runtime/fabric/transport.ts`).
enum CapabilityKind: String, Codable, Sendable {
    case read
    case write
    case execute
    case compute
    case observe
    case stream
}

// MARK: - Node endpoint

/// One reachable endpoint for a Fabric node.
struct NodeEndpoint: Codable, Sendable, Identifiable, Hashable {
    let transport: String
    let url: String
    let priority: Int
    let metadata: [String: AnyCodable]?

    var id: String { "\(transport)-\(url)" }

    enum CodingKeys: String, CodingKey {
        case transport, url, priority, metadata
    }
}

// MARK: - Capability

/// A capability published by a Fabric node.
struct FabricCapability: Codable, Sendable, Identifiable, Hashable {
    let id: String
    let name: String
    let version: String
    let kind: CapabilityKind
    let resource: String
    let description: String?
}

// MARK: - Node resource

/// An advertised resource on a Fabric node (CPU, memory, GPU, etc.).
struct NodeResource: Codable, Sendable, Identifiable, Hashable {
    let kind: String
    let name: String
    let value: AnyCodable?
    let unit: String?

    var id: String { "\(kind)-\(name)" }
}

// MARK: - Node identity

/// Full identity of a Fabric peer, returned by `GET /v1/fabric/peers` and
/// `GET /v1/fabric/peers/local`.
struct NodeIdentity: Codable, Sendable, Identifiable {
    let nodeId: String
    let name: String
    let runtimeType: String
    let platform: String
    let version: String
    let endpoints: [NodeEndpoint]
    let capabilities: [FabricCapability]
    let resources: [NodeResource]?

    var id: String { nodeId }

    enum CodingKeys: String, CodingKey {
        case nodeId, name, runtimeType, platform, version, endpoints, capabilities, resources
    }
}

// MARK: - Lease policy

/// Optional policy embedded in a `FabricLease`.
struct FabricLeasePolicy: Codable, Sendable {
    let workloadId: String?
    let principalId: String?
    let budgetId: String?
    let maxInvocations: Int?
    let extra: [String: AnyCodable]?
}

// MARK: - Lease

/// A signed capability grant issued by `POST /v1/fabric/leases`.
struct FabricLease: Codable, Sendable, Identifiable {
    let id: String
    let capabilityId: String
    let grantee: String
    let issuedAt: String
    let expiresAt: String?
    let status: String
    let constraints: [String: AnyCodable]?
    let policy: FabricLeasePolicy?
    let signature: String?

    var isActive: Bool { status == "active" }
}

// MARK: - Lease request

/// Body for `POST /v1/fabric/leases`.
struct IssueLeaseRequest: Codable, Sendable {
    let capabilityId: String
    let grantee: String
    let ttlSeconds: Int?
    let constraints: [String: AnyCodable]?
    let policy: FabricLeasePolicy?
}

// MARK: - Capability invocation

/// Body for `POST /v1/session-worker/invoke`.
struct CapabilityInvocationRequest: Codable, Sendable {
    let capability: String
    let inputs: [String: AnyCodable]
}

/// Result returned by `POST /v1/session-worker/invoke`.
struct InvocationResult: Codable, Sendable {
    // Optional: a computer's allternit-api answers `{"result":{...}}` only;
    // requiring these failed the decode after the message was accepted.
    let ok: Bool?
    let capability: String?
    let nodeId: String?
    let result: AnyCodable?
    let error: String?
    let leaseId: String?
}

// MARK: - Capability query

/// Query parameters for `GET /v1/fabric/peers`.
struct CapabilityQuery: Codable, Sendable {
    var name: String?
    var kind: CapabilityKind?
    var resource: String?
    var nodeId: String?
}

// MARK: - AnyCodable helper

/// A minimal type-erased Codable wrapper for the free-form `Record<string, unknown>`
/// values that appear in Fabric metadata, constraints, and policy extras.
/// It round-trips JSON primitives, arrays, and dictionaries without forcing
/// callers into `Any`.
///
/// `Sendable` is unchecked because the erased `Any` value may hold non-Sendable
/// Foundation types on decode; callers only ever construct these from plain
/// JSON-decodable primitives.
struct AnyCodable: Codable, @unchecked Sendable, Hashable {
    /// Sentinel for values that cannot be represented (e.g. raw JSON `null`).
    private enum Sentinel: Sendable {}

    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let bool = try? container.decode(Bool.self) {
            value = bool
        } else if let int = try? container.decode(Int.self) {
            value = int
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map { $0.value }
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues { $0.value }
        } else {
            value = Sentinel.self
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case let bool as Bool:
            try container.encode(bool)
        case let int as Int:
            try container.encode(int)
        case let double as Double:
            try container.encode(double)
        case let string as String:
            try container.encode(string)
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        case is Sentinel.Type:
            try container.encodeNil()
        default:
            try container.encodeNil()
        }
    }

    static func == (lhs: AnyCodable, rhs: AnyCodable) -> Bool {
        String(describing: lhs.value) == String(describing: rhs.value)
    }

    func hash(into hasher: inout Hasher) {
        hasher.combine(String(describing: value))
    }
}
