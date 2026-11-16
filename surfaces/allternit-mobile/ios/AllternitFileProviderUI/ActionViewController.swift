import FileProvider
import FileProviderUI
import SwiftUI
import UIKit
import AllternitFilesKit

/// The File Provider UI extension (plan step 20, design 5B "Sheets"): what
/// Files shows over an Allternit location when the extension needs the
/// user, and for our four actions.
///
/// - `prepare(forError:)`: the File Provider answered `.notAuthenticated`.
///   Its `userInfo` (or the pending prompt it left in `FilesSharedStore`)
///   says which sheet: wake consent, Face ID unlock of a protected root, or
///   sign in.
/// - `prepare(forAction:)`: "Send to agent", "Share with…", "Summarize",
///   "Keep offline" / "Don't keep offline" (Info.plist
///   `NSExtensionFileProviderActions`).
///
/// It talks to the computer through the same relay-only `FilesClient` as
/// the File Provider (no mesh in an extension, spike 5) and never wakes a
/// computer without the user tapping Wake.
final class ActionViewController: FPUIActionExtensionViewController {
    private var hosting: UIHostingController<AnyView>?

    override func prepare(forAction actionIdentifier: String, itemIdentifiers: [NSFileProviderItemIdentifier]) {
        let context = makeContext()
        guard let action = FilesUIAction(rawValue: actionIdentifier), let itemId = itemIdentifiers.first?.rawValue,
              context.client != nil else {
            show(AnyView(SignInSheet(context: context, message: nil)))
            return
        }
        show(AnyView(ActionSheet(context: context, action: action, itemId: itemId)))
    }

    override func prepare(forError error: Error) {
        let context = makeContext()
        let nsError = error as NSError
        let message = nsError.userInfo[NSLocalizedDescriptionKey] as? String
        switch PromptRouting.kind(for: nsError, store: context.store, computerId: context.computerId) {
        case .wake:
            show(AnyView(WakeSheet(context: context)))
        case .unlock:
            show(AnyView(UnlockSheet(context: context)))
        case .signIn:
            show(AnyView(SignInSheet(context: context, message: message)))
        }
    }

    private func makeContext() -> SheetContext {
        let domain = extensionContext.domainIdentifier
        let context = SheetContext(domain: domain)
        context.finish = { [weak self] in self?.extensionContext.completeRequest() }
        context.cancel = { [weak self] in
            self?.extensionContext.cancelRequest(withError: NSError(domain: FPUIErrorDomain,
                                                                    code: Int(FPUIExtensionErrorCode.userCancelled.rawValue)))
        }
        context.openApp = { [weak self] url, done in self?.extensionContext.open(url, completionHandler: done) }
        return context
    }

    private func show(_ root: AnyView) {
        hosting?.willMove(toParent: nil)
        hosting?.view.removeFromSuperview()
        hosting?.removeFromParent()
        let controller = UIHostingController(rootView: root)
        controller.view.backgroundColor = .clear
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
        hosting = controller
    }
}

/// The action ids in Info.plist.
enum FilesUIAction: String {
    case sendToAgent = "com.allternit.mobile.files.action.sendToAgent"
    case share = "com.allternit.mobile.files.action.share"
    case summarize = "com.allternit.mobile.files.action.summarize"
    case keepOffline = "com.allternit.mobile.files.action.keepOffline"
    case stopOffline = "com.allternit.mobile.files.action.stopOffline"
}

/// What a sheet needs: the computer this domain shows, a relay-only client
/// for it, and how to close.
final class SheetContext {
    let store: FilesSharedStore
    let domain: NSFileProviderDomainIdentifier?
    let computerId: ComputerId?
    let computerName: String
    let client: FilesClient?
    var finish: () -> Void = {}
    var cancel: () -> Void = {}
    var openApp: (URL, ((Bool) -> Void)?) -> Void = { _, done in done?(false) }

    init(domain: NSFileProviderDomainIdentifier?) {
        store = FilesSharedStore.fromMainBundle()
        self.domain = domain
        computerId = domain.flatMap(FilesDomains.computerId(for:))
        computerName = computerId.flatMap { store.computer($0)?.name } ?? "your computer"
        if let computerId, store.token() != nil {
            client = store.makeClient(computerId: computerId)
        } else {
            client = nil
        }
    }

    /// Asks Files to look at this computer again (badges, pins, a woken
    /// computer's content).
    func signalChanges() {
        guard let computerId else { return }
        Task { await FilesDomains.signalChanges(computerId) }
    }
}

/// Which prompt sheet an error asks for.
enum PromptRouting {
    static func kind(for error: NSError, store: FilesSharedStore, computerId: ComputerId?) -> FilesPromptKind {
        if let raw = error.userInfo[FilesPromptKind.userInfoKey] as? String, let kind = FilesPromptKind(rawValue: raw) {
            return kind
        }
        // The system may drop custom userInfo keys on the way here: fall back
        // to what the extension left in the shared store.
        if store.token() == nil || store.tokenRejected { return .signIn }
        guard let computerId else { return .signIn }
        let unlock = store.pendingUnlock(computerId: computerId)
        let wake = store.pendingWake(computerId: computerId)
        switch (unlock, wake) {
        case let (unlock?, wake?): return unlock.at > wake.at ? .unlock : .wake
        case (.some, nil): return .unlock
        case (nil, .some): return .wake
        default: return .signIn
        }
    }
}
