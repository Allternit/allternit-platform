import Foundation
import MessageUI
import UIKit

/// Result of handing a bot-prepared text to the system. Only `.sent` may
/// show "Sent" on the card.
enum MessageComposeOutcome: Equatable, Sendable {
    case sent
    case cancelled
    case failed
    /// Opened through an `sms:` URL: the app cannot know whether the user sent it.
    case handedOff
}

/// Presents the system Messages sheet prefilled with a recipient and body.
/// Nothing is sent without the user's own tap on Send in that sheet.
@MainActor
final class MessageComposer: NSObject, MFMessageComposeViewControllerDelegate {
    private static var active: MessageComposer?
    private var completion: ((MessageComposeOutcome) -> Void)?

    /// `sms:<number>&body=<encoded>`; the fallback when `canSendText()` is false.
    nonisolated static func smsURL(to: String, text: String) -> URL? {
        let number = to.filter { $0.isNumber || $0 == "+" }
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&+=#?%/;:@$,")
        guard let body = text.addingPercentEncoding(withAllowedCharacters: allowed) else { return nil }
        return URL(string: "sms:\(number)&body=\(body)")
    }

    static func present(to: String, text: String, completion: @escaping (MessageComposeOutcome) -> Void) {
        guard MFMessageComposeViewController.canSendText(), let top = topViewController() else {
            guard let url = smsURL(to: to, text: text) else { completion(.failed); return }
            UIApplication.shared.open(url) { ok in
                Task { @MainActor in completion(ok ? .handedOff : .failed) }
            }
            return
        }
        let composer = MessageComposer()
        composer.completion = completion
        active = composer
        let vc = MFMessageComposeViewController()
        vc.recipients = [to]
        vc.body = text
        vc.messageComposeDelegate = composer
        top.present(vc, animated: true)
    }

    nonisolated func messageComposeViewController(_ controller: MFMessageComposeViewController,
                                                  didFinishWith result: MessageComposeResult) {
        MainActor.assumeIsolated {
            let outcome: MessageComposeOutcome
            switch result {
            case .sent: outcome = .sent
            case .cancelled: outcome = .cancelled
            default: outcome = .failed
            }
            controller.dismiss(animated: true)
            let done = completion
            completion = nil
            MessageComposer.active = nil
            done?(outcome)
        }
    }

    private static func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .first { $0.activationState == .foregroundActive }
        var top = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
        while let next = top?.presentedViewController { top = next }
        return top
    }
}
