import Foundation

/// What the app shows about the ChatGPT sign-in on this iPhone: signed in or
/// not, the plan's models, and whether the plan needs the person (usage
/// limit, expired sign-in). Tokens live in `ChatGPTAuthService`, never here.
@MainActor
final class ChatGPTDeviceStore: ObservableObject {
    static let shared = ChatGPTDeviceStore()

    enum Needs: Equatable, Sendable {
        case usageLimit
        case loginExpired

        var reason: String {
            switch self {
            case .usageLimit: return "ChatGPT limit reached"
            case .loginExpired: return "Sign-in expired"
            }
        }

        var code: String {
            switch self {
            case .usageLimit: return "usage_limit"
            case .loginExpired: return "login_expired"
            }
        }
    }

    @Published private(set) var signedIn = false
    @Published private(set) var email: String?
    @Published private(set) var plan: String?
    @Published private(set) var models: [ChatGPTModel] = []
    @Published private(set) var needs: Needs?
    @Published private(set) var modelsError: String?

    private var loadingModels = false

    init() {
        Task { await reload() }
    }

    /// Re-reads the Keychain (after sign-in, sign-out or launch).
    func reload() async {
        let session = await ChatGPTAuthService.shared.session()
        signedIn = session != nil
        email = session?.email
        plan = session?.plan
        if session == nil { models = [] }
        if signedIn, models.isEmpty { await loadModels() }
    }

    func loadModels(force: Bool = false) async {
        guard signedIn, !loadingModels, force || models.isEmpty else { return }
        loadingModels = true
        defer { loadingModels = false }
        do {
            models = try await ChatGPTAuthService.shared.listModels()
            modelsError = nil
            if needs == .loginExpired { needs = nil }
        } catch ChatGPTAuthError.signedOut {
            markExpired()
        } catch {
            modelsError = "Couldn't load your ChatGPT models. \(ConnectionsClient.describe(error))"
        }
    }

    func signedIn(_ session: ChatGPTSession) async {
        signedIn = true
        email = session.email
        plan = session.plan
        needs = nil
        await loadModels(force: true)
    }

    /// Returns false when OpenAI didn't confirm the revoke (tokens are gone
    /// from the phone either way).
    func signOut() async -> Bool {
        let confirmed = await ChatGPTAuthService.shared.signOut()
        signedIn = false
        email = nil
        plan = nil
        models = []
        needs = nil
        return confirmed
    }

    func markLimitReached() { needs = .usageLimit }

    func markExpired() {
        needs = .loginExpired
        Task { await reload() }
    }

    /// A reply came back, so the plan works again.
    func markWorking() { if needs != nil { needs = nil } }

    // MARK: - Disclosure (contract §9.9: shown before the first connect)

    private static let acceptedKey = "allternit.chatgpt-device.disclosure-accepted"

    var disclosureAccepted: Bool {
        UserDefaults.standard.string(forKey: Self.acceptedKey) == ChatGPTDeviceDisclosure.text.version
    }

    func acceptDisclosure() {
        UserDefaults.standard.set(ChatGPTDeviceDisclosure.text.version, forKey: Self.acceptedKey)
        objectWillChange.send()
    }
}

/// The note before the first ChatGPT sign-in on this iPhone. It follows the
/// §9.8 disclosure (same version, same buttons) but says what is true here:
/// the sign-in and the chat stay on the phone, nothing runs on an Allternit
/// server. Approved 2026-11-14.
enum ChatGPTDeviceDisclosure {
    static let text = ConnDisclosure(
        version: "2026-11-14.1",
        title: "Before you connect your ChatGPT plan",
        body: [
            "You sign in to ChatGPT yourself, on this iPhone. Your sign-in is saved in this iPhone's Keychain. Allternit never receives it.",
            "ChatGPT's terms may not allow using a plan this way. ChatGPT could limit or close your account. Allternit does not control that and cannot undo it.",
            "Messages you send with a ChatGPT model go from this iPhone straight to OpenAI and count toward your plan's limits.",
            "Your plan only answers messages you send yourself, in chat. Bots, routines, schedules and agents never use it.",
            "Disconnect signs you out at OpenAI and deletes the sign-in from this iPhone.",
        ],
        acceptLabel: "I understand, connect",
        declineLabel: "Use a key instead"
    )
}
