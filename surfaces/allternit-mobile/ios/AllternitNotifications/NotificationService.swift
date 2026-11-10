import UserNotifications

/// Notification service extension. For now it hands every push through
/// unchanged; fetching private content, the sender avatar and the
/// communication intent come later (plan step I2).
final class NotificationService: UNNotificationServiceExtension {
    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestAttempt: UNNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        self.contentHandler = contentHandler
        bestAttempt = request.content
        contentHandler(request.content)
        self.contentHandler = nil
    }

    /// iOS is about to kill the extension: deliver what we have.
    override func serviceExtensionTimeWillExpire() {
        if let contentHandler, let bestAttempt {
            contentHandler(bestAttempt)
        }
        contentHandler = nil
    }
}
