#if DEBUG
import SwiftUI
import UIKit

/// Debug only: draws the phone cards with fixtures for screenshots
/// (`-phone-gallery <page>`, add `-phone-gallery-dark` for dark). Pages:
/// approval, approval2, status, status2, entry, intents, imessage, imessage2.
enum PhoneSurfacesGallery {
    private static var window: UIWindow?

    static func installIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        guard let i = args.firstIndex(of: "-phone-gallery"), i + 1 < args.count else { return }
        let page = args[i + 1]
        let dark = args.contains("-phone-gallery-dark")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            guard let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first else { return }
            let w = UIWindow(windowScene: scene)
            w.windowLevel = .alert + 1
            w.overrideUserInterfaceStyle = dark ? .dark : .light
            w.rootViewController = UIHostingController(rootView: GalleryPage(page: page))
            w.makeKeyAndVisible()
            window = w
        }
    }
}

private struct GalleryPage: View {
    let page: String

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(title).font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary"))
                content
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
        }
        .background(Color("BgSecondary").ignoresSafeArea())
    }

    private var title: String { "Fixture gallery · \(page)" }

    @ViewBuilder
    private var content: some View {
        switch page {
        case "approval":
            label("Inbox row (waiting)")
            InboxRowPreview(item: Fixtures.hold)
            ForEach([PhoneHoldReviewView.Phase.loading, .shown, .failed], id: \.self) { phase in
                label("Review pane · \(phase)")
                pane(phase)
            }
        case "approval2":
            ForEach([PhoneHoldReviewView.Phase.approved, .denied, .changed, .expired], id: \.self) { phase in
                label("Review pane · \(phase)")
                pane(phase)
            }
        case "status", "status2":
            let all = PhoneTaskCardModel.Status.allCases
            ForEach(page == "status" ? Array(all.prefix(4)) : Array(all.suffix(4)), id: \.self) { status in
                PhoneTaskCardView(card: Fixtures.card(status), loadThumb: nil,
                                  fixedThumb: [.running, .waitingApproval, .done].contains(status) ? Fixtures.screen : nil)
            }
        case "entry":
            label("Composer chip (armed)")
            PhoneArmedChip(name: "Pixel 8") {}
            label("“+” sheet row · states")
            RunOnPhoneRow(fixedState: .idle)
            RunOnPhoneRow(fixedState: .noPhone)
            RunOnPhoneRow(fixedState: .offline("Pixel 8"))
            RunOnPhoneRow(fixedState: .signedOut)
            RunOnPhoneRow(fixedState: .quota)
        case "intents":
            ForEach(Fixtures.intentLines, id: \.0) { line in
                label(line.0)
                Text(line.1)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 14))
            }
        case "imessage":
            label("Quick panel · ready, with the last phone run")
            IMQuickPanel(fixedState: .ready, fixedRun: Fixtures.sharedRun("waiting_approval"))
                .frame(height: 400).clipShape(RoundedRectangle(cornerRadius: 16))
            IMPhoneRunCard(run: Fixtures.sharedRun("done"))
            IMPhoneRunCard(run: Fixtures.sharedRun("refused"))
        default:
            ForEach([IMAccountState.signedOut, .expired, .offline, .quota], id: \.self) { state in
                label("Quick panel · \(state)")
                IMQuickPanel(fixedState: state).frame(height: 150).clipShape(RoundedRectangle(cornerRadius: 16))
            }
        }
    }

    private func pane(_ phase: PhoneHoldReviewView.Phase) -> some View {
        PhoneHoldReviewView(item: Fixtures.hold, fixedPhase: phase, fixedImage: Fixtures.screen, cropHeight: 150)
            .clipShape(RoundedRectangle(cornerRadius: 16))
    }

    private func label(_ text: String) -> some View {
        Text(text).font(.caption2).foregroundColor(Color("TextTertiary"))
    }
}

/// The inbox row's look for a phone hold (the real row is private to InboxSheet).
private struct InboxRowPreview: View {
    let item: InboxItem
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("APPROVAL · PHONE  \(item.botName)").font(.system(size: 11, weight: .semibold)).foregroundColor(Color("TextSecondary"))
            Text(item.summary).font(.subheadline).foregroundColor(Color("TextPrimary"))
            HStack {
                Button("Review") {}.buttonStyle(.borderedProminent).tint(Color("AccentPrimary")).controlSize(.small)
                Button("Deny") {}.buttonStyle(.bordered).controlSize(.small)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 14))
    }
}

private enum Fixtures {
    static let hold: InboxItem = {
        let json = """
        {"id":"approval:ap1","kind":"approval","botId":"b1","botName":"Errand bot","threadId":"t1",
         "summary":"Errand bot needs approval on Pixel 8: Tap Place order · $42.18","at":"2026-11-10T15:00:00Z",
         "primary":{"action":"approve","label":"Approve"},"actions":["approve","deny"],"contentHash":"h1",
         "subkind":"phone_action","detail":{"phone":{"serial":"link:1","phoneName":"Pixel 8","app":"DoorDash",
         "effect":"pay","tool":"phone.tap","intent":"Tap Place order","goal":"Order dinner: 2 pad thai and spring rolls from Thai Basil",
         "runId":"task-1","step":6,"element":{"label":"Place order · $42.18"}}}}
        """
        return try! JSONDecoder().decode(InboxItem.self, from: Data(json.utf8))
    }()

    static func card(_ status: PhoneTaskCardModel.Status) -> PhoneTaskCardModel {
        let lines: [PhoneTaskCardModel.Status: String] = [.running: "Adding spring rolls", .done: "Order confirmed", .failed: "Could not find the cart button."]
        let steps: [PhoneTaskCardModel.Status: Int] = [.running: 4, .waitingApproval: 6, .done: 7, .denied: 6, .approvalTimeout: 6, .phoneLocked: 3, .failed: 5, .offline: 3]
        return PhoneTaskCardModel(runId: "task-\(status.rawValue)", serial: "link:1",
                                  goal: "Order dinner on DoorDash: 2 pad thai and spring rolls, under $45",
                                  status: status, step: steps[status] ?? 1, inboxId: "approval:ap1",
                                  phoneName: "Pixel 8", line: lines[status])
    }

    static func sharedRun(_ status: String) -> SharedPhoneRun {
        var r = SharedPhoneRun(runId: "task-1", goal: "Turn on Do Not Disturb until 9", status: status, step: 2, phoneName: "Pixel 8", updatedAt: Date())
        if status == "refused" { r.problem = "refused"; r.message = "Your bots are paused, so I didn't start this phone task. Resume them and ask again." }
        return r
    }

    static let intentLines: [(String, String)] = [
        ("Run a Phone Task · linked", PhoneIntentText.started(goal: "Turn on Do Not Disturb", phone: PhoneInfo(serial: "link:1", name: "Pixel 8", online: true))),
        ("Run a Phone Task · signed out", IntentFailure.signedOut.dialog),
        ("Run a Phone Task · no phone linked", PhoneIntentFailure.noPhone.dialog),
        ("Run a Phone Task · refused", PhoneIntentFailure.refused("Your bots are paused, so I didn't start this phone task. Resume them and ask again.").dialog),
        ("Run a Phone Routine · linked", PhoneIntentText.routineStarted("Morning check", phone: PhoneInfo(serial: "link:1", name: "Pixel 8", online: true))),
        ("Run a Phone Routine · phone offline", PhoneIntentFailure.phoneOffline("Pixel 8").dialog),
        ("Phone Task Status · waiting", PhoneIntentText.status(PhoneRunStatusInfo(runId: "r", goal: "Order dinner", status: "waiting_approval", phoneName: "Pixel 8"))),
        ("Phone Task Status · done", PhoneIntentText.status(PhoneRunStatusInfo(runId: "r", goal: "Order dinner", status: "done", phoneName: "Pixel 8"))),
    ]

    /// A drawn phone screen standing in for the crop and the thumbnail.
    static let screen: UIImage = {
        let size = CGSize(width: 360, height: 760)
        return UIGraphicsImageRenderer(size: size).image { ctx in
            UIColor.white.setFill(); ctx.fill(CGRect(origin: .zero, size: size))
            let title: [NSAttributedString.Key: Any] = [.font: UIFont.boldSystemFont(ofSize: 26), .foregroundColor: UIColor.black]
            let body: [NSAttributedString.Key: Any] = [.font: UIFont.systemFont(ofSize: 18), .foregroundColor: UIColor.darkGray]
            ("Thai Basil" as NSString).draw(at: CGPoint(x: 24, y: 60), withAttributes: title)
            ("Pad thai × 2        $28.00" as NSString).draw(at: CGPoint(x: 24, y: 130), withAttributes: body)
            ("Spring rolls          $7.50" as NSString).draw(at: CGPoint(x: 24, y: 170), withAttributes: body)
            ("Delivery + fees      $6.68" as NSString).draw(at: CGPoint(x: 24, y: 210), withAttributes: body)
            let button = CGRect(x: 24, y: 640, width: 312, height: 64)
            UIColor(red: 0.85, green: 0.15, blue: 0.1, alpha: 1).setFill()
            UIBezierPath(roundedRect: button, cornerRadius: 32).fill()
            let b: [NSAttributedString.Key: Any] = [.font: UIFont.boldSystemFont(ofSize: 22), .foregroundColor: UIColor.white]
            ("Place order · $42.18" as NSString).draw(at: CGPoint(x: 72, y: 657), withAttributes: b)
        }
    }()
}
#endif
