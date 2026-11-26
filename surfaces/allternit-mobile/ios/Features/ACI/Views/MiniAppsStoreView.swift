import SwiftUI

/// Phase-1 Mini-apps Store for iOS.
///
/// Browse the community catalog and public MCP registry, pin/unpin apps, and
/// open them in the in-app MiniAppRuntimeView. Install/start/runtime are
/// deferred — iOS has no desktop bridge for those operations.
struct MiniAppsStoreView: View {
    @StateObject private var store = MiniAppCatalogStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var searchText = ""
    @State private var selectedCategory: MiniAppCategory? = nil
    @State private var selectedApp: MiniApp? = nil

    private let theme = ModeTheme(mode: .browser)

    private var filteredApps: [MiniApp] {
        store.apps.filter { app in
            let matchesSearch = searchText.isEmpty
                || app.name.localizedCaseInsensitiveContains(searchText)
                || app.description.localizedCaseInsensitiveContains(searchText)
            let matchesCategory = selectedCategory == nil || app.category == selectedCategory
            return matchesSearch && matchesCategory
        }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                header
                Divider().background(Color("BorderSubtle"))
                categoryBar
                Divider().background(Color("BorderSubtle"))
                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $selectedApp) { app in
                MiniAppRuntimeView(app: app)
            }
        }
        .task {
            store.fetchCatalogIfNeeded()
        }
    }

    // MARK: - Header

    private var header: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Mini Apps")
                        .font(.system(.title3, design: .serif))
                        .fontWeight(.medium)
                        .foregroundColor(Color("TextPrimary"))
                    Text("Browse connectors, runtimes, and tools")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }

                Spacer()

                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                        .background(Color("BgPanel"))
                        .clipShape(Circle())
                }
                .accessibilityLabel("Close")
            }

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search mini apps…", text: $searchText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .foregroundColor(Color("TextPrimary"))
                if !searchText.isEmpty {
                    Button(action: { searchText = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 40)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    // MARK: - Category bar

    private var categoryBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                categoryPill(nil)
                ForEach(MiniAppCategory.allCases, id: \.self) { category in
                    categoryPill(category)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
        }
    }

    private func categoryPill(_ category: MiniAppCategory?) -> some View {
        let selected = selectedCategory == category
        return Button(action: { selectedCategory = category }) {
            Text(category?.label ?? "All")
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(selected ? .black : Color("TextPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(selected ? theme.accent : Color("BgPanel"))
                .clipShape(Capsule())
                .overlay(
                    Capsule()
                        .stroke(selected ? Color.clear : Theme.borderWarmDefault, lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.apps.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.apps.isEmpty {
            Spacer()
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load catalog",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchCatalogIfNeeded(force: true) }
            )
            Spacer()
        } else if filteredApps.isEmpty {
            Spacer()
            emptyState
            Spacer()
        } else {
            listContent
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            title: "No mini apps found",
            message: searchText.isEmpty
                ? "Browse connectors, runtimes, and tools."
                : "No mini apps match your search.",
            art: .search
        )
    }

    private var listContent: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.flexible())], spacing: 12) {
                ForEach(filteredApps) { app in
                    appCard(app)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .refreshable {
            store.fetchCatalogIfNeeded(force: true)
        }
    }

    private func appCard(_ app: MiniApp) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                appIcon(app)

                VStack(alignment: .leading, spacing: 4) {
                    Text(app.name)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)

                    Text(app.publisher ?? app.catalogSource.rawValue.uppercased())
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }

                Spacer()

                if app.verified {
                    Image(systemName: "checkmark.shield")
                        .font(.system(size: 14))
                        .foregroundColor(Theme.statusSuccess)
                }
            }

            Text(app.description)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(3)
                .multilineTextAlignment(.leading)

            HStack(spacing: 6) {
                Text(app.category.label)
                    .font(.caption2)
                    .fontWeight(.semibold)
                    .foregroundColor(theme.accent)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(theme.accentSoft)
                    .clipShape(Capsule())

                ForEach(app.capabilities.prefix(2), id: \.self) { capability in
                    Text(capability)
                        .font(.caption2)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(Color("BgSecondary"))
                        .clipShape(Capsule())
                }
            }

            HStack(spacing: 10) {
                Button(action: { selectedApp = app }) {
                    HStack(spacing: 4) {
                        Image(systemName: "app.window")
                            .font(.system(size: 11, weight: .semibold))
                        Text("Open")
                            .font(.system(size: 12, weight: .semibold))
                    }
                    .foregroundColor(.black)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(theme.accent)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                }
                .buttonStyle(.plain)

                Button(action: { store.setPinned(!app.isPinned, for: app.id) }) {
                    Image(systemName: app.isPinned ? "pin.fill" : "pin")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(app.isPinned ? theme.accent : Color("TextSecondary"))
                        .frame(width: 36, height: 36)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(app.isPinned ? "Unpin app" : "Pin app")

                Spacer()
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func appIcon(_ app: MiniApp) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: Theme.radiusSM)
                .fill(Color("BgSecondary"))
            if let repo = app.repo {
                // GitHub avatar via the same service the web uses.
                AsyncImage(url: URL(string: "https://github.com/\(repo.split(separator: "/").first ?? Substring(repo)).png?size=64")) { phase in
                    if let image = phase.image {
                        image.resizable().scaledToFit()
                    } else {
                        Image(systemName: "app.window")
                            .font(.system(size: 18))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .frame(width: 40, height: 40)
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
            } else {
                Image(systemName: "app.window")
                    .font(.system(size: 18))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(width: 44, height: 44)
    }

}
