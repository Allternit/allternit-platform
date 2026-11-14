import SwiftUI
import UIKit

// The compute gate on the phone (contract: CONTRACT-compute-gate-2026-11-13.md).
// "Agents need a computer": three paths (own Desktop, subscribe, credits) and
// a chat-only confirmation. Copy strings are the contract's, verbatim.
// Plans and credits open the web only where StorefrontGate allows purchase
// links (US storefront); elsewhere card A plus "Manage your plan on the web."

enum ComputeGateCopy {
    static let title = "Agents need a computer"
    static let body = "Pick where your agents run. Chat with Allternit Cloud models stays open."
    static let cardATitle = "Use your own computer"
    static let cardACost = "Free. Link the Allternit Desktop app."
    static let cardAButton = "Link my computer"
    static let cardBTitle = "Subscribe"
    static let cardBButton = "See plans"
    static let cardCTitle = "Buy credits"
    static let cardCButton = "Buy credits"
    static let chatOnlyLink = "Just chat for now"
    static let chatOnlyConfirm = "Chat works without a computer. Agents will not run until you add one."
    static let chatOnlyContinue = "Continue with chat"
    static let chatOnlyBack = "Pick a computer"
    static let waiting = "Waiting for the Desktop app. Open it to finish linking."
    static let provisioning = "Starting your computer. This takes about a minute."
    static let ready = "Your computer is ready."
    static let sleeping = "Your computer is asleep. It wakes when an agent needs it."
    static let outOfCredits = "Your credits ran out and the computer stopped. Add credits to start it again."
    static let lowRunway24 = "About a day of computer time left on your credits."
    static let lowRunway2 = "About 2 hours of computer time left on your credits."
    static let freeRetired = "Free hosted computers are no longer offered. Pick one of the options above."
    static let honesty = "Hosted computers run on Allternit servers. They sleep when idle."
    static let manageOnWeb = "Manage your plan on the web."
    static let checkFailed = "Couldn't check your computer."

    static func money(_ v: Double) -> String { String(format: "$%.2f", v) }
    static func whole(_ v: Double) -> String { v == v.rounded() ? String(format: "$%.0f", v) : money(v) }

    /// "Plus $20, Max $100 or Ultra $200 a month. Includes one hosted computer." (prices from the server)
    static func cardBCost(_ plans: [ComputeStatus.PlanOption]) -> String {
        let parts = plans.map { "\($0.name) \(whole($0.priceUsdMonth))" }
        let list: String
        switch parts.count {
        case 0: return "Includes one hosted computer."
        case 1: list = parts[0]
        default: list = parts.dropLast().joined(separator: ", ") + " or " + parts[parts.count - 1]
        }
        return "\(list) a month. Includes one hosted computer."
    }

    static func cardCCost(_ price: Double) -> String {
        "A hosted computer billed per minute while it runs. From \(money(price)) an hour."
    }

    static func insufficient(_ min: Double) -> String { "Starting needs at least \(whole(min)) in credits." }
}

/// One plain line about the plan computer's hours this month, or nil.
enum ComputeHoursLine {
    static func text(_ s: ComputeStatus) -> String? {
        guard let h = s.computer?.hours, h.includedHours > 0 else { return nil }
        if h.stoppedForHours == true {
            var when = " or wait for next month"
            if let raw = h.resumesAt, let date = parse(raw) {
                let f = DateFormatter()
                f.locale = Locale(identifier: "en_US_POSIX")
                f.timeZone = TimeZone(identifier: "UTC")
                f.dateFormat = "MMM d"
                when = " or it starts again on \(f.string(from: date))"
            }
            return "Stopped: no credits left. Add credits\(when)."
        }
        if h.usedHours >= Double(h.includedHours) {
            var price = ""
            if let p = h.overagePricePerHourUsd { price = " at " + ComputeGateCopy.money(p) + "/hour" }
            return "\(h.includedHours) hours used. Extra hours bill from credits\(price)."
        }
        return "\(Int(h.usedHours.rounded(.down))) of \(h.includedHours) hours used this month"
    }

    private static func parse(_ raw: String) -> Date? {
        let a = ISO8601DateFormatter()
        if let d = a.date(from: raw) { return d }
        a.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return a.date(from: raw)
    }
}

/// A one-line status for the account's computer, or nil when there is nothing to say.
enum ComputeGateNotice {
    static func text(_ s: ComputeStatus) -> String? {
        if s.isSuspended { return ComputeHoursLine.text(s) ?? ComputeGateCopy.outOfCredits }
        guard let c = s.computer else { return nil }
        if s.state == "provisioning" || c.status == "starting" { return ComputeGateCopy.provisioning }
        if c.status == "sleeping" { return ComputeGateCopy.sleeping }
        if c.kind == "credits", c.status == "running", let credits = s.paths?.credits, credits.pricePerHourUsd > 0 {
            let hours = credits.balanceUsd / credits.pricePerHourUsd
            if hours <= 2 { return ComputeGateCopy.lowRunway2 }
            if hours <= 24 { return ComputeGateCopy.lowRunway24 }
        }
        return nil
    }
}

struct ComputeGateView: View {
    /// Called when the gate is done (computer ready, or chat-only confirmed).
    var onDone: (() -> Void)? = nil

    @ObservedObject private var store = ComputeStatusStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared

    @State private var confirmingChatOnly = false
    @State private var working = false
    @State private var linking = false
    @State private var copied = false
    @State private var startError: String?
    @State private var waitingRegistered = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if store.loadFailed && store.status == nil {
                failed
            } else if let s = store.status {
                content(s)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 44)
                    .accessibilityLabel("Checking your computer")
            }
        }
        .task { await store.refreshIfNeeded() }
        .onDisappear { stopWaiting() }
        .onChange(of: store.status) { _, s in
            if let s, !s.gate, s.computer != nil, s.state == "paired" || s.state == "hosted" { stopWaiting() }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(ComputeGateCopy.title)
                .font(.title3.weight(.semibold))
                .foregroundStyle(SetupPalette.ink)
                .accessibilityAddTraits(.isHeader)
            Text(ComputeGateCopy.body)
                .font(.subheadline)
                .foregroundStyle(SetupPalette.sub)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var failed: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(ComputeGateCopy.checkFailed)
                .font(.subheadline)
                .foregroundStyle(SetupPalette.ink)
            Button("Retry") { Task { await store.refresh() } }
                .buttonStyle(SetupInlineButtonStyle())
                .accessibilityHint("Checks your computer again")
        }
    }

    @ViewBuilder
    private func content(_ s: ComputeStatus) -> some View {
        if !s.gate, s.computer != nil, s.state != "none" {
            // Gate lifted: say where things stand.
            VStack(alignment: .leading, spacing: 8) {
                Text(ComputeGateNotice.text(s) ?? ComputeGateCopy.ready)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(SetupPalette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                if !s.isSuspended, let hours = ComputeHoursLine.text(s) {
                    Text(hours)
                        .font(.subheadline)
                        .foregroundStyle(SetupPalette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let onDone {
                    Button("Done", action: onDone).buttonStyle(SetupPrimaryButtonStyle())
                }
            }
        } else if confirmingChatOnly {
            chatOnlyConfirm
        } else {
            if let notice = ComputeGateNotice.text(s) {
                Text(notice).font(.subheadline.weight(.semibold)).foregroundStyle(SetupPalette.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }
            cardA(s)
            if storefront.allowsPurchaseLinks {
                if let plans = s.paths?.plan?.plans, !plans.isEmpty { cardB(plans) }
                if let credits = s.paths?.credits { cardC(credits) }
            } else {
                Text(ComputeGateCopy.manageOnWeb)
                    .font(.footnote)
                    .foregroundStyle(SetupPalette.sub)
            }
            Button(ComputeGateCopy.chatOnlyLink) { confirmingChatOnly = true }
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(SetupPalette.ink)
                .frame(minHeight: 44)
            Text(ComputeGateCopy.honesty)
                .font(.footnote)
                .foregroundStyle(SetupPalette.mut)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: - Cards

    private func card<C: View>(_ title: String, cost: String, @ViewBuilder _ actions: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline).foregroundStyle(SetupPalette.ink)
            Text(cost).font(.subheadline).foregroundStyle(SetupPalette.sub)
                .fixedSize(horizontal: false, vertical: true)
            actions()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line))
        .accessibilityElement(children: .contain)
    }

    private func cardA(_ s: ComputeStatus) -> some View {
        let link = s.paths?.own?.downloadUrl ?? "https://www.allternit.com/download"
        let shown = link.replacingOccurrences(of: "https://www.", with: "").replacingOccurrences(of: "https://", with: "")
        return card(ComputeGateCopy.cardATitle, cost: ComputeGateCopy.cardACost) {
            if linking {
                VStack(alignment: .leading, spacing: 8) {
                    Text("1. On your computer, open \(shown)")
                        .font(.subheadline).foregroundStyle(SetupPalette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        Button(copied ? "Copied" : "Copy link") {
                            UIPasteboard.general.string = link
                            copied = true
                        }
                        .buttonStyle(SetupInlineButtonStyle())
                        if let url = URL(string: link) {
                            ShareLink(item: url) { Text("Share") }
                                .buttonStyle(SetupInlineButtonStyle())
                        }
                    }
                    Text("2. Open Allternit and approve the link it opens in your browser.")
                        .font(.subheadline).foregroundStyle(SetupPalette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("3. Waiting for your computer…")
                            .font(.subheadline).foregroundStyle(SetupPalette.sub)
                    }
                    .accessibilityElement(children: .combine)
                }
            } else {
                Button(ComputeGateCopy.cardAButton) { linking = true; startWaiting() }
                    .buttonStyle(SetupPrimaryButtonStyle())
            }
        }
    }

    private func cardB(_ plans: [ComputeStatus.PlanOption]) -> some View {
        card(ComputeGateCopy.cardBTitle, cost: ComputeGateCopy.cardBCost(plans)) {
            Button(ComputeGateCopy.cardBButton) {
                UIApplication.shared.open(AccountPlanStore.shared.upgradeURL)
            }
            .buttonStyle(SetupPrimaryButtonStyle())
            .accessibilityHint("Opens the plans in your browser")
        }
    }

    private func cardC(_ credits: ComputeStatus.Credits) -> some View {
        let canStart = credits.balanceUsd >= credits.minStartUsd
        let size = credits.sizes?.first?.id ?? "medium"
        return card(ComputeGateCopy.cardCTitle, cost: ComputeGateCopy.cardCCost(credits.pricePerHourUsd)) {
            Text("Balance \(ComputeGateCopy.money(credits.balanceUsd)) · \(ComputeGateCopy.money(credits.pricePerHourUsd)) an hour")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(SetupPalette.ink)
            if canStart {
                Button(working ? "Starting …" : "Start computer") { Task { await start(size: size) } }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .disabled(working)
            } else {
                Text(ComputeGateCopy.insufficient(credits.minStartUsd))
                    .font(.footnote).foregroundStyle(SetupPalette.sub)
                Button(ComputeGateCopy.cardCButton) { UIApplication.shared.open(StorefrontGate.creditsURL) }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .accessibilityHint("Opens billing in your browser")
            }
            if let startError {
                Text(startError).font(.footnote).foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var chatOnlyConfirm: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(ComputeGateCopy.chatOnlyConfirm)
                .font(.subheadline)
                .foregroundStyle(SetupPalette.ink)
                .fixedSize(horizontal: false, vertical: true)
            Button(ComputeGateCopy.chatOnlyContinue) {
                Task {
                    working = true
                    defer { working = false }
                    try? await store.acknowledgeChatOnly()
                    onDone?()
                }
            }
            .buttonStyle(SetupPrimaryButtonStyle())
            .disabled(working)
            Button(ComputeGateCopy.chatOnlyBack) { confirmingChatOnly = false }
                .buttonStyle(SetupGhostButtonStyle())
        }
    }

    // MARK: - Actions

    private func start(size: String) async {
        working = true
        startError = nil
        defer { working = false }
        do {
            try await store.startCreditComputer(size: size)
            startWaiting()
        } catch ComputeGateError.insufficientCredits(let min) {
            let value = min ?? store.status?.paths?.credits?.minStartUsd ?? 0
            startError = ComputeGateCopy.insufficient(value)
            await store.refresh()
        } catch ComputeGateError.freeRetired {
            startError = ComputeGateCopy.freeRetired
        } catch {
            startError = "Something went wrong. Try again."
        }
    }

    private func startWaiting() {
        guard !waitingRegistered else { return }
        waitingRegistered = true
        store.beginWaiting()
    }

    private func stopWaiting() {
        guard waitingRegistered else { return }
        waitingRegistered = false
        store.endWaiting()
    }
}

/// The gate as a sheet (409 compute_required, or the strip's action).
struct ComputeGateSheet: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                ComputeGateView(onDone: { dismiss() })
                    .padding(16)
            }
            .background(SetupPalette.bg.ignoresSafeArea())
            .navigationTitle("Computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Close") { dismiss() } }
            }
        }
    }
}

/// The small banner for agent screens: gate on and not chat-only-confirmed
/// shows "Agents need a computer"; chat-only shows the same strip as a
/// reminder; a failed check says so with Retry (agent screens stay closed).
struct ComputeGateStrip: View {
    @ObservedObject private var store = ComputeStatusStore.shared
    @State private var showSheet = false

    var body: some View {
        Group {
            if store.loadFailed && store.status == nil {
                row(text: ComputeGateCopy.checkFailed, action: "Retry") { Task { await store.refresh() } }
            } else if let s = store.status, s.gate {
                row(text: s.isSuspended ? (ComputeHoursLine.text(s) ?? ComputeGateCopy.outOfCredits) : ComputeGateCopy.title,
                    action: "Pick a computer") { showSheet = true }
            } else if let s = store.status, let notice = ComputeGateNotice.text(s) ?? ComputeHoursLine.text(s) {
                row(text: notice, action: nil, perform: {})
            }
        }
        .task { await store.refreshIfNeeded() }
        .sheet(isPresented: $showSheet) { ComputeGateSheet() }
    }

    private func row(text: String, action: String?, perform: @escaping () -> Void) -> some View {
        HStack(spacing: 12) {
            Text(text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            if let action {
                Button(action: perform) {
                    Text(action)
                        .font(.subheadline.weight(.semibold))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundColor(Color("TextPrimary"))
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .accessibilityElement(children: .combine)
    }
}

/// Setup's "Choose how your agents run" step: the gate inline in the Where
/// step while the account has no computer (and has not confirmed chat-only).
struct SetupComputeGatePanel: View {
    @ObservedObject private var store = ComputeStatusStore.shared

    var body: some View {
        Group {
            if store.needsFullGate || (store.loadFailed && store.status == nil) {
                SetupPanel(title: "Choose how your agents run") { ComputeGateView() }
            }
        }
        .task { await store.refreshIfNeeded() }
    }
}
