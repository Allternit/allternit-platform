import Intents
import UserNotifications

/// Notification service extension (plan step I2).
///
/// For a push whose `data.private` is true the APNs body is generic; this
/// fetches the real text by event id with the device read token
/// (`GET /api/v1/notifications/content/:eventId`, device_read.rs), attaches
/// the bot's avatar, and, only when the cloud says `communication`, donates
/// an `INSendMessageIntent` so iOS shows a communication notification with the
/// bot as sender. Any failure or the time limit delivers the original
/// content: a notification is never dropped.
final class NotificationService: UNNotificationServiceExtension {
    /// Own budget, under the roughly 30 s iOS allows.
    private static let budget: Duration = .seconds(20)

    private let delivery = Delivery()

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        delivery.begin(original: request.content, handler: contentHandler)

        let info = request.content.userInfo
        let data = info["data"] as? [String: Any]
        guard data?["private"] as? Bool == true,
              let eventId = data?["eventId"] as? String, !eventId.isEmpty else {
            delivery.finish(request.content)
            return
        }

        let delivery = self.delivery
        let original = UnsafeContent(request.content)
        let work = Task {
            let result = await Self.enrich(original.value, eventId: eventId)
            delivery.finish(result)
        }
        Task {
            try? await Task.sleep(for: Self.budget)
            work.cancel()
            delivery.finish(nil)
        }
    }

    /// iOS is about to kill the extension: deliver what we have.
    override func serviceExtensionTimeWillExpire() {
        delivery.finish(nil)
    }

    // MARK: Fetch + build

    private struct Content: Decodable {
        struct Sender: Decodable {
            var name: String?
            var avatarUrl: String?
        }
        var title: String?
        var body: String?
        var sender: Sender?
        var communication: Bool?
    }

    /// The fetched content applied to a copy of `original`, or `original`
    /// itself when anything fails.
    private static func enrich(_ original: UNNotificationContent, eventId: String) async -> UNNotificationContent {
        guard let token = SharedAccess.readToken(),
              let content = await fetch(eventId: eventId, token: token) else { return original }
        let mutable = (original.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        if let t = content.title, !t.isEmpty { mutable.title = t }
        if let b = content.body, !b.isEmpty { mutable.body = b }

        var avatarData: Data?
        if let s = content.sender?.avatarUrl, let url = URL(string: s), url.scheme == "https",
           let (data, ext) = await download(url) {
            avatarData = data
            if let attachment = attachment(data: data, ext: ext) { mutable.attachments = [attachment] }
        }

        guard content.communication == true else { return mutable }
        let name = content.sender?.name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let botId = ((original.userInfo["data"] as? [String: Any])?["botId"] as? String) ?? name
        guard !name.isEmpty, let updated = await communicationContent(
            from: mutable, name: name, botId: botId, avatar: avatarData
        ) else { return mutable }
        return updated
    }

    private static func fetch(eventId: String, token: String) async -> Content? {
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        guard let escaped = eventId.addingPercentEncoding(withAllowedCharacters: allowed) else { return nil }
        var req = URLRequest(url: AppGroupStore.cloudBaseURL.appendingPathComponent("api/v1/notifications/content/\(escaped)"))
        req.httpMethod = "GET"
        req.timeoutInterval = 10
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        guard let (data, response) = try? await URLSession.shared.data(for: req),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return try? JSONDecoder().decode(Content.self, from: data)
    }

    private static func download(_ url: URL) async -> (Data, String)? {
        var req = URLRequest(url: url)
        req.timeoutInterval = 8
        guard let (data, response) = try? await URLSession.shared.data(for: req),
              let http = response as? HTTPURLResponse, http.statusCode == 200,
              !data.isEmpty, data.count <= 2_000_000 else { return nil }
        let mime = http.mimeType ?? ""
        let ext = mime.contains("png") ? "png" : mime.contains("gif") ? "gif" : mime.contains("jpeg") || mime.contains("jpg") ? "jpg" : "png"
        return (data, ext)
    }

    /// Writes the avatar to a temp file (the system moves it) and wraps it.
    private static func attachment(data: Data, ext: String) -> UNNotificationAttachment? {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("avatar-\(UUID().uuidString)", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let file = dir.appendingPathComponent("avatar.\(ext)")
            try data.write(to: file)
            return try UNNotificationAttachment(identifier: "avatar", url: file)
        } catch { return nil }
    }

    /// Donates the incoming-message intent and returns the content updated
    /// from it. The sender is labelled as an assistant.
    private static func communicationContent(
        from content: UNMutableNotificationContent, name: String, botId: String, avatar: Data?
    ) async -> UNNotificationContent? {
        let handle = INPersonHandle(value: "bot:\(botId)", type: .unknown)
        let image = avatar.map { INImage(imageData: $0) }
        let sender = INPerson(
            personHandle: handle,
            nameComponents: nil,
            displayName: "\(name) (assistant)",
            image: image,
            contactIdentifier: nil,
            customIdentifier: "bot:\(botId)"
        )
        let intent = INSendMessageIntent(
            recipients: nil,
            outgoingMessageType: .outgoingMessageText,
            content: content.body,
            speakableGroupName: nil,
            conversationIdentifier: content.threadIdentifier.isEmpty ? "bot-\(botId)" : content.threadIdentifier,
            serviceName: nil,
            sender: sender,
            attachments: nil
        )
        if image != nil { intent.setImage(image, forParameterNamed: \.sender) }
        let interaction = INInteraction(intent: intent, response: nil)
        interaction.direction = .incoming
        try? await interaction.donate()
        return try? content.updating(from: intent)
    }
}

// MARK: - Once-only delivery

/// UNNotificationContent is not Sendable; this carries it across a task.
private struct UnsafeContent: @unchecked Sendable {
    let value: UNNotificationContent
    init(_ value: UNNotificationContent) { self.value = value }
}

/// Holds the system's handler and calls it exactly once, whichever of the
/// fetch, the 20 s budget or `serviceExtensionTimeWillExpire` gets there
/// first. `finish(nil)` delivers the original content.
private final class Delivery: @unchecked Sendable {
    private let lock = NSLock()
    private var handler: ((UNNotificationContent) -> Void)?
    private var original: UNNotificationContent?

    func begin(original: UNNotificationContent, handler: @escaping (UNNotificationContent) -> Void) {
        lock.lock(); defer { lock.unlock() }
        self.original = original
        self.handler = handler
    }

    func finish(_ content: UNNotificationContent?) {
        lock.lock()
        let h = handler, o = original
        handler = nil
        lock.unlock()
        guard let h else { return }
        h(content ?? o ?? UNNotificationContent())
    }
}
