import SwiftUI

import StoreKit

/// Settings → Billing (Claude polish i3, web frame `M1-pay-billing-phone`):
/// the member card, the plan row, usage bars, and a read-only invoices list
/// (`GET /api/v1/billing/invoices`, cloud-api #3097). An invoice's PDF opens
/// in an in-app Safari view.
///
/// App Store 3.1.1: no card form, no card update and no link out to pay.
/// Plans and credit packs are bought with In-App Purchase where the server
/// has Apple switched on and StoreKit returns the products
/// (StorePurchaseManager), with Restore Purchases and the subscription
/// terms. "Manage subscription" opens Apple's in-app manage sheet
/// (StoreKit `manageSubscriptionsSheet`). A plan billed on the web or by
/// Google shows who bills it, with no link.
///
/// `title: "Usage"` opens Settings → Usage instead (`UsagePageView`, phone
/// calm I15), so every caller keeps its route.
struct ComputeBillingView: View {
    @ObservedObject private var usageStore = UsageStore.shared
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @ObservedObject private var store = StorePurchaseManager.shared
    var title: String = "Billing"
    @State private var showManageSubscription = false
    @State private var invoices: [BillingInvoice] = []
    @State private var invoiceCursor: String?
    @State private var invoiceState: InvoiceLoadState = .loading
    @State private var openPDF: IdentifiableURL?

    /// One invoice from `GET /api/v1/billing/invoices` (snake_case).
    struct BillingInvoice: Decodable, Identifiable, Equatable {
        let id: String
        let createdAt: String
        let totalUsd: Double
        let status: String
        let pdfUrl: String?

        enum CodingKeys: String, CodingKey {
            case id, status
            case createdAt = "created_at"
            case totalUsd = "total_usd"
            case pdfUrl = "pdf_url"
        }

        var date: Date? { parseUsageDate(createdAt) }
        /// The PDF only; never a hosted pay page.
        var pdf: URL? { pdfUrl.flatMap(URL.init(string:)).flatMap { $0.scheme == "https" ? $0 : nil } }
        var statusLabel: String {
            switch status {
            case "paid": return "Paid"
            case "open": return "Open"
            case "void": return "Void"
            case "uncollectible": return "Unpaid"
            case "draft": return "Draft"
            default: return status.capitalized
            }
        }
    }
    private struct InvoicePage: Decodable {
        let invoices: [BillingInvoice]
        let cursor: String?
    }
    enum InvoiceLoadState: Equatable { case loading, loaded, unavailable, failed }

    private var memberName: String {
        let first = AuthManager.shared.currentSession?.user?.firstName?.trimmingCharacters(in: .whitespaces) ?? ""
        return first.isEmpty ? "Member" : first
    }

    var body: some View {
        if title == "Usage" {
            // Settings → Usage (phone calm I15): its own page; the billing
            // list below is Settings → Billing.
            UsagePageView()
        } else {
            billingList
        }
    }

    private var billingList: some View {
        // Dot title that docks into the bar on scroll (UI feedback I12).
        DockingTitleList(title: title) {
            if title != "Usage" {
                Section {
                    MemberCardView(
                        planId: plan.subscription?.planId ?? "free",
                        member: memberName,
                        renewsAt: plan.subscription?.isPaid == true ? plan.subscription?.renewsAt : nil,
                        status: plan.subscription?.status,
                        width: 320
                    )
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
                    .listRowInsets(EdgeInsets(top: 8, leading: 0, bottom: 8, trailing: 0))
                }
            }
            Section {
                HStack {
                    Text("Current plan")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if plan.isLoading && plan.planName == nil {
                        ProgressView()
                    } else {
                        Text(plan.planName ?? usageStore.planLabel ?? "Free")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(Color("BgSecondary"))
                            .clipShape(Capsule())
                    }
                }
                if let status = plan.subscription?.status, plan.subscription?.isPaid == true, status != "active" {
                    Text(Self.statusCopy(status))
                        .font(.caption)
                        .foregroundColor(status == "trialing" ? Color("TextSecondary") : Theme.statusWarning)
                }
                if let subscription = plan.subscription, let line = subscription.billedByLine {
                    // Who bills the plan, as text only: no web link (3.1.1).
                    if subscription.conflict {
                        Label(StoreViewCopy.conflict, systemImage: "exclamationmark.triangle")
                            .font(.footnote.weight(.semibold))
                            .foregroundColor(Theme.statusWarning)
                    }
                    Text(line)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                if plan.subscription?.billingProvider == "apple" && plan.subscription?.isPaid == true {
                    Button {
                        Haptics.light()
                        showManageSubscription = true
                    } label: {
                        HStack {
                            Text("Manage subscription")
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                            Spacer()
                            Image(systemName: "chevron.right")
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextTertiary"))
                                .accessibilityHidden(true)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint("Opens your App Store subscriptions")
                }
            } header: {
                Text("Plan")
            }

            Section {
                PlanUsageRows()
            } header: {
                Text("Usage")
            }

            // Allternit Open: the allowance meter, once the server reports it.
            if title != "Usage", storefront.allowsPurchaseLinks, let open = plan.openModels, open.plan != nil || open.termsAccepted {
                Section { OpenModelsStatusRows(entitlement: open) } header: { Text("Status") }
            }

            // In-App Purchase: plan tickets priced by the App Store, while no
            // plan is billed on the web or by Google (D5).
            if title != "Usage" && storefront.showsStoreButtons(for: .plan)
                && StorePurchaseManager.planPurchaseAllowed(plan.subscription) {
                Section {
                    ForEach(store.plans) { item in
                        StorePlanTicket(item: item, store: store)
                            .listRowBackground(Color.clear)
                            .listRowInsets(EdgeInsets(top: 5, leading: 0, bottom: 5, trailing: 0))
                    }
                } header: {
                    Text(plan.subscription?.isPaid == true ? "Change plan" : "Choose a plan")
                }
            }

            if title != "Usage" && plan.openModels != nil {
                Section { OpenResearchTicketView().listRowBackground(Color.clear).listRowInsets(EdgeInsets(top: 5, leading: 0, bottom: 5, trailing: 0)) }
            }

            if title != "Usage" && storefront.showsStoreButtons(for: .pack) {
                Section {
                    StorePackGrid(store: store)
                        .listRowBackground(Color.clear)
                        .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
                } header: {
                    Text("Buy credits")
                }
            }

            if title != "Usage" && storefront.showsStoreButtons {
                Section {
                    StorePurchaseNoticeView(store: store)
                    StoreRestoreButton(store: store)
                    StoreTermsView(store: store)
                }
                .listRowBackground(Color.clear)
            }

            if AuthManager.shared.isSignedIn && invoiceState != .unavailable {
                Section {
                    invoiceRows
                } header: {
                    Text("Invoices")
                }
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .manageSubscriptionsSheet(isPresented: $showManageSubscription)
        .sheet(item: $openPDF) { item in
            SafariView(url: item.url).ignoresSafeArea()
        }
        .task {
            await loadInvoices(reset: true)
            UsageStore.shared.fetchUsageIfNeeded(force: true)
            AccountPlanStore.shared.load(force: true)
            StorefrontGate.shared.refresh()
            if title != "Usage" {
                StorePurchaseManager.shared.notice = nil
                await StorePurchaseManager.shared.loadCatalog()
            }
        }
        .refreshable {
            UsageStore.shared.fetchUsageIfNeeded(force: true)
            AccountPlanStore.shared.load(force: true)
            await loadInvoices(reset: true)
        }
    }

    // MARK: Invoices (read-only)

    /// Date, total, status; tapping a row with a PDF opens it in-app. No pay
    /// button, also for an open invoice.
    @ViewBuilder
    private var invoiceRows: some View {
        switch invoiceState {
        case .loading where invoices.isEmpty:
            HStack { Text("Nov 4, 2026").font(.subheadline); Spacer(); Text("$00.00").font(.subheadline) }
                .redacted(reason: .placeholder)
                .accessibilityLabel("Loading invoices")
        case .failed where invoices.isEmpty:
            Text("Couldn't load your invoices. Pull down to try again.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
        default:
            if invoices.isEmpty {
                Text("No invoices yet.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(invoices) { invoice in
                    invoiceRow(invoice)
                }
                if invoiceCursor != nil {
                    Button("Show more") { Task { await loadInvoices(reset: false) } }
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .frame(minHeight: 44)
                        .disabled(invoiceState == .loading)
                }
            }
        }
    }

    private func invoiceRow(_ invoice: BillingInvoice) -> some View {
        Button {
            guard let pdf = invoice.pdf else { return }
            Haptics.selection()
            openPDF = IdentifiableURL(url: pdf)
        } label: {
            HStack(spacing: 12) {
                Text(invoice.date.map { Self.invoiceDate.string(from: $0) } ?? "—")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(String(format: "$%.2f", invoice.totalUsd))
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .monospacedDigit()
                Text(invoice.statusLabel)
                    .font(.footnote)
                    .foregroundColor(invoice.status == "paid" ? Color("TextSecondary") : Theme.statusWarning)
                    .frame(minWidth: 52, alignment: .trailing)
                Image(systemName: "doc.text")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .opacity(invoice.pdf == nil ? 0 : 1)
                    .accessibilityHidden(true)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(invoice.pdf == nil)
        .accessibilityElement(children: .combine)
        .accessibilityHint(invoice.pdf == nil ? "" : "Opens the invoice PDF")
    }

    private func loadInvoices(reset: Bool) async {
        guard AuthManager.shared.isSignedIn else { return }
        if !reset && invoiceCursor == nil { return }
        var path = "billing/invoices?limit=12"
        if !reset, let cursor = invoiceCursor,
           let encoded = cursor.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) {
            path += "&cursor=\(encoded)"
        }
        invoiceState = .loading
        do {
            let page: InvoicePage = try await APIClient.shared.get(path: path)
            invoices = reset ? page.invoices : invoices + page.invoices
            invoiceCursor = page.cursor
            invoiceState = .loaded
        } catch APIError.httpError(let code, _) where code == 404 || code == 501 {
            // A server without the invoices route: no section.
            invoiceState = .unavailable
        } catch {
            invoiceState = .failed
        }
    }

    private static let invoiceDate: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        f.timeStyle = .none
        return f
    }()

    private static func statusCopy(_ status: String) -> String {
        switch status {
        case "trialing": return "Free trial"
        case "past_due": return "Payment past due."
        case "unpaid": return "Payment failed. Your plan is paused."
        default: return status.capitalized
        }
    }
}
