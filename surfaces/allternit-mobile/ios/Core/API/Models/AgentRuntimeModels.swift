import Foundation

// MARK: - Connector Binding

struct ConnectorBinding: Decodable, Sendable, Identifiable, Equatable {
    let provider: String
    let label: String?
    let autonomous: Bool
    let capabilities: [String]

    var id: String { provider }
    var displayName: String { label?.isEmpty == false ? label! : provider }

    enum CodingKeys: String, CodingKey {
        case provider, label, autonomous, capabilities
    }
}

// MARK: - Secret Ref

struct SecretRef: Decodable, Sendable, Identifiable, Equatable {
    let key: String
    let name: String?
    let required: Bool
    let vaultRef: String?

    var id: String { key }
    var displayName: String { name?.isEmpty == false ? name! : key }

    enum CodingKeys: String, CodingKey {
        case key, name, required
        case vaultRef = "vault_ref"
    }
}

// MARK: - Harness

struct HarnessConfig: Decodable, Sendable, Equatable {
    let mode: String
    let byok: [String: HarnessBYOKProvider]?

    enum CodingKeys: String, CodingKey {
        case mode, byok
    }
}

struct HarnessBYOKProvider: Decodable, Sendable, Equatable {
    let apiKey: String?

    enum CodingKeys: String, CodingKey {
        case apiKey = "api_key"
    }
}

// MARK: - VM Operator

struct VMOperatorConfig: Decodable, Sendable, Equatable {
    let enabled: Bool
    let provider: String?
    let image: String?
    let networkPolicy: String?
    let persistence: String?
    let vncEnabled: Bool
    let vncUrl: String?
    let allowedActions: [String]
    let resources: VMResources

    enum CodingKeys: String, CodingKey {
        case enabled, provider, image
        case networkPolicy = "network_policy"
        case persistence
        case vncEnabled = "vnc_enabled"
        case vncUrl = "vnc_url"
        case allowedActions = "allowed_actions"
        case resources
    }

    init(enabled: Bool = false, provider: String? = nil, image: String? = nil,
         networkPolicy: String? = nil, persistence: String? = nil, vncEnabled: Bool = false,
         vncUrl: String? = nil, allowedActions: [String] = [],
         resources: VMResources = VMResources()) {
        self.enabled = enabled
        self.provider = provider
        self.image = image
        self.networkPolicy = networkPolicy
        self.persistence = persistence
        self.vncEnabled = vncEnabled
        self.vncUrl = vncUrl
        self.allowedActions = allowedActions
        self.resources = resources
    }
}

struct VMResources: Decodable, Sendable, Equatable {
    let cpu: String?
    let memory: String?
    let disk: String?

    enum CodingKeys: String, CodingKey {
        case cpu, memory, disk
    }

    init(cpu: String? = nil, memory: String? = nil, disk: String? = nil) {
        self.cpu = cpu
        self.memory = memory
        self.disk = disk
    }
}

// MARK: - Identity Channels

struct IdentityChannels: Decodable, Sendable, Equatable {
    let email: IdentityEmailChannel?
    let phone: IdentityPhoneChannel?
    let wallet: IdentityWalletChannel?

    enum CodingKeys: String, CodingKey {
        case email, phone, wallet
    }
}

struct IdentityEmailChannel: Decodable, Sendable, Equatable {
    let address: String?
    let receiveEnabled: Bool

    enum CodingKeys: String, CodingKey {
        case address
        case receiveEnabled = "receive_enabled"
    }
}

struct IdentityPhoneChannel: Decodable, Sendable, Equatable {
    let number: String?
    let voiceEnabled: Bool
    let smsEnabled: Bool

    enum CodingKeys: String, CodingKey {
        case number
        case voiceEnabled = "voice_enabled"
        case smsEnabled = "sms_enabled"
    }
}

struct IdentityWalletChannel: Decodable, Sendable, Equatable {
    let address: String?

    enum CodingKeys: String, CodingKey {
        case address
    }
}
