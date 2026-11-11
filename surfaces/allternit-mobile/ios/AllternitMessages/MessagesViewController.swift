import Messages
import SwiftUI
import UIKit

/// The iMessage app (`com.allternit.mobile.messages`). Opens on the quick
/// panel (compact presentation). Reads only the App Group: the read token
/// (`SharedAccess`) and the last phone run the app wrote.
final class MessagesViewController: MSMessagesAppViewController {
    private var host: UIHostingController<AnyView>?

    override func viewDidLoad() {
        super.viewDidLoad()
        mount()
    }

    override func willBecomeActive(with conversation: MSConversation) {
        super.willBecomeActive(with: conversation)
        mount()
    }

    private func mount() {
        host?.view.removeFromSuperview()
        host?.removeFromParent()
        let panel = IMQuickPanel(open: { [weak self] url in self?.extensionContext?.open(url, completionHandler: nil) })
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
