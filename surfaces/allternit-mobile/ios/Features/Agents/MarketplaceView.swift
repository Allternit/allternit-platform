import SwiftUI

/// Agent | bot marketplace — browse/search/install/rate shared agents
/// and bots (PalsHub-equivalent). Reached from AgentHubView's "Discover"
/// entry point.
struct MarketplaceView: View {
    @StateObject private var store = MarketplaceStore.shared
    @State private var searchText = ""
    @State private var searchTask: Task<Void, Never>? = nil

    var body: some View {
        DockingTitleList(title: "Marketplace") {
            if store.isLoading && store.listings.isEmpty {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
                    .listRowSeparator(.hidden)
            } else if let error = store.error {
                FriendlyInlineStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load agents",
                    message: FriendlyErrorMessage.from(error),
                    actionTitle: "Retry",
                    action: { store.search(query: searchText.isEmpty ? nil : searchText) }
                )
                .listRowSeparator(.hidden)
            } else if store.listings.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "square.grid.2x2",
                    title: "No published agents or bots yet",
                    message: "Check back later or publish your own from the workspace."
                )
                .listRowSeparator(.hidden)
            } else {
                ForEach(store.listings) { listing in
                    ZStack {
                        MarketplaceListingRow(listing: listing)
                        NavigationLink(destination: MarketplaceListingDetailView(listingId: listing.id)) {
                            EmptyView()
                        }
                        .opacity(0)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("\(listing.title). \(listing.description)")
                    .accessibilityHint("Opens details")
                    .accessibilityAddTraits(.isButton)
                    .listRowInsets(EdgeInsets(top: 5, leading: 14, bottom: 5, trailing: 14))
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                }
            }
        }
        .background(Color("BgPrimary"))
        .searchable(text: $searchText, prompt: "Search agents")
        .onChange(of: searchText) { _, newValue in
            searchTask?.cancel()
            searchTask = Task {
                try? await Task.sleep(for: .milliseconds(300))
                guard !Task.isCancelled else { return }
                store.search(query: newValue.isEmpty ? nil : newValue)
            }
        }
        .task {
            if store.listings.isEmpty { store.search() }
        }
        .refreshable {
            store.search(query: searchText.isEmpty ? nil : searchText)
        }
    }
}

private struct MarketplaceListingRow: View {
    let listing: MarketplaceListing

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(listing.title.isEmpty ? "Untitled agent" : listing.title)
                .font(.subheadline)
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))
            Text(listing.description.isEmpty ? "No description provided." : listing.description)
                .font(.caption)
                .foregroundColor(listing.description.isEmpty ? Color("TextSecondary").opacity(0.7) : Color("TextSecondary"))
                .lineLimit(2)
            HStack(spacing: 10) {
                if let publisherName = listing.publisherName {
                    Label(publisherName, systemImage: "person.circle")
                        .accessibilityLabel("Published by \(publisherName)")
                }
                if listing.ratingCount > 0 {
                    Label(String(format: "%.1f (%d)", listing.ratingAvg, listing.ratingCount), systemImage: "star.fill")
                        .accessibilityLabel("Rating \(String(format: "%.1f", listing.ratingAvg)) out of 5 from \(listing.ratingCount) reviews")
                }
                Label("\(listing.installCount)", systemImage: "arrow.down.circle")
                    .accessibilityLabel("\(listing.installCount) installs")
            }
            .font(.caption2)
            .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(PolishTokens.neutralFill))
    }
}

/// .94 press for the Install and Rate buttons.
private struct MarketplacePressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? PolishTokens.pressPill : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
    }
}

struct MarketplaceListingDetailView: View {
    let listingId: String

    @StateObject private var store = MarketplaceStore.shared
    @State private var detail: MarketplaceListingDetail? = nil
    @State private var isLoading = true
    @State private var loadError: String? = nil
    @State private var isInstalling = false
    @State private var installError: String? = nil
    @State private var installedAgentId: String? = nil
    @State private var isRatingPresented = false

    var body: some View {
        Group {
            if let detail {
                List {
                    Section {
                        Text(detail.listing.description)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        if let publisherName = detail.listing.publisherName {
                            HStack {
                                Text("By")
                                Text(publisherName).foregroundColor(Color("TextSecondary"))
                            }
                            .font(.caption)
                        }
                        if !detail.listing.tags.isEmpty {
                            Text(detail.listing.tags.joined(separator: ", "))
                                .font(.caption2)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }

                    Section {
                        if installedAgentId != nil {
                            Label("Installed", systemImage: "checkmark.circle.fill")
                                .foregroundColor(Theme.statusSuccess)
                        } else {
                            Button(action: install) {
                                if isInstalling {
                                    ProgressView()
                                } else {
                                    Text("Install")
                                }
                            }
                            .buttonStyle(MarketplacePressStyle())
                            .foregroundColor(Color("AccentPrimary"))
                            .disabled(isInstalling)
                        }
                        Button("Rate this agent") { isRatingPresented = true }
                            .buttonStyle(MarketplacePressStyle())
                            .foregroundColor(Color("AccentPrimary"))
                        if let installError {
                            Text(installError)
                                .font(.caption)
                                .foregroundColor(Theme.statusWarning)
                        }
                    }

                    if !detail.ratings.isEmpty {
                        Section {
                            ForEach(detail.ratings) { rating in
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack {
                                        Text(rating.reviewerName ?? "Anonymous")
                                            .font(.caption)
                                            .fontWeight(.medium)
                                        Spacer()
                                        Text(String(repeating: "★", count: rating.rating))
                                            .font(.caption)
                                            .foregroundColor(Theme.statusWarning)
                                    }
                                    if let review = rating.review, !review.isEmpty {
                                        Text(review)
                                            .font(.caption)
                                            .foregroundColor(Color("TextSecondary"))
                                    }
                                }
                            }
                        } header: {
                            SectionLabel("Reviews (\(detail.listing.ratingCount))").textCase(nil)
                        }
                    }
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
                .background(Color("BgPrimary"))
            } else if isLoading {
                ProgressView()
            } else if let loadError {
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't load agent details",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { Task { await load() } }
                )
            }
        }
        .navigationTitle(detail?.listing.title ?? "Agent")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .sheet(isPresented: $isRatingPresented) {
            RateListingSheet(listingId: listingId) {
                await load()
            }
        }
    }

    private func load() async {
        isLoading = true
        loadError = nil
        do {
            detail = try await AgentMarketplaceClient.shared.getListing(id: listingId)
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    private func install() {
        isInstalling = true
        installError = nil
        Task {
            do {
                installedAgentId = try await store.install(listingId: listingId)
                Haptics.success()
            } catch {
                installError = error.localizedDescription
            }
            isInstalling = false
        }
    }
}

private struct RateListingSheet: View {
    let listingId: String
    let onSaved: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var rating = 5
    @State private var review = ""
    @State private var isSaving = false
    @State private var saveError: String? = nil

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack(spacing: 8) {
                        ForEach(1...5, id: \.self) { value in
                            Button(action: { rating = value }) {
                                Image(systemName: value <= rating ? "star.fill" : "star")
                                    .font(.system(size: 28, weight: .semibold))
                                    .foregroundColor(.yellow)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("\(value) stars")
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 8)
                } header: {
                    Text("Rating")
                }
                Section {
                    TextEditor(text: $review)
                        .frame(minHeight: 80)
                } header: {
                    Text("Review (optional)")
                }
                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }
            }
            .navigationTitle("Rate this Agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: save) {
                        if isSaving { ProgressView() } else { Text("Save").fontWeight(.semibold) }
                    }
                    .disabled(isSaving)
                }
            }
        }
    }

    private func save() {
        isSaving = true
        saveError = nil
        Task {
            do {
                try await MarketplaceStore.shared.rate(
                    listingId: listingId, rating: rating,
                    review: review.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : review
                )
                await onSaved()
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}

/// Publishes a snapshot of ONE of the caller's own agents to the
/// marketplace. `sourceAgent` is pre-filled when opened from that agent's
/// detail view; otherwise the user picks one from their own registry.
struct PublishAgentSheet: View {
    let sourceAgent: AgentRecord?

    @Environment(\.dismiss) private var dismiss
    @StateObject private var hubStore = AgentHubStore.shared

    @State private var selectedAgentId: String?
    @State private var title = ""
    @State private var description = ""
    @State private var category = ""
    @State private var tagsText = ""
    @State private var isSaving = false
    @State private var saveError: String? = nil

    private var selectedAgentName: String {
        guard let selectedAgentId else { return "Choose an agent" }
        return hubStore.agents.first { $0.id == selectedAgentId }?.name ?? "Choose an agent"
    }

    init(sourceAgent: AgentRecord? = nil) {
        self.sourceAgent = sourceAgent
        _selectedAgentId = State(initialValue: sourceAgent?.id)
        _title = State(initialValue: sourceAgent?.name ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
                if sourceAgent == nil {
                    Section {
                        Menu {
                            Button(action: { selectedAgentId = nil }) {
                                HStack {
                                    if selectedAgentId == nil {
                                        Image(systemName: "checkmark")
                                    }
                                    Text("Choose an agent")
                                }
                            }
                            ForEach(hubStore.agents) { agent in
                                Button(action: { selectedAgentId = agent.id }) {
                                    HStack {
                                        if selectedAgentId == agent.id {
                                            Image(systemName: "checkmark")
                                        }
                                        Text(agent.name)
                                    }
                                }
                            }
                        } label: {
                            HStack {
                                Text(selectedAgentName)
                                    .foregroundColor(Color("TextPrimary"))
                                Spacer()
                                Image(systemName: "chevron.up.chevron.down")
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                    } header: {
                        Text("Agent")
                    }
                }
                Section {
                    TextField("Title", text: $title)
                    TextField("Category (optional)", text: $category)
                    TextField("Tags, comma-separated (optional)", text: $tagsText)
                } header: {
                    Text("Listing")
                }
                Section {
                    TextEditor(text: $description)
                        .frame(minHeight: 100)
                } header: {
                    Text("Description")
                } footer: {
                    Text("Publishing shares a snapshot of this agent's model, prompt, and configuration — installers get their own independent copy, not a live link back to yours.")
                }
                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }
            }
            .navigationTitle("Publish to Marketplace")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: publish) {
                        if isSaving { ProgressView() } else { Text("Publish").fontWeight(.semibold) }
                    }
                    .disabled(isSaving || selectedAgentId == nil || title.trimmingCharacters(in: .whitespacesAndNewlines).count < 3 || description.trimmingCharacters(in: .whitespacesAndNewlines).count < 10)
                }
            }
            .task {
                if sourceAgent == nil { hubStore.fetchAgentsIfNeeded() }
            }
        }
    }

    private func publish() {
        guard let selectedAgentId else { return }
        isSaving = true
        saveError = nil
        Task {
            do {
                let tags = tagsText.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                try await MarketplaceStore.shared.publish(
                    sourceAgentId: selectedAgentId, title: title, description: description,
                    category: category.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : category,
                    tags: tags
                )
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}
