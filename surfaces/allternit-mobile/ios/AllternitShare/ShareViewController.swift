import UIKit

/// Share extension placeholder: closes at once. Saving the shared items to
/// the outbox and picking a bot come later (plan step I7).
final class ShareViewController: UIViewController {
    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        extensionContext?.completeRequest(returningItems: [], completionHandler: nil)
    }
}
