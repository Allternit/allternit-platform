import SwiftUI
import UIKit

// "Allternit Cloud models need credits" on the phone
// (CONTRACT-cloud-credits-2026-11-15). Opened from a locked row in the model
// picker, from a send on a locked model (D9) and from a 402 refusal.
// Where the App Store sells credit packs (server `store.apple.enabled` and
// StoreKit products, any storefront) the packs are In-App Purchase buttons
// priced by the App Store. On the US storefront the sheet also links out to
// the web to buy (StorefrontGate). With neither, it shows the title, one
// line and OK, with no price and no link (D12).

enum CloudCreditsCopy {
    static let titleRequired = "Allternit Cloud models need credits"
    static let titleExhausted = "Your Cloud credits are used up"
    static let body = "Free accounts don't include Cloud usage. $1 buys $1 of credit. Plans and API keys you connect still work without credits."
    /// With App Store packs: store prices are higher than the credits, so no "$1 buys $1".
    static let bodyStore = "Free accounts don't include Cloud usage. Credits work on all your devices. Plans and API keys you connect still work without credits."
    static let buyWeb = "Buy on allternit.com"
    /// The one line shown where purchase links aren't allowed: no price, no link.
    static let lineRequired = "Free accounts don't include Cloud usage."
    static let lineExhausted = "Plans and API keys you connect still work without credits."
    static let packs = ["$10", "$25", "$50", "$100"]
    static let buy = "Buy credits"
    static let plan = "Choose a plan"
    static let planNote = "Plus includes monthly credits."
    static let notNow = "Not now"
    static let ok = "OK"

    static func title(_ reason: String?) -> String {
        if OpenModelsCopy.isOpenReason(reason) { return "Open models need credits" }
        return reason == "credits_exhausted" ? titleExhausted : titleRequired
    }

    static func line(_ reason: String?) -> String {
        if let open = OpenModelsCopy.message(forReason: reason) { return open }
        return reason == "credits_exhausted" ? lineExhausted : lineRequired
    }

    /// "GLM 5.3: $0.90 per 1M input, $3.30 per 1M output"; nil without both prices.
    static func priceLine(model: String?, input: Double?, output: Double?) -> String? {
        guard let model, let input, let output else { return nil }
        return "\(model): \(money(input)) per 1M input, \(money(output)) per 1M output"
    }

    static func money(_ value: Double) -> String { String(format: "$%.2f", value) }
}

/// One showing of the sheet: why, and which model the person picked.
struct CloudCreditsRequest: Identifiable, Equatable {
    let id = UUID()
    /// `credits_required` or `credits_exhausted`.
    let reason: String?
    let modelId: String?
    let modelName: String?
    let inputPer1mUsd: Double?
    let outputPer1mUsd: Double?

    init(reason: String?, modelId: String? = nil, modelName: String? = nil,
         inputPer1mUsd: Double? = nil, outputPer1mUsd: Double? = nil) {
        self.reason = reason
        self.modelId = modelId
        self.modelName = modelName
        self.inputPer1mUsd = inputPer1mUsd
        self.outputPer1mUsd = outputPer1mUsd
    }
}

/// Builds requests and remembers the locked model the person tried, so it is
/// selected once the entitlement turns allowed (contract client rules).
@MainActor
enum CloudCreditsGate {
    /// The locked model the person tried last; picked when credits arrive.
    private(set) static var pendingModelId: String? = nil

    static func request(modelId: String?, reason: String?) -> CloudCreditsRequest {
        let store = ModelStore.shared
        let row = store.cloudRow(for: modelId)
        pendingModelId = modelId
        return CloudCreditsRequest(
            reason: reason ?? AccountPlanStore.shared.cloudModels?.reason,
            modelId: modelId,
            modelName: row?.name ?? store.chatModels.first { $0.id == modelId }?.name,
            inputPer1mUsd: row?.inputPer1mUsd,
            outputPer1mUsd: row?.outputPer1mUsd
        )
    }

    /// Called after every plan-store load: once the tried model is no longer
    /// locked, select it.
    static func entitlementChanged(_ entitlement: CloudModelsEntitlement?) {
        guard let pending = pendingModelId, entitlement != nil, !ModelStore.shared.isCloudLocked(pending) else { return }
        pendingModelId = nil
        ModelStore.shared.userSelectedModel(pending)
    }

    static func clearPending() { pendingModelId = nil }

    /// The relayed (computer) path turns the server's 402 into prose and
    /// loses `error.code` (contract pre-check e). Recognize it from the text.
    nonisolated static func reason(inRelayedError text: String?) -> String? {
        guard let text = text?.lowercased(), !text.isEmpty else { return nil }
        if text.contains("credits_exhausted") || text.contains("credits are used up") { return "credits_exhausted" }
        if text.contains("credits_required") || text.contains("need credits") || text.contains("needs credits")
            || text.contains("payment required") || text.range(of: #"\b402\b"#, options: .regularExpression) != nil {
            return "credits_required"
        }
        return nil
    }
}

struct CloudCreditsSheet: View {
    let request: CloudCreditsRequest
    /// Test and gallery seam: forces the storefront answer. nil = StorefrontGate.
    var allowsPurchaseLinksOverride: Bool? = nil

    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var store = StorePurchaseManager.shared
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var showStorePlans = false

    private var allowsLinks: Bool { allowsPurchaseLinksOverride ?? storefront.allowsPurchaseLinks }
    /// App Store packs can be bought here.
    private var storePacks: Bool { storefront.showsStoreButtons(for: .pack) }
    private var canBuy: Bool { allowsLinks || storePacks }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Image(systemName: "lock.fill")
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(SetupPalette.ink)
                .frame(width: 40, height: 40)
                .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 12))
                .accessibilityHidden(true)
            Text(CloudCreditsCopy.title(request.reason))
                .font(.title3.weight(.semibold))
                .foregroundStyle(SetupPalette.ink)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            if canBuy {
                purchaseContent
            } else {
                Text(CloudCreditsCopy.line(request.reason))
                    .font(.subheadline)
                    .foregroundStyle(SetupPalette.sub)
                    .fixedSize(horizontal: false, vertical: true)
                Button(CloudCreditsCopy.ok) { dismiss() }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .padding(.top, 4)
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 24)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .presentationBackground(SetupPalette.bg)
        .presentationDetents(canBuy ? [.medium, .large] : [.height(250)])
        .presentationDragIndicator(.visible)
        .onAppear { if !allowsLinks { storefront.refresh() } }
        .task { await store.loadCatalog() }
        .sheet(isPresented: $showStorePlans) { StorePurchaseSheet(focus: .plan) }
    }

    /// Plans through the App Store: on, and no plan billed elsewhere.
    private var storePlans: Bool {
        storefront.showsStoreButtons(for: .plan) && StorePurchaseManager.planPurchaseAllowed(AccountPlanStore.shared.subscription)
    }

    @ViewBuilder
    private var purchaseContent: some View {
        Text(OpenModelsCopy.isOpenReason(request.reason) ? OpenModelsCopy.message(forReason: request.reason) ?? "" : storePacks ? CloudCreditsCopy.bodyStore : CloudCreditsCopy.body)
            .font(.subheadline)
            .foregroundStyle(SetupPalette.sub)
            .fixedSize(horizontal: false, vertical: true)
        if let price = CloudCreditsCopy.priceLine(model: request.modelName, input: request.inputPer1mUsd, output: request.outputPer1mUsd) {
            Text(price)
                .font(.footnote.weight(.semibold))
                .foregroundStyle(SetupPalette.ink)
                .fixedSize(horizontal: false, vertical: true)
        }
        if storePacks {
            StorePackGrid(store: store)
            StorePurchaseNoticeView(store: store)
        } else {
            HStack(spacing: 8) {
                ForEach(CloudCreditsCopy.packs, id: \.self) { pack in
                    Text(pack)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(SetupPalette.ink)
                        .frame(maxWidth: .infinity, minHeight: 40)
                        .background(SetupPalette.fill, in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(SetupPalette.line, lineWidth: 1))
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Credit packs: \(CloudCreditsCopy.packs.joined(separator: ", "))")
        }
        VStack(spacing: 8) {
            if allowsLinks && storePacks {
                // The App Store packs lead; the web link is the second way.
                Button { openURL(StorefrontGate.creditsURL) } label: {
                    Label(CloudCreditsCopy.buyWeb, systemImage: "arrow.up.right")
                        .labelStyle(TrailingIconLabelStyle())
                }
                .buttonStyle(OutlineButtonStyle())
                .accessibilityHint("Opens billing in your browser")
            } else if allowsLinks {
                Button { openURL(StorefrontGate.creditsURL) } label: {
                    Label(CloudCreditsCopy.buy, systemImage: "arrow.up.right")
                        .labelStyle(TrailingIconLabelStyle())
                }
                .buttonStyle(SetupPrimaryButtonStyle())
                .accessibilityHint("Opens billing in your browser")
            }
            if storePlans {
                Button(CloudCreditsCopy.plan) { showStorePlans = true }
                    .buttonStyle(OutlineButtonStyle())
                    .accessibilityHint("Shows the plans")
            } else if allowsLinks {
                Button { openURL(AccountPlanStore.shared.upgradeURL) } label: {
                    Label(CloudCreditsCopy.plan, systemImage: "arrow.up.right")
                        .labelStyle(TrailingIconLabelStyle())
                }
                .buttonStyle(OutlineButtonStyle())
                .accessibilityHint("Opens the plans in your browser")
            }
            if storePlans || allowsLinks {
                Text(CloudCreditsCopy.planNote)
                    .font(.caption)
                    .foregroundStyle(SetupPalette.mut)
            }
            if storePacks {
                StoreRestoreButton(store: store)
            }
            Button(CloudCreditsCopy.notNow) {
                CloudCreditsGate.clearPending()
                dismiss()
            }
            .buttonStyle(SetupGhostButtonStyle())
        }
        .padding(.top, 2)
    }
}

/// Full-width secondary button: white with a hairline border.
private struct OutlineButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13.5, weight: .semibold))
            .foregroundStyle(SetupPalette.ink)
            .frame(maxWidth: .infinity, minHeight: 44)
            .background(configuration.isPressed ? SetupPalette.fill : SetupPalette.bg, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(SetupPalette.line, lineWidth: 1))
            .contentShape(Rectangle())
    }
}

/// Title then a small trailing icon (the link-out arrow).
private struct TrailingIconLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.title
            configuration.icon.font(.caption.weight(.semibold))
        }
    }
}

#if DEBUG
/// Debug only: shows the credits sheet or the locked picker with fixtures for
/// screenshots (`-cloud-credits-gallery us|intl|us-exhausted|picker`).
@MainActor enum CloudCreditsGallery {
    private static var window: UIWindow?

    static func installIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        guard let i = args.firstIndex(of: "-cloud-credits-gallery"), i + 1 < args.count else { return }
        let page = args[i + 1]
        let locked = CloudModelsEntitlement(allowed: false, reason: page == "us-exhausted" ? "credits_exhausted" : "credits_required",
                                            balanceUsd: 0, byokProviders: ["deepseek"])
        AccountPlanStore.shared.setFixture(credits: BillingCredits(balanceUsd: 0, monthToDateUsageUsd: 0, cloudModels: locked))
        ModelStore.shared.setFixtures(models: [], cloudCatalog: [
            SetupModelRow(id: "glm-5.3", name: "GLM 5.3", provider: "deepinfra", isDefault: true, lane: "standard", inputPer1mUsd: 0.9, outputPer1mUsd: 3.3),
            SetupModelRow(id: "kimi-k2.7", name: "Kimi K2.7", provider: "deepinfra", lane: "standard", inputPer1mUsd: 0.6, outputPer1mUsd: 2.5),
            SetupModelRow(id: "deepseek-v4", name: "DeepSeek V4", provider: "deepseek", lane: "standard", inputPer1mUsd: 0.3, outputPer1mUsd: 1.1),
            SetupModelRow(id: "open-llama", name: "Open lane model", provider: "open", lane: "open"),
        ])
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            guard let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first else { return }
            let w = UIWindow(windowScene: scene)
            w.windowLevel = .alert + 1
            w.rootViewController = UIHostingController(rootView: GalleryHost(page: page))
            w.makeKeyAndVisible()
            window = w
        }
    }

    private struct GalleryHost: View {
        let page: String
        @State private var shown = true

        var body: some View {
            Color("BgSecondary")
                .ignoresSafeArea()
                .overlay(alignment: .top) {
                    Text("Fixture gallery · \(page)").font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary")).padding(.top, 60)
                }
                .sheet(isPresented: $shown) {
                    if page == "picker" {
                        ModelPickerSheet(modelStore: .shared)
                    } else {
                        CloudCreditsSheet(
                            request: CloudCreditsGate.request(modelId: "allternit/glm-5.3", reason: nil),
                            allowsPurchaseLinksOverride: page != "intl"
                        )
                    }
                }
        }
    }
}
#endif
