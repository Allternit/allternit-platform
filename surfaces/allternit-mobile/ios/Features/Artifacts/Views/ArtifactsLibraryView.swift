import SwiftUI

#if DEBUG
/// String wrapper for `.sheet(item:)` debug deep-links.
private struct IdentifiableString: Identifiable {
    var wrappedValue: String
    var id: String { wrappedValue }
}
#endif

/// The Artifacts tab: the account's Artifacts v2 library (the same records
/// Desktop, ai.allternit.com and the PWA show), with "Make something new"
/// on top, Everything / Mine / Shared with me and kind filters, search,
/// pull to refresh and paging. A row opens the rendered artifact.
struct ArtifactsLibraryView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var store = ArtifactLibraryStore.shared
    @StateObject private var launcher = ArtifactStudioLauncher.shared
    @State private var openArtifact: ArtifactV2Summary?
    @State private var studio: ArtifactStudioSession?
    @State private var launchError: String?
    @State private var showingOfficeDocuments = false
    @State private var searchDraft = ""
    #if DEBUG
    @State private var debugOfficeDocumentId: IdentifiableString? = nil
    #endif

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary"))
        .sheet(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title)
        }
        .fullScreenCover(item: $studio, onDismiss: { Task { await store.reload() } }) { session in
            ArtifactStudioView(studio: session)
                .environmentObject(modeStore)
                .environmentObject(agentModeStore)
        }
        .sheet(isPresented: $showingOfficeDocuments) {
            OfficeDocumentsView()
        }
        .alert("Couldn't start that", isPresented: Binding(get: { launchError != nil }, set: { if !$0 { launchError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(launchError ?? "")
        }
        #if DEBUG
        .sheet(item: $debugOfficeDocumentId) { id in
            V2ArtifactView(artifactId: id.wrappedValue)
        }
        .task {
            // `-open-office-documents` / `-open-office-document-id <id>`
            // (DEBUG only): drive the Office surfaces without tap injection.
            let args = CommandLine.arguments
            if args.contains("-open-office-documents") {
                showingOfficeDocuments = true
            }
            if let idx = args.firstIndex(of: "-open-office-document-id"), args.indices.contains(idx + 1) {
                debugOfficeDocumentId = .init(wrappedValue: args[idx + 1])
            }
        }
        #endif
        .task {
            searchDraft = store.searchText
            if store.phase == .idle || store.artifacts.isEmpty { await store.reload() }
        }
        .task(id: searchDraft) {
            // Debounced server-side search (`q` matches titles).
            guard searchDraft != store.searchText else { return }
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            store.searchText = searchDraft
            await store.reload()
        }
    }

    // MARK: - Chrome

    private var header: some View {
        HStack {
            Button(action: {
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                    isSidebarOpen.toggle()
                }
            }) {
                Image(systemName: "line.3.horizontal")
                    .font(.title3)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Open sidebar")

            Text("Artifacts")
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))

            Spacer()

            Button(action: { showingOfficeDocuments = true }) {
                Image(systemName: "doc.text.magnifyingglass")
                    .font(.title3)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Office documents")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
        .background(Color("BgPrimary"))
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            TextField("Search artifacts", text: $searchDraft)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
            if !searchDraft.isEmpty {
                Button(action: { searchDraft = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private var filters: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(ArtifactLibraryStore.Scope.allCases) { scope in
                    chip(scope.label, selected: store.scope == scope) {
                        guard store.scope != scope else { return }
                        store.scope = scope
                        Task { await store.reload() }
                    }
                }
                Rectangle().fill(Color("BorderSubtle")).frame(width: 1, height: 20)
                chip("All kinds", selected: store.kind == nil) {
                    guard store.kind != nil else { return }
                    store.kind = nil
                    Task { await store.reload() }
                }
                ForEach(ArtifactV2Kind.filterKinds) { kind in
                    chip(kind.label, selected: store.kind == kind) {
                        store.kind = store.kind == kind ? nil : kind
                        Task { await store.reload() }
                    }
                }
            }
            .padding(.horizontal, 16)
        }
    }

    private func chip(_ title: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: {
            UISelectionFeedbackGenerator().selectionChanged()
            action()
        }) {
            Text(title)
                .font(.footnote.weight(.medium))
                .foregroundColor(selected ? Color("BgPrimary") : Color("TextPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(selected ? Color("TextPrimary") : Color("BgSecondary"))
                .clipShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    // MARK: - Make something new

    private var galleryTiles: [ArtifactStudioKind] {
        ArtifactStudioKind.allCases.filter { kind in
            guard let template = kind.templateKind, let settings = store.orgSettings else { return true }
            return settings.allows(kind: template)
        }
    }

    private var gallery: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Make something new")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 16)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 10) {
                    ForEach(galleryTiles) { kind in
                        Button { start(kind) } label: { tile(kind) }
                            .buttonStyle(.plain)
                            .disabled(launcher.launching != nil)
                    }
                }
                .padding(.horizontal, 16)
            }
        }
    }

    private func tile(_ kind: ArtifactStudioKind) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: kind.systemImage)
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if launcher.launching == kind {
                    ProgressView().controlSize(.small)
                } else if kind.isBeta {
                    Text("Beta")
                        .font(.caption2.weight(.semibold))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Text(kind.label)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text(kind.description)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(3)
                .multilineTextAlignment(.leading)
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(width: 168, height: 132, alignment: .topLeading)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint(kind.description)
    }

    private func start(_ kind: ArtifactStudioKind) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        Task {
            do {
                studio = try await launcher.launch(kind)
            } catch is CancellationError {
                return
            } catch {
                launchError = "Couldn't start a \(kind.noun.lowercased()) session. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    // MARK: - Library

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .signedOut:
            FriendlyStateView(
                style: .empty,
                icon: "person.crop.circle.badge.questionmark",
                title: "Sign in to see your artifacts",
                message: "Your docs, pages, dashboards and other artifacts live in your Allternit account."
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        default:
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    gallery.padding(.top, 14)
                    searchBar.padding(.horizontal, 16).padding(.top, 18)
                    filters.padding(.top, 10).padding(.bottom, 6)
                    libraryRows
                }
                .padding(.bottom, 24)
            }
            .refreshable { await store.reload() }
        }
    }

    @ViewBuilder
    private var libraryRows: some View {
        switch store.phase {
        case .idle, .loading:
            ProgressView()
                .frame(maxWidth: .infinity)
                .padding(.top, 40)
        case .failed(let message):
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load artifacts",
                message: FriendlyErrorMessage.from(message),
                actionTitle: "Retry",
                action: { Task { await store.reload() } }
            )
            .padding(.top, 24)
        case .loaded, .signedOut:
            if store.artifacts.isEmpty {
                emptyState.padding(.top, 24)
            } else {
                LazyVStack(spacing: 8) {
                    ForEach(store.artifacts) { saved in
                        if let summary = saved.summary {
                            Button {
                                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                openArtifact = summary
                            } label: {
                                ArtifactLibraryRow(summary: summary)
                            }
                            .buttonStyle(.plain)
                            .task { await store.loadMoreIfNeeded(current: saved) }
                        }
                    }
                    if store.isLoadingMore {
                        ProgressView().padding(.vertical, 12)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
            }
        }
    }

    private var emptyState: some View {
        let filtered = store.kind != nil || !store.searchText.isEmpty || store.scope != .all
        return FriendlyStateView(
            style: .empty,
            icon: filtered ? "magnifyingglass" : "archivebox",
            title: filtered ? "Nothing matches" : "No artifacts yet",
            message: filtered
                ? "Try another filter or search."
                : "Ask for a doc, deck, sheet or page in any chat, or start from Make something new above."
        )
    }
}

/// One library row (also used by the studio pane).
struct ArtifactLibraryRow: View {
    let summary: ArtifactV2Summary

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: ArtifactV2Kind.systemImage(for: summary.kind))
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 34, height: 34)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(summary.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(metadata)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 58)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    private var metadata: String {
        var parts: [String] = [summary.knownKind?.noun ?? summary.kind.capitalized]
        if !summary.isMine {
            parts.append(summary.owner.name.map { "Shared by \($0)" } ?? "Shared")
        } else if summary.isLegacy {
            parts.append("Legacy")
        }
        if let date = summary.updatedDate {
            parts.append(date.formatted(.relative(presentation: .named)))
        }
        return parts.joined(separator: " · ")
    }
}
