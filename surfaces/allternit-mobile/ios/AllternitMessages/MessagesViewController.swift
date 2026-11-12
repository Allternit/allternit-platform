import Messages
import SwiftUI
import UIKit

/// The iMessage app (`com.allternit.mobile.messages`). Opens on the quick
/// panel (compact presentation). Reads only the App Group: the read token
/// (`SharedAccess`) and the last phone run the app wrote. A run started here
/// posts one card that `IMRunController` updates in place while the panel is open.
final class MessagesViewController: MSMessagesAppViewController {
    private var host: UIHostingController<AnyView>?
    private let live = IMRunController()

    override func viewDidLoad() {
        super.viewDidLoad()
        mount()
    }

    override func willBecomeActive(with conversation: MSConversation) {
        super.willBecomeActive(with: conversation)
        live.attach(conversation)
        mount()
        live.resume()
    }

    override func didResignActive(with conversation: MSConversation) {
        super.didResignActive(with: conversation)
        live.pause()
    }

    /// A card tapped while the app is open: show its run (or its caption for a recipient).
    override func didSelect(_ message: MSMessage, conversation: MSConversation) {
        super.didSelect(message, conversation: conversation)
        live.select(message, in: conversation)
    }

    /// The user sent a staged card update from the input field.
    override func didStartSending(_ message: MSMessage, conversation: MSConversation) {
        super.didStartSending(message, conversation: conversation)
        live.didSend(message)
    }

    private func mount() {
        host?.view.removeFromSuperview()
        host?.removeFromParent()
        let panel = IMQuickPanel(open: { [weak self] url in self?.extensionContext?.open(url, completionHandler: nil) }, live: live)
        let controller = UIHostingController(rootView: AnyView(panel))
        addChild(controller)
        controller.view.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(controller.view)
        NSLayoutConstraint.activate([
            controller.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            controller.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            controller.view.topAnchor.constraint(equalTo: view.topAnchor),
            controller.view.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        controller.didMove(toParent: self)
        host = controller
    }
}
