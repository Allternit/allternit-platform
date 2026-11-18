import SwiftUI

/// Models tab surface (rail item `.models`). Two segments, no new rail tab:
///
/// - **Catalog**: Freeweights models as identity faces with the one fit
///   function's label for this iPhone, and the detail sheet with Download.
/// - **On this iPhone**: running model, engines, downloads in progress,
///   downloaded models (Run / Remove) and the memory budget in plain words.
///
/// Provider status and the default model (`ModelManagementListContent`)
/// open from the header's sliders button, so nothing that was here is lost.
struct ModelsTabView: View {
    @Binding var isSidebarOpen: Bool

    enum Segment: String, CaseIterable, Identifiable {
        case catalog = "Catalog"
        case device = "On this iPhone"
        var id: String { rawValue }
    }

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var browse = FreeweightsBrowseStore()
    @AppStorage("allternit-models-segment") private var segment: Segment = .catalog
    @State private var isProvidersPresented = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                header
                Picker("Models view", selection: $segment) {
                    ForEach(Segment.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                .padding(.bottom, 10)

                switch segment {
                case .catalog: FreeweightsCatalogView(store: browse)
                case .device: OnThisIPhoneView(onBrowse: { segment = .catalog }, onRun: openChat)
                }
            }
            .background(Color("BgPrimary"))
            .freeweightsDetailSheet(browse)
            .sheet(isPresented: $isProvidersPresented) {
                NavigationStack {
                    ModelManagementView()
                        .toolbar {
                            ToolbarItem(placement: .topBarTrailing) { Button("Done") { isProvidersPresented = false } }
                        }
                }
            }
        }
        .onAppear { browse.onRun = openChat }
        .task {
            #if DEBUG
            if let s = UserDefaults.standard.string(forKey: "models-segment") { segment = s == "device" ? .device : .catalog }
            #endif
        }
    }

    /// Run: the model is already selected; switch to a fresh chat.
    private func openChat(_ id: String) {
        ModelStore.shared.userSelectedModel(id)
        modeStore.selectBarItem(.chats)
        NotificationCenter.default.post(name: .shellNewChat, object: nil)
    }

    // Header bar (matches AgentHubView's/ArtifactsLibraryView's chrome).
    private var header: some View {
        HStack {
            // Tap opens the drawer; hold fans the modes out (step 24).
            PhoneMenuButton {
                withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) { isSidebarOpen.toggle() }
            }

            Text("Models")
                .font(.system(.title3, design: .serif))
                .fontWeight(.medium)
                .foregroundColor(Color("TextPrimary"))

            Spacer()

            circleButton("slider.horizontal.3", label: "Providers and default model") { isProvidersPresented = true }
            circleButton("arrow.clockwise", label: "Refresh models") {
                Task { await browse.reload() }
                OnDeviceRuntime.shared.refreshInstalls()
            }
            .disabled(browse.isLoading)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
    }

    private func circleButton(_ icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 32, height: 32)
                .background(Color("BgPanel"))
                .clipShape(Circle())
        }
        .accessibilityLabel(label)
    }
}

// MARK: - Catalog segment

/// Freeweights models as identity faces (grid, the default; deck) or rows, each
/// with this iPhone's fit label. Chips: Fits this iPhone · Everything ·
/// Phone collection. Works signed out and offline (last copy on disk).
struct FreeweightsCatalogView: View {
    @ObservedObject var store: FreeweightsBrowseStore
    @ObservedObject private var rt = OnDeviceRuntime.shared
    @AppStorage("allternit-models-catalog-face") private var face = "grid"

    var body: some View {
        let ids = store.filter == .everything ? store.results.map(\.id) : store.localIds
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                searchRow
                chips
                content(ids)
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 24)
        }
        .refreshable { await store.reload() }
        .task {
            #if DEBUG
            if let f = UserDefaults.standard.string(forKey: "fw-filter"), let filter = FreeweightsBrowseStore.Filter.allCases.first(where: { "\($0)" == f }) { store.filter = filter }
            #endif
            await store.loadIfNeeded()
            #if DEBUG
            if let id = UserDefaults.standard.string(forKey: "fw-detail"), store.selected == nil { store.selected = store.summary(for: id) }
            #endif
        }
    }

    private var searchRow: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                TextField("Search Freeweights", text: $store.query)
                    .font(.system(size: 15))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.search)
                if !store.query.isEmpty {
                    Button { store.query = "" } label: { Image(systemName: "xmark.circle.fill").foregroundColor(Color("TextSecondary")) }
                        .buttonStyle(.plain).accessibilityLabel("Clear search")
                }
            }
            .padding(.horizontal, 10).padding(.vertical, 8)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 10))

            HStack(spacing: 2) {
                faceButton("list.bullet", "list", "List")
                faceButton("square.grid.2x2", "grid", "Grid")
                faceButton("rectangle.portrait", "deck", "Deck")
            }
            .padding(2)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 10))
        }
    }

    private func faceButton(_ icon: String, _ value: String, _ label: String) -> some View {
        Button { face = value } label: {
            Image(systemName: icon).font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 34, height: 30)
                .background(face == value ? Color("BgPrimary") : .clear, in: RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(face == value ? [.isSelected] : [])
    }

    private var chips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(FreeweightsBrowseStore.Filter.allCases) { f in
                    Button { store.filter = f } label: {
                        Text(f.rawValue)
                            .font(.system(size: 13, weight: .medium))
                            .padding(.horizontal, 12).padding(.vertical, 6)
                            .foregroundColor(store.filter == f ? Color("BgPrimary") : Color("TextPrimary"))
                            .background(store.filter == f ? Color("TextPrimary") : Color("FillNeutral"), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(store.filter == f ? [.isSelected] : [])
                }
            }
        }
    }

    @ViewBuilder
    private func content(_ ids: [String]) -> some View {
        let waitingForIndex = store.filter != .everything && store.deviceIndex == nil
        if store.isLoading && (store.filter == .everything ? store.results.isEmpty : store.catalog.isEmpty) {
            ProgressView("Loading the catalog").frame(maxWidth: .infinity).padding(.top, 40)
        } else if store.downReason != nil, store.catalog.isEmpty, store.results.isEmpty {
            FriendlyInlineStateView(
                style: .offline, icon: "wifi.slash",
                title: "Freeweights is not answering",
                message: "The catalog couldn't be reached and there is no copy on this iPhone yet. Check your connection, then try again.",
                actionTitle: "Retry", action: { Task { await store.reload() } })
            .padding(.top, 20)
        } else if waitingForIndex && store.hasLoaded {
            FriendlyInlineStateView(
                style: .offline, icon: "iphone.slash",
                title: "Fit data didn't load",
                message: "Freeweights didn't send the list of builds, so this iPhone can't tell what fits. Everything still shows the catalog.",
                actionTitle: "Show everything", action: { store.filter = .everything })
            .padding(.top, 20)
        } else if store.hasLoaded && ids.isEmpty {
            FriendlyInlineStateView(
                style: .empty, icon: "magnifyingglass",
                title: store.query.isEmpty ? "Nothing here yet" : "No models match \u{201C}\(store.query)\u{201D}",
                message: store.filter == .fits ? "Try Everything, or a family name like qwen, gemma or smollm." : "Try a family name like qwen, gemma or deepseek.")
            .padding(.top, 20)
        } else {
            budgetLine
            let shown = store.filter == .everything ? ids : Array(ids.prefix(store.localLimit))
            if face == "deck" {
                deck(shown)
            } else if face == "list" {
                VStack(spacing: 0) {
                    ForEach(shown, id: \.self) { id in
                        listRow(id)
                        if id != shown.last { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                    }
                }
            } else {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10, alignment: .top), GridItem(.flexible(), spacing: 10, alignment: .top)], spacing: 14) {
                    ForEach(shown, id: \.self) { id in tile(id) }
                }
            }
            more(total: ids.count, shown: shown.count)
            if let age = store.staleAge {
                Text("Offline copy from \(FreeweightsSection.ageText(age)). Pull to refresh.")
                    .font(.caption).foregroundColor(Color("TextSecondary"))
            }
        }
    }

    private var budgetLine: some View {
        Text("This iPhone can give a model up to \(FwText.deviceSize(rt.budget.fastBytes)) of memory.")
            .font(.system(size: 12))
            .foregroundColor(Color("TextSecondary"))
    }

    private func faceModel(_ id: String, _ fit: FitResult) -> FwFaceModel {
        let row = store.catalog[id]
        return FwFaceModel(summary: store.results.first { $0.id == id } ?? store.summary(for: id), row: row, id: id,
                           sizeBytes: fit.quant?.size ?? store.quants(id)?.map(\.size).min(),
                           formats: fit.quant.map { [$0.format] })
    }

    private func tile(_ id: String) -> some View {
        let fit = store.fit(id)
        return Button { store.selected = store.summary(for: id) } label: {
            VStack(alignment: .leading, spacing: 6) {
                FwIdentityFace(model: faceModel(id, fit), compact: true)
                FitLabel(fit: fit)
            }
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens the model, its builds and Download")
    }

    /// Deck (the site's third face): one large identity face per page,
    /// swiped sideways, with the fit label under it.
    private func deck(_ ids: [String]) -> some View {
        TabView {
            ForEach(ids, id: \.self) { id in
                let fit = store.fit(id)
                Button { store.selected = store.summary(for: id) } label: {
                    VStack(alignment: .leading, spacing: 10) {
                        FwIdentityFace(model: faceModel(id, fit))
                        FitLabel(fit: fit)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 2)
                }
                .buttonStyle(.plain)
                .accessibilityHint("Opens the model, its builds and Download")
            }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        .frame(height: 380)
    }

    private func listRow(_ id: String) -> some View {
        let fit = store.fit(id)
        let m = faceModel(id, fit)
        return Button { store.selected = store.summary(for: id) } label: {
            HStack(spacing: 12) {
                FwDotWord(text: m.mono, height: 16)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color("TextPrimary"), lineWidth: 1.5))
                VStack(alignment: .leading, spacing: 3) {
                    Text(m.name).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                    Text([FwText.params(m.params).map { "\($0) params" }, id.split(separator: "/").first.map(String.init)].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 11.5, design: .monospaced)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                    FitLabel(fit: fit)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold)).foregroundColor(Color("TextTertiary"))
            }
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func more(total: Int, shown: Int) -> some View {
        let hasMore = store.filter == .everything ? store.nextCursor != nil : total > shown
        if hasMore {
            Button {
                if store.filter == .everything { Task { await store.loadMore() } } else { store.localLimit += FreeweightsBrowseStore.localPage }
            } label: {
                HStack {
                    Text(store.filter == .everything ? "Show more" : "Show more (\(total - shown) left)").font(.subheadline.weight(.medium))
                    if store.isLoadingMore { ProgressView().padding(.leading, 4) }
                }
                .frame(maxWidth: .infinity).padding(.vertical, 10)
            }
            .disabled(store.isLoadingMore)
        }
    }
}

// MARK: - On this iPhone segment

/// What runs here: the loaded model and the memory budget, the engines,
/// downloads in progress, and downloaded models with Run and Remove.
/// Everything here works offline and signed out.
struct OnThisIPhoneView: View {
    var onBrowse: () -> Void
    var onRun: (String) -> Void

    @ObservedObject private var rt = OnDeviceRuntime.shared
    @ObservedObject private var downloads = OnDeviceDownloads.shared
    @State private var confirmRemove: OnDeviceInstall?

    var body: some View {
        List {
            memorySection
            enginesSection
            if !downloads.active.isEmpty || !downloads.errors.isEmpty { downloadsSection }
            downloadedSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .onAppear { rt.refreshInstalls() }
        .confirmationDialog("Remove this model from your iPhone?", isPresented: Binding(get: { confirmRemove != nil }, set: { if !$0 { confirmRemove = nil } }), titleVisibility: .visible, presenting: confirmRemove) { i in
            Button("Remove \(FwText.deviceSize(Double(i.sizeBytes)))", role: .destructive) { Task { await rt.delete(i) } }
        } message: { _ in Text("You can download it again from the catalog.") }
    }

    private func short(_ id: String) -> String { id.split(separator: "/").last.map(String.init) ?? id }

    // Running now + the budget in plain words.
    private var memorySection: some View {
        Section {
            if let l = rt.loaded {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(l.install.map { short($0.modelId) } ?? "Apple Intelligence").font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                        Text(runningDetail(l)).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    Button("Stop") { Task { await rt.unload(reason: "stopped by user") } }.buttonStyle(.borderless)
                }
            }
            let used = Double(rt.loaded.map { $0.memBytes > 0 ? $0.memBytes : ($0.install?.sizeBytes ?? 0) } ?? 0)
            VStack(alignment: .leading, spacing: 6) {
                ProgressView(value: min(1, used / max(1, rt.budget.fastBytes)))
                    .tint(Color("TextPrimary"))
                Text(rt.loaded == nil
                     ? "Nothing is loaded. Allternit can give a model up to \(FwText.deviceSize(rt.budget.fastBytes)) of memory on this iPhone."
                     : "\(FwText.deviceSize(used)) of \(FwText.deviceSize(rt.budget.fastBytes)) this app can use for models")
                    .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.vertical, 2)
            .accessibilityElement(children: .combine)
        } header: {
            Text(rt.loaded == nil ? "Memory" : "Running now")
        } footer: {
            Text("Models unload when Allternit goes to the background, on a low-memory warning, and after 5 idle minutes.")
        }
    }

    private func runningDetail(_ l: OnDeviceRuntime.Loaded) -> String {
        var parts = [l.engine == .mlx ? "MLX" : l.engine == .llamaCpp ? "llama.cpp" : "Built in"]
        if let s = rt.lastStats, s.tokensPerSecond > 0 { parts.append(String(format: "%.0f tok/s", s.tokensPerSecond)) }
        parts.append("unloads in the background")
        return parts.joined(separator: " · ")
    }

    private var enginesSection: some View {
        Section("Engines") {
            ForEach(rt.runtimes) { r in
                HStack {
                    Circle().fill(r.state == .ready ? Theme.statusSuccess : r.state == .off ? Theme.statusWarning : Color("TextTertiary"))
                        .frame(width: 7, height: 7)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(engineName(r)).font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                        Text(r.note ?? engineDetail(r)).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    if r.id == .appleFM, r.state == .ready {
                        Button("Use") { ModelStore.shared.userSelectedModel(OnDeviceModelId.appleFM); onRun(OnDeviceModelId.appleFM) }
                            .buttonStyle(.borderless)
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    private func engineName(_ r: DeviceRuntime) -> String {
        switch r.id {
        case .iosMLX: return "MLX"
        case .iosLlama: return "llama.cpp"
        case .appleFM: return "Apple Intelligence"
        default: return r.engine
        }
    }

    private func engineDetail(_ r: DeviceRuntime) -> String {
        switch r.id {
        case .iosMLX: return "Metal · fastest for MLX models"
        case .iosLlama: return "Metal · runs GGUF models"
        case .appleFM: return "Built in · no download"
        default: return r.version ?? ""
        }
    }

    private var downloadsSection: some View {
        Section("Downloads") {
            ForEach(Array(downloads.active.union(downloads.errors.keys)).sorted(), id: \.self) { key in
                let parts = key.split(separator: "#").map(String.init)
                let p = rt.progress[key]
                HStack {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("\(short(parts.first ?? key)) · \(parts.last ?? "")").font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                        if let error = downloads.errors[key] {
                            Text(error).font(.system(size: 12)).foregroundColor(Theme.statusError)
                        } else {
                            ProgressView(value: p?.fraction ?? 0).tint(Color("TextPrimary"))
                            Text(p.map { $0.totalBytes > 0 ? "\(Int(($0.fraction * 100).rounded()))% · \(FwText.deviceSize(Double($0.receivedBytes))) of \(FwText.deviceSize(Double($0.totalBytes))) · background" : "Checking the signed manifest…" } ?? "Starting…")
                                .font(.system(size: 12).monospacedDigit()).foregroundColor(Color("TextSecondary"))
                        }
                    }
                    Spacer()
                    if downloads.errors[key] != nil {
                        Button("Dismiss") { downloads.clearError(key) }.buttonStyle(.borderless)
                    } else {
                        Button("Cancel") { downloads.cancel(modelId: parts.first ?? "", quant: parts.last ?? "") }.buttonStyle(.borderless)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var downloadedSection: some View {
        Section {
            if rt.installs.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("No models on this iPhone yet").font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    Text("Pick one that fits in the catalog. It downloads in the background and then runs here, even offline.")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                    Button("Browse models that fit", action: onBrowse).font(.system(size: 15, weight: .semibold))
                }
                .padding(.vertical, 4)
            } else {
                ForEach(rt.installs) { i in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(short(i.modelId)).font(.system(size: 15)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                            Text("\(i.quant) · \(i.engine == .mlx ? "MLX" : "llama.cpp") · \(FwText.deviceSize(Double(i.sizeBytes)))\(i.verified ? " · checked" : "")")
                                .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                        }
                        Spacer()
                        Button("Remove") { confirmRemove = i }
                            .font(.system(size: 15))
                            .foregroundColor(Theme.statusError)
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Remove \(short(i.modelId))")
                        Button("Run") { onRun(OnDeviceModelId.make(i)) }
                            .font(.system(size: 15, weight: .semibold))
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Run \(short(i.modelId)) in chat")
                    }
                }
            }
        } header: {
            Text("On this iPhone")
        } footer: {
            Text("Models stay on this iPhone and are not backed up to iCloud. \(FwText.deviceSize(Double(rt.storageBytes))) used, \(FwText.deviceSize(Double(rt.freeDiskBytes))) free.")
        }
    }
}
