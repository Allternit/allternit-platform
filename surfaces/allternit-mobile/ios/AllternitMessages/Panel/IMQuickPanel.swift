import SwiftUI

/// What the quick panel can tell about the account, from the App Group token.
enum IMAccountState: Equatable {
    case checking, ready, signedOut, offline, expired, quota
}

/// The iMessage app's quick panel (Eoj's pick, 2026-11-10): the mock's five
/// actions plus "Run on my phone" with its status card. Actions that need the
/// signed-in session open Allternit with the request filled in; nothing is
/// sent on its own.
struct IMQuickPanel: View {
    var fixedState: IMAccountState? = nil
    var fixedRun: SharedPhoneRun? = nil
    /// Opens an `allternit://` link in the app (the view controller's extension context).
    var open: (URL) -> Void = { _ in }
    /// The live card for this conversation (nil in the app's gallery).
    var live: IMRunController? = nil

    @State private var state: IMAccountState = .checking
    @State private var run: SharedPhoneRun?
    @State private var phoneGoal = ""
    @State private var picked: IMAction? = nil

    enum IMAction: String, CaseIterable, Identifiable {
        case draft = "Draft a reply", research = "Research", image = "Create an image"
        case share = "Share an artifact", task = "Check a task", phone = "Run on my phone"
        var id: String { rawValue }
        var icon: String {
            switch self {
            case .draft: return "bubble.left"
            case .research: return "magnifyingglass"
            case .image: return "photo"
            case .share: return "doc.text"
            case .task: return "checklist"
            case .phone: return "iphone"
            }
        }
        var prompt: String {
            switch self {
            case .draft: return "Write a friendly reply to this conversation."
            case .research: return "Help us decide. What should we compare?"
            case .image: return "Make an illustrated invitation for this plan."
            case .share: return "Share a summary of my saved plan."
            case .task, .phone: return ""
            }
        }
    }

    private var current: IMAccountState { fixedState ?? state }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text("A://")
                    .font(.system(size: 13, weight: .heavy, design: .monospaced))
                    .foregroundColor(Color("BgPrimary"))
                    .lineLimit(1).minimumScaleFactor(0.6)
                    .frame(width: 30, height: 30)
                    .background(Color("TextPrimary"), in: RoundedRectangle(cornerRadius: 9))
                Text("Allternit").font(.system(size: 15, weight: .semibold))
                Spacer()
            }
            .foregroundColor(Color("TextPrimary"))
            ScrollView { content }.scrollBounceBehavior(.basedOnSize)
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color("BgPrimary"))
        .task { await refresh() }
    }

    @ViewBuilder
    private var content: some View {
        switch current {
        case .checking:
            ProgressView().frame(maxWidth: .infinity)
        case .signedOut:
            notice("Sign in to Allternit", "Open the Allternit app and sign in. Then come back here.", "Open Allternit", "allternit://inbox")
        case .expired:
            notice("Your session ended", "Open Allternit once so this panel can read your status again.", "Open Allternit", "allternit://inbox")
        case .offline:
            notice("You're offline", "Messages can't reach Allternit right now. Try again when you're back online.", nil, nil)
        case .quota:
            notice("Phone minutes used up", "Your phone minutes are used up this month.", "See usage", "allternit://settings/billing")
        case .ready:
            if let live { IMReadyLive(live: live, ready: ready, open: open) } else { ready }
        }
    }

    private var ready: some View {
        VStack(alignment: .leading, spacing: 10) {
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                ForEach(IMAction.allCases) { action in
                    Button { tap(action) } label: {
                        VStack(spacing: 6) {
                            Image(systemName: action.icon).font(.system(size: 17))
                            Text(action.rawValue).font(.system(size: 11, weight: .medium)).lineLimit(2)
                                .multilineTextAlignment(.center)
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 64)
                        .background(picked == action ? Color("BorderSubtle") : Color("FillNeutral"),
                                    in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    }
                    .buttonStyle(.plain)
                }
            }
            if picked == .phone { phoneComposer }
            if live?.thread == nil, let run = fixedRun ?? run, picked == .phone || picked == .task || fixedRun != nil {
                IMPhoneRunCard(run: run, open: open)
            }
        }
    }

    private var phoneComposer: some View {
        HStack(spacing: 8) {
            TextField("What should your bot do on your phone?", text: $phoneGoal)
                .font(.system(size: 14))
                .padding(10)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 10))
            Button("Run") { startPhone() }
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(Color("BgPrimary"))
                .padding(.horizontal, 14).frame(minHeight: 40)
                .background(Color("TextPrimary"), in: RoundedRectangle(cornerRadius: 10))
                .disabled(phoneGoal.trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    private func notice(_ title: String, _ body: String, _ action: String?, _ link: String?) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(body).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            if let action, let link, let url = URL(string: link) {
                Button(action) { open(url) }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("BgPrimary"))
                    .padding(.horizontal, 14).frame(minHeight: 36)
                    .background(Color("TextPrimary"), in: Capsule())
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func tap(_ action: IMAction) {
        picked = action
        switch action {
        case .phone, .task: run = SharedPhoneRun.load()
        default:
            var c = URLComponents(string: "allternit://bot/main/chat")!
            c.queryItems = [URLQueryItem(name: "prompt", value: action.prompt)]
            if let url = c.url { open(url) }
        }
    }

    private func startPhone() {
        let goal = phoneGoal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { return }
        var c = URLComponents(string: "allternit://phone-task")!
        c.queryItems = [URLQueryItem(name: "goal", value: goal), URLQueryItem(name: "source", value: "imessage_ext")]
        SharedPhoneRun(runId: nil, goal: goal, status: "starting", step: 0, phoneName: "your phone", updatedAt: Date()).save()
        run = SharedPhoneRun.load()
        phoneGoal = ""
        // The first card goes out on this tap (decision 9); later updates replace it in place.
        live?.start(goal: goal)
        #if DEBUG
        if IMRunController.fixtureActive { return }
        #endif
        if let url = c.url { open(url) }
    }

    private func refresh() async {
        guard fixedState == nil else { return }
        run = SharedPhoneRun.load()
        #if DEBUG
        if IMRunController.fixtureActive { state = .ready; return }
        #endif
        switch await StatusSummaryClient().fetch() {
        case .success: state = run?.problem == "quota" ? .quota : .ready
        case .failure(.signedOut): state = .signedOut
        case .failure(.http(let code)) where code == 401 || code == 403: state = .expired
        case .failure(.http(let code)) where code == 402 || code == 429: state = .quota
        case .failure: state = AppGroupStore.signedIn ? .offline : .signedOut
        }
    }
}

/// The ready panel with the live run on top: a tapped card shows its run (or, for a recipient,
/// the caption only) in place of the actions.
private struct IMReadyLive<Ready: View>: View {
    @ObservedObject var live: IMRunController
    let ready: Ready
    var open: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if live.selected != nil {
                IMLiveRunSection(live: live, open: open)
                Button("Back to actions") { live.clearSelection() }
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(minHeight: 30)
            } else {
                ready
                IMLiveRunSection(live: live, open: open)
            }
        }
    }
}

/// The status card in Messages: the same states as the in-app card, with no
/// thumbnail (the extension has no session for the image).
struct IMPhoneRunCard: View {
    let run: SharedPhoneRun
    var open: (URL) -> Void = { _ in }

    private var label: (String, Color) {
        switch run.problem ?? run.status {
        case "starting": return ("Opening Allternit", .blue)
        case "running": return ("Running", .blue)
        case "waiting_approval": return ("Needs you", .orange)
        case "done": return ("Done", .green)
        case "denied": return ("Denied", .red)
        case "approval_timeout": return ("Timed out", Color("TextSecondary"))
        case "phone_locked": return ("Phone locked", Color("TextSecondary"))
        case "offline", "noPhone": return ("Phone offline", Color("TextSecondary"))
        case "refused": return ("Not started", .red)
        case "quota": return ("Quota reached", Color("TextSecondary"))
        default: return ("Failed", .red)
        }
    }

    private var line: String {
        if let message = run.message, !message.isEmpty { return message }
        switch run.status {
        case "starting": return "Allternit opens to start this on your phone."
        case "running": return run.step == 0 ? "Starting on \(run.phoneName)…" : "Step \(run.step) on \(run.phoneName)"
        case "waiting_approval": return "Step \(run.step) · Waiting for your OK. Review it in Allternit."
        case "done": return "Finished on \(run.phoneName)."
        case "denied": return "You denied step \(run.step). Nothing was done on your phone."
        case "approval_timeout": return "Nobody answered in 5 minutes, so nothing was done."
        case "phone_locked": return "\(run.phoneName) was locked. Unlock it, then run again."
        default: return "The task stopped at step \(run.step)."
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            RoundedRectangle(cornerRadius: 8).fill(Color("FillNeutral"))
                .frame(width: 40, height: 72)
                .overlay(Image(systemName: "iphone.gen3").foregroundColor(Color("TextTertiary")))
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(run.phoneName).font(.system(size: 11, weight: .semibold)).foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Text(label.0).font(.system(size: 11, weight: .semibold)).foregroundColor(label.1)
                        .padding(.horizontal, 8).padding(.vertical, 2)
                        .background(label.1.opacity(0.12), in: Capsule())
                }
                Text(run.goal).font(.system(size: 13, weight: .medium)).foregroundColor(Color("TextPrimary")).lineLimit(2)
                Text(line).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                if run.status == "waiting_approval", let url = URL(string: "allternit://inbox") {
                    Button("Review") { open(url) }
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .padding(.horizontal, 12).frame(minHeight: 30)
                        .background(Color("TextPrimary"), in: Capsule())
                }
            }
        }
        .padding(10)
        .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle")))
    }
}
