import StoreKit
import SwiftUI

// In-App Purchase pieces (StoreKit 2, StorePurchaseManager): plan tickets
// and pack buttons priced by the App Store (D11), the notice line with its
// actions, Restore Purchases, the subscription terms, the "Billed by" line,
// and StorePurchaseSheet, which every purchase entry point opens (Settings ›
// Billing shows the same pieces inline). Buttons show only when the server
// has Apple switched on and StoreKit returned the products (D10). The US
// storefront also keeps the web link.

enum StoreViewCopy {
    static let sheetTitlePlans = "Choose a plan"
    static let sheetTitlePacks = "Buy credits"
    static let sheetBody = "Plans and credits work on all your devices. Credits are added to the account you're signed in to."
    static let packsHeader = "Credit packs"
    static let restore = "Restore Purchases"
    static let restoring = "Restoring…"
    static let manageApple = "Manage in App Store"
    static let loading = "Loading prices…"
    static let tryAgain = "Try again"
    static let signIn = "Sign in"
    static let subscribe = "Subscribe"
    static let switchPlan = "Switch"
    static let buying = "Buying…"
    static let webLink = "Buy on allternit.com"
    static let webNote = "Opens allternit.com in your browser."
    static let conflict = "You have two plans. Cancel one."
    static let done = "Done"

    static let termsTail = "Payment is charged to your Apple Account at confirmation of purchase. A subscription renews automatically each month unless you turn off auto-renew at least 24 hours before the end of the current period. Your account is charged for renewal within 24 hours before the period ends, at the same price. Manage or cancel a subscription in Settings > your name > Subscriptions on your iPhone. Cancelling stops the next renewal; you keep the plan until the period ends. Credit packs are one-time purchases. Refunds are handled by Apple at reportaproblem.apple.com."
    static let termsURL = URL(string: "https://allternit.com/terms")!
    static let privacyURL = URL(string: "https://allternit.com/privacy")!

    /// "Allternit Plus is $24.99 per month, Allternit Max is $124.99 per month."
    /// from the App Store's own names and local prices.
    static func pricesSentence(_ pairs: [(name: String, price: String)]) -> String? {
        guard !pairs.isEmpty else { return nil }
        let parts = pairs.map { "\($0.name) is \($0.price) per month" }
        let joined: String
        switch parts.count {
        case 1: joined = parts[0]
        case 2: joined = "\(parts[0]) and \(parts[1])"
        default: joined = parts.dropLast().joined(separator: ", ") + ", and " + parts.last!
        }
        return joined + "."
    }

    static func packCredits(_ item: StoreCatalogItem) -> String {
        "\(item.label) credits"
    }
}

/// The notice after a purchase or restore, with the action it needs.
/// Status is never color alone: each notice has its own icon and text.
struct StorePurchaseNoticeView: View {
    @ObservedObject var store: StorePurchaseManager = .shared
    @Environment(\.openURL) private var openURL
    @State private var showManage = false

    var body: some View {
        if let notice = store.notice {
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: icon(notice))
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(notice.isError ? SetupPalette.warnFg : SetupPalette.ink)
                        .accessibilityHidden(true)
                    Text(StorePurchaseCopy.text(notice))
                        .font(.footnote)
                        .foregroundStyle(SetupPalette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                actions(notice)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(SetupPalette.line, lineWidth: 1))
            .accessibilityElement(children: .contain)
            .manageSubscriptionsSheet(isPresented: $showManage)
        }
    }

    private func icon(_ notice: StorePurchaseNotice) -> String {
        switch notice {
        case .purchased, .restored: return "checkmark.circle"
        case .pending: return "hourglass"
        case .cancelled, .nothingToRestore: return "info.circle"
        case .sandboxNotGranted: return "testtube.2"
        case .signedOut: return "person.crop.circle"
        default: return "exclamationmark.triangle"
        }
    }

    @ViewBuilder
    private func actions(_ notice: StorePurchaseNotice) -> some View {
        switch notice {
        case .failed(_, let canRetry) where canRetry && store.canRetry:
            Button(StoreViewCopy.tryAgain) { Task { await store.retry() } }
                .buttonStyle(SetupInlineButtonStyle())
        case .storeUnavailable:
            Button(StoreViewCopy.tryAgain) { Task { await store.loadCatalog(force: true) } }
                .buttonStyle(SetupInlineButtonStyle())
        case .signedOut:
            Button(StoreViewCopy.signIn) { AuthManager.shared.signIn() }
                .buttonStyle(SetupInlineButtonStyle())
        case .alreadySubscribed(let provider, let url, _):
            if provider == "apple" {
                Button(StoreViewCopy.manageApple) { showManage = true }
                    .buttonStyle(SetupInlineButtonStyle())
            } else if let url {
                Button("Manage on \(StorePurchaseCopy.billedBy(provider))") { openURL(url) }
                    .buttonStyle(SetupInlineButtonStyle())
                    .accessibilityHint("Opens in your browser")
            }
        default:
            EmptyView()
        }
    }
}

/// The plan's "Billed by …" line with its manage action, and the two-plans
/// banner (D5). Nothing on Free.
struct StoreBilledByView: View {
    let subscription: BillingSubscription?
    @ObservedObject private var storefront = StorefrontGate.shared
    @Environment(\.openURL) private var openURL
    @State private var showManage = false

    var body: some View {
        if let subscription, let line = subscription.billedByLine {
            VStack(alignment: .leading, spacing: 6) {
                if subscription.conflict {
                    Label(StoreViewCopy.conflict, systemImage: "exclamationmark.triangle")
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(Theme.statusWarning)
                }
                Text(line)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                if subscription.billingProvider == "apple" {
                    Button(StoreViewCopy.manageApple) { showManage = true }
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(minHeight: 44, alignment: .leading)
                        .buttonStyle(.borderless)
                } else if let raw = subscription.manageUrl, let url = URL(string: raw),
                          subscription.billingProvider != "stripe" || storefront.allowsPurchaseLinks {
                    // A web plan's manage link only where web links are allowed (US).
                    Button("Manage on \(StorePurchaseCopy.billedBy(subscription.billingProvider))") { openURL(url) }
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(minHeight: 44, alignment: .leading)
                        .buttonStyle(.borderless)
                        .accessibilityHint("Opens in your browser")
                }
            }
            .manageSubscriptionsSheet(isPresented: $showManage)
        }
    }
}

/// One store plan as a ticket, priced by the App Store.
struct StorePlanTicket: View {
    let item: StoreCatalogItem
    @ObservedObject var store: StorePurchaseManager = .shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var auth = AuthManager.shared

    var body: some View {
        let current = plan.subscription?.isPaid == true && plan.subscription?.planId == item.id
        PlanTicketView(
            planId: item.id,
            label: item.label,
            price: store.displayPrice(item) ?? "",
            lines: lines,
            current: current,
            actionTitle: actionTitle,
            action: {
                if auth.isSignedIn { Task { await store.purchase(item) } } else { AuthManager.shared.signIn() }
            }
        )
        .disabled(store.busyProductId != nil && store.busyProductId != item.productId)
        .accessibilityHint(auth.isSignedIn ? "Buys with your Apple Account" : "Opens sign in")
    }

    private var lines: [String] {
        var out = [String(format: "$%.0f in Allternit Cloud credits each month", item.creditsUsd)]
        if let cap = item.rolloverCapUsd { out.append(String(format: "Unused credits roll over, up to $%.0f", cap)) }
        return out
    }

    private var actionTitle: String {
        if !auth.isSignedIn { return StorePurchaseCopy.signIn }
        if store.busyProductId == item.productId { return StoreViewCopy.buying }
        return plan.subscription?.isPaid == true ? StoreViewCopy.switchPlan : StoreViewCopy.subscribe
    }
}

/// One store pack as a button: credits on top, the App Store price under.
struct StorePackButton: View {
    let item: StoreCatalogItem
    @ObservedObject var store: StorePurchaseManager = .shared
    @ObservedObject private var auth = AuthManager.shared

    var body: some View {
        let busy = store.busyProductId == item.productId
        let price = store.displayPrice(item) ?? ""
        Button {
            if auth.isSignedIn { Task { await store.purchase(item) } } else { AuthManager.shared.signIn() }
        } label: {
            VStack(spacing: 2) {
                if busy {
                    ProgressView().controlSize(.small)
                } else {
                    Text(item.label)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(SetupPalette.ink)
                }
                Text(price)
                    .font(.system(size: 11.5))
                    .foregroundStyle(SetupPalette.sub)
            }
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(SetupPalette.bg, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(SetupPalette.line, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(store.busyProductId != nil && !busy)
        .accessibilityLabel(busy ? "Buying \(StoreViewCopy.packCredits(item))" : "\(StoreViewCopy.packCredits(item)), \(price)")
        .accessibilityHint(auth.isSignedIn ? "Buys with your Apple Account" : "Opens sign in")
    }
}

/// The four packs in a row (two rows at large text sizes).
struct StorePackGrid: View {
    @ObservedObject var store: StorePurchaseManager = .shared
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: typeSize.isAccessibilitySize ? 2 : 4)
        LazyVGrid(columns: columns, spacing: 8) {
            ForEach(store.packs) { StorePackButton(item: $0, store: store) }
        }
    }
}

/// Restore Purchases (App Review requires it next to the purchase buttons).
struct StoreRestoreButton: View {
    @ObservedObject var store: StorePurchaseManager = .shared

    var body: some View {
        Button {
            Task { await store.restore() }
        } label: {
            HStack(spacing: 6) {
                if store.isRestoring { ProgressView().controlSize(.small) }
                Text(store.isRestoring ? StoreViewCopy.restoring : StoreViewCopy.restore)
            }
        }
        .buttonStyle(SetupGhostButtonStyle())
        .disabled(store.isRestoring)
        .accessibilityHint("Adds purchases made with this Apple Account to your Allternit account")
    }
}

/// The auto-renew disclosure with the App Store's own names and prices, and
/// the Terms and Privacy links (App Store guideline 3.1.2).
struct StoreTermsView: View {
    @ObservedObject var store: StorePurchaseManager = .shared

    private var prices: String? {
        StoreViewCopy.pricesSentence(store.plans.compactMap { item -> (name: String, price: String)? in
            guard let product = store.products[item.productId] else { return nil }
            let name = product.displayName.isEmpty ? "Allternit \(item.label)" : product.displayName
            return (name: name, price: product.displayPrice)
        })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text([prices, StoreViewCopy.termsTail].compactMap { $0 }.joined(separator: " "))
                .font(.caption2)
                .foregroundStyle(SetupPalette.mut)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 16) {
                Link("Terms of Use", destination: StoreViewCopy.termsURL)
                Link("Privacy Policy", destination: StoreViewCopy.privacyURL)
            }
            .font(.caption.weight(.semibold))
            .foregroundStyle(SetupPalette.ink)
            // Two links in one List row: each takes only its own taps.
            .buttonStyle(.borderless)
        }
    }
}

/// The purchase sheet every entry point opens: plans, packs, notice,
/// Restore Purchases, terms, and on the US storefront the web link too.
struct StorePurchaseSheet: View {
    /// Which part leads: plans (usage wall, Meetings) or packs (credits).
    var focus: StoreKind = .plan

    @ObservedObject private var store = StorePurchaseManager.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var auth = AuthManager.shared
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    private var showsPlans: Bool { StorePurchaseManager.planPurchaseAllowed(plan.subscription) && !store.plans.isEmpty }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text(StoreViewCopy.sheetBody)
                        .font(.subheadline)
                        .foregroundStyle(SetupPalette.sub)
                        .fixedSize(horizontal: false, vertical: true)
                    if !auth.isSignedIn {
                        Text(StorePurchaseCopy.signedOut)
                            .font(.footnote)
                            .foregroundStyle(SetupPalette.ink)
                            .fixedSize(horizontal: false, vertical: true)
                        Button(StorePurchaseCopy.signIn) { AuthManager.shared.signIn() }
                            .buttonStyle(SetupPrimaryButtonStyle())
                    }
                    StorePurchaseNoticeView(store: store)
                    content
                    if storefront.allowsPurchaseLinks {
                        VStack(alignment: .leading, spacing: 4) {
                            Button {
                                openURL(focus == .pack ? StorefrontGate.creditsURL : plan.upgradeURL)
                            } label: {
                                Label(StoreViewCopy.webLink, systemImage: "arrow.up.right")
                            }
                            .font(.subheadline.weight(.semibold))
                            .foregroundStyle(SetupPalette.ink)
                            .frame(minHeight: 44)
                            .accessibilityHint(StoreViewCopy.webNote)
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
            }
            .background(SetupPalette.bg)
            .navigationTitle(focus == .pack ? StoreViewCopy.sheetTitlePacks : StoreViewCopy.sheetTitlePlans)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(StoreViewCopy.done) { dismiss() }
                }
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .task {
            store.notice = nil
            await store.loadCatalog()
            if auth.isSignedIn { AccountPlanStore.shared.load() }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch store.catalogState {
        case .idle, .loading:
            HStack(spacing: 8) {
                ProgressView()
                Text(StoreViewCopy.loading).font(.subheadline).foregroundStyle(SetupPalette.sub)
            }
            .accessibilityElement(children: .combine)
        case .unavailable, .notConfigured:
            if store.notice == nil {
                Text(StorePurchaseCopy.storeUnavailable)
                    .font(.subheadline)
                    .foregroundStyle(SetupPalette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                Button(StoreViewCopy.tryAgain) { Task { await store.loadCatalog(force: true) } }
                    .buttonStyle(SetupInlineButtonStyle())
            }
        case .ready:
            if focus == .pack {
                packs
                plans
            } else {
                plans
                packs
            }
            StoreBilledByView(subscription: plan.subscription)
            HStack {
                StoreRestoreButton(store: store)
                Spacer()
            }
            StoreTermsView(store: store)
        }
    }

    @ViewBuilder
    private var plans: some View {
        if showsPlans {
            VStack(spacing: 10) {
                ForEach(store.plans) { StorePlanTicket(item: $0, store: store) }
            }
        }
    }

    @ViewBuilder
    private var packs: some View {
        if !store.packs.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text(StoreViewCopy.packsHeader)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(SetupPalette.sub)
                    .accessibilityAddTraits(.isHeader)
                StorePackGrid(store: store)
            }
        }
    }
}
