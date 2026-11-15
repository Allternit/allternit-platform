import SwiftUI

/// ACI › ACI Extensions, the web's `BrowserExtensionsView compact`
/// (views/BrowserExtensionsView.tsx): header, one compact bar (category
/// chips, search, Marketplace, Upload) and the extension cards.
///
/// The list lives on the device, like the web's persisted
/// browserExtensions.store: the four Allternit extensions plus whatever was
/// installed from the marketplace or uploaded.
struct ACIExtensionsView: View {
    @StateObject private var store = ACIExtensionsStore.shared
    @Environment(\.openURL) private var openURL
    @State private var query = ""
    @State private var category: ACIExtension.Category? = nil
    @State private var showMarketplace = false
    @State private var showUpload = false
    @State private var detail: ACIExtension? = nil
    @State private var toast: ACIToast.Message? = nil

    private var filtered: [ACIExtension] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return store.extensions.filter { ext in
            (q.isEmpty || ext.name.lowercased().contains(q) || ext.description.lowercased().contains(q))
                && (category == nil || ext.category == category)
        }
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                ACIViewHeader(title: "ACI Extensions", bottom: 16)
                    .padding(.horizontal, 16)
                Section {
                    content
                        .padding(.horizontal, 16)
                        .padding(.top, 16)
                        .padding(.bottom, 48)
                } header: {
                    bar.background(Color("BgPrimary"))
                }
            }
        }
        .background(Color("BgPrimary"))
        .overlay(alignment: .bottom) { ACIToast(message: $toast) }
        .sheet(isPresented: $showUpload) {
            ACIUploadExtensionSheet { name, description, url in
                store.addCustom(name: name, description: description, storeURL: url)
                toast = .init(kind: .success, title: "Added and enabled \(name)")
            }
        }
        .sheet(item: $detail) { ext in
            ACIExtensionDetailSheet(extension: ext, isMarketplace: !store.extensions.contains(where: { $0.id == ext.id })) { action in
                perform(action, on: ext)
            }
        }
    }

    private var bar: some View {
        ACICompactBar(query: $query, searchLabel: "Search extensions", placeholder: "Search extensions") {
            ACIBarChip(title: "All", isOn: category == nil) { category = nil }
            ForEach(ACIExtension.Category.allCases, id: \.self) { item in
                ACIBarChip(title: item.label, isOn: category == item) { category = item }
            }
        } actions: {
            ACIBarIcon(systemName: "storefront", label: showMarketplace ? "My Extensions" : "Marketplace", isOn: showMarketplace) {
                showMarketplace.toggle()
            }
            ACIBarIcon(systemName: "plus", label: "Upload Extension") { showUpload = true }
        }
    }

    @ViewBuilder
    private var content: some View {
        if showMarketplace {
            VStack(alignment: .leading, spacing: 20) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Extension Marketplace")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Install curated browser capabilities into Allternit.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                }
                ForEach(ACIExtension.marketplace) { ext in
                    ACIExtensionCard(extension: ext, onOpen: { detail = ext }, onToggle: {}, onRemove: {}, onInstall: {
                        perform(.install, on: ext)
                    })
                }
            }
        } else if filtered.isEmpty {
            let filteredOut = !query.isEmpty || category != nil
            VStack(spacing: 16) {
                VStack(spacing: 8) {
                    Image(systemName: "puzzlepiece.extension")
                        .font(.system(size: 52, weight: .thin))
                        .foregroundColor(Color("TextTertiary"))
                    Text("No extensions found")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(filteredOut ? "Try adjusting your filters or search query." : "Install ACI extensions to give your agent new capabilities.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                    ACIPrimaryButton(title: filteredOut ? "Clear filters" : "Browse marketplace", height: 40) {
                        if filteredOut {
                            query = ""
                            category = nil
                        } else {
                            showMarketplace = true
                        }
                    }
                    .frame(maxWidth: 220)
                    .padding(.top, 4)
                }
                VStack(spacing: 4) {
                    Text("Build your own extensions")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Our SDK allows you to create native ACI extensions that interact with agent context, handle page automation, and surface unique tools.")
                        .font(.system(size: 13))
                        .lineSpacing(3)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                        .padding(.bottom, 12)
                    Button {
                        if let url = URL(string: "https://allternit.com/developers") { openURL(url) }
                    } label: {
                        HStack(spacing: 6) {
                            Text("Open Developer Documentation")
                            Image(systemName: "arrow.up.right.square")
                        }
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    }
                    .buttonStyle(.plain)
                }
                .padding(20)
                .frame(maxWidth: .infinity)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .padding(.vertical, 48)
        } else {
            VStack(spacing: 20) {
                ForEach(filtered) { ext in
                    ACIExtensionCard(
                        extension: ext,
                        onOpen: { detail = ext },
                        onToggle: { perform(.toggle, on: ext) },
                        onRemove: { perform(.remove, on: ext) },
                        onInstall: { perform(.install, on: ext) }
                    )
                }
            }
        }
    }

    // MARK: Actions (useExtensionsManager)

    enum Action { case toggle, install, remove }

    private func perform(_ action: Action, on ext: ACIExtension) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        switch action {
        case .toggle:
            store.toggle(ext.id)
            toast = .init(kind: .success, title: "\(ext.name) \(ext.enabled ? "disabled" : "enabled in the browser toolbar")")
        case .install:
            if store.extensions.contains(where: { $0.id == ext.id }) {
                store.install(ext.id)
            } else {
                store.installFromMarketplace(ext)
            }
            toast = .init(kind: .success, title: "Installed and enabled \(ext.name)")
        case .remove:
            if ext.owned {
                store.setEnabled(ext.id, false)
                toast = .init(kind: .success, title: "\(ext.name) disabled")
            } else {
                store.remove(ext.id)
                toast = .init(kind: .success, title: "Uninstalled \(ext.name)")
            }
        }
    }
}

// MARK: - Model

struct ACIExtension: Identifiable, Codable, Equatable, Sendable {
    enum Category: String, Codable, CaseIterable, Sendable {
        case productivity, automation, ai, utilities, office

        var label: String {
            switch self {
            case .productivity: return "Productivity"
            case .automation: return "Automation"
            case .ai: return "AI Powered"
            case .utilities: return "Utilities"
            case .office: return "Office"
            }
        }
    }

    var id: String
    var name: String
    var description: String
    var icon: String
    var version: String
    var author: String
    var category: Category
    var enabled: Bool
    var installed: Bool
    var owned: Bool = false
    var storeURL: String? = nil
    var permissions: [String] = []

    /// browserExtensions.store `ALLTERNIT_OWNED_EXTENSIONS`.
    static let allternit: [ACIExtension] = [
        ACIExtension(id: "allternit-agent", name: "Allternit Computer Agent",
                     description: "The attached browser extension for Allternit browser-use and computer-use.",
                     icon: "◎", version: "1.0.0", author: "Allternit", category: .ai, enabled: true, installed: true,
                     owned: true, permissions: ["scripts", "clipboard", "downloads"]),
        ACIExtension(id: "allternit-office-word", name: "Allternit for Word",
                     description: "Allternit Office agent and document tools for Microsoft Word.",
                     icon: "W", version: "1.0.0", author: "Allternit", category: .office, enabled: false, installed: false, owned: true),
        ACIExtension(id: "allternit-office-excel", name: "Allternit for Excel",
                     description: "Allternit Office agent and workbook tools for Microsoft Excel.",
                     icon: "X", version: "1.0.0", author: "Allternit", category: .office, enabled: false, installed: false, owned: true),
        ACIExtension(id: "allternit-office-powerpoint", name: "Allternit for PowerPoint",
                     description: "Allternit Office agent and presentation tools for Microsoft PowerPoint.",
                     icon: "P", version: "1.0.0", author: "Allternit", category: .office, enabled: false, installed: false, owned: true),
    ]

    /// extension-marketplace.ts `EXTENSION_MARKETPLACE`.
    static let marketplace: [ACIExtension] = [
        ACIExtension(id: "market-claude-chrome", name: "Claude in Chrome",
                     description: "AI assistant for navigating sites and completing multi-step browser workflows.",
                     icon: "✦", version: "latest", author: "Anthropic", category: .ai, enabled: false, installed: false,
                     storeURL: "https://chromewebstore.google.com/detail/claude/fcoeoabgfenejglbffodgkkbkcdhcgfn", permissions: ["tabs", "scripts"]),
        ACIExtension(id: "market-ublock-origin", name: "uBlock Origin",
                     description: "Fast, efficient and privacy-focused content blocking for the browser.",
                     icon: "U", version: "latest", author: "Raymond Hill", category: .utilities, enabled: false, installed: false,
                     storeURL: "https://chromewebstore.google.com/detail/ublock-origin/cjpalhdlnbpafiamejdnhcphjbkeiagm", permissions: ["tabs"]),
        ACIExtension(id: "market-1password", name: "1Password",
                     description: "Secure password manager and digital wallet with browser autofill.",
                     icon: "1P", version: "latest", author: "1Password", category: .productivity, enabled: false, installed: false,
                     storeURL: "https://chromewebstore.google.com/detail/1password-password-manage/aeblfdkhhhdcdjpifhhbdiojplfjncoa", permissions: ["tabs", "clipboard"]),
        ACIExtension(id: "market-grammarly", name: "Grammarly",
                     description: "Writing assistant for grammar, spelling, clarity and tone.",
                     icon: "G", version: "latest", author: "Grammarly", category: .productivity, enabled: false, installed: false,
                     storeURL: "https://chromewebstore.google.com/detail/grammarly-grammar-checker/kbfnbcaeplbcioakkpcpgfkobkghlhen", permissions: ["scripts"]),
    ]
}

/// The extensions list, persisted on the device (the web persists it in
/// localStorage under its zustand store).
@MainActor
final class ACIExtensionsStore: ObservableObject {
    static let shared = ACIExtensionsStore()

    @Published private(set) var extensions: [ACIExtension] {
        didSet { save() }
    }

    private static let key = "allternit.aci-extensions"

    private init() {
        if let data = UserDefaults.standard.data(forKey: Self.key),
           let saved = try? JSONDecoder().decode([ACIExtension].self, from: data) {
            // The Allternit extensions always exist; keep their saved state.
            let missing = ACIExtension.allternit.filter { owned in !saved.contains(where: { $0.id == owned.id }) }
            extensions = missing + saved
        } else {
            extensions = ACIExtension.allternit
        }
    }

    func toggle(_ id: String) {
        update(id) { $0.enabled.toggle() }
    }

    func setEnabled(_ id: String, _ enabled: Bool) {
        update(id) { $0.enabled = enabled }
    }

    func install(_ id: String) {
        update(id) {
            $0.installed = true
            $0.enabled = true
        }
    }

    func remove(_ id: String) {
        extensions.removeAll { $0.id == id }
    }

    func installFromMarketplace(_ ext: ACIExtension) {
        if let existing = extensions.first(where: { $0.name == ext.name }) {
            install(existing.id)
            return
        }
        var copy = ext
        copy.id = "ext-\(UUID().uuidString.prefix(8).lowercased())"
        copy.installed = true
        copy.enabled = true
        extensions.append(copy)
    }

    func addCustom(name: String, description: String, storeURL: String?) {
        extensions.append(ACIExtension(
            id: "ext-\(UUID().uuidString.prefix(8).lowercased())",
            name: name,
            description: description.isEmpty ? "Custom ACI extension" : description,
            icon: "E", version: "1.0.0", author: "Custom", category: .utilities,
            enabled: true, installed: true, storeURL: storeURL
        ))
    }

    private func update(_ id: String, _ change: (inout ACIExtension) -> Void) {
        guard let index = extensions.firstIndex(where: { $0.id == id }) else { return }
        change(&extensions[index])
    }

    private func save() {
        if let data = try? JSONEncoder().encode(extensions) {
            UserDefaults.standard.set(data, forKey: Self.key)
        }
    }
}

// MARK: - Card

/// ExtensionCard: logo + category badge, name (green dot when on), version
/// and author, description, "View details", then the toggle row or Install.
private struct ACIExtensionCard: View {
    let `extension`: ACIExtension
    let onOpen: () -> Void
    let onToggle: () -> Void
    let onRemove: () -> Void
    let onInstall: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onOpen) {
                VStack(alignment: .leading, spacing: 0) {
                    HStack(alignment: .top) {
                        ACIExtensionLogo(extension: `extension`)
                        Spacer()
                        Text(`extension`.category.rawValue.uppercased())
                            .font(.system(size: 10, weight: .semibold))
                            .tracking(0.6)
                            .foregroundColor(Color("TextTertiary"))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(Color("FillNeutral"))
                            .clipShape(Capsule())
                    }
                    .padding(.bottom, 16)
                    HStack(spacing: 8) {
                        Text(`extension`.name)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if `extension`.installed && `extension`.enabled {
                            Circle().fill(Theme.statusSuccess).frame(width: 8, height: 8)
                        }
                    }
                    Text("v\(`extension`.version) · \(`extension`.author)")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.top, 4)
                    Text(`extension`.description)
                        .font(.system(size: 13))
                        .lineSpacing(4)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(3)
                        .multilineTextAlignment(.leading)
                        .padding(.top, 8)
                        .padding(.bottom, 16)
                    Text("View details")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                .padding(.top, 16)
            HStack(spacing: 12) {
                if `extension`.installed {
                    Button(action: onToggle) {
                        HStack(spacing: 8) {
                            ACIMiniSwitch(isOn: `extension`.enabled)
                            Text(`extension`.enabled ? "Active" : "Off")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundColor(`extension`.enabled ? Color("TextPrimary") : Color("TextTertiary"))
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(`extension`.enabled ? "Disable extension" : "Enable extension")
                    Spacer()
                    Button(action: onRemove) {
                        Image(systemName: `extension`.owned ? "xmark" : "trash")
                            .font(.system(size: 15))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 32, height: 32)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(`extension`.owned ? "Remove extension" : "Uninstall extension")
                } else {
                    Button(action: onInstall) {
                        HStack(spacing: 6) {
                            Image(systemName: "plus").font(.system(size: 14, weight: .bold))
                            Text("Install Extension").font(.system(size: 14, weight: .medium))
                        }
                        .foregroundColor(Color("OnAccent"))
                        .frame(maxWidth: .infinity, minHeight: 36)
                        .background(Color("AccentPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                }
            }
            .frame(minHeight: 36)
            .padding(.top, 12)
        }
        .padding(20)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

/// ToggleRight / ToggleLeft: a small switch drawn, not a control.
private struct ACIMiniSwitch: View {
    let isOn: Bool

    var body: some View {
        Capsule()
            .fill(isOn ? Color("AccentPrimary") : Color("BorderStrong"))
            .frame(width: 26, height: 16)
            .overlay(alignment: isOn ? .trailing : .leading) {
                Circle().fill(isOn ? Color("OnAccent") : Color("BgPanel")).frame(width: 10, height: 10).padding(3)
            }
    }
}

/// ExtensionLogo: the Allternit mark for the agent, a letter tile otherwise
/// (Office tiles in their app colour).
private struct ACIExtensionLogo: View {
    let `extension`: ACIExtension

    private var tint: Color? {
        switch `extension`.id {
        case "allternit-office-word": return Color(hex: "#185ABD")
        case "allternit-office-excel": return Color(hex: "#107C41")
        case "allternit-office-powerpoint": return Color(hex: "#C43E1C")
        default: return nil
        }
    }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8)
                .fill(Color("BgPrimary"))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderStrong"), lineWidth: 1))
            if `extension`.id == "allternit-agent" {
                Image("MatrixLogo").resizable().scaledToFit().padding(8)
            } else if let tint {
                Text(`extension`.icon)
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(.white)
                    .frame(width: 18, height: 18)
                    .background(tint)
                    .clipShape(RoundedRectangle(cornerRadius: 3))
            } else {
                Text(`extension`.icon)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(width: 28, height: 28)
        .accessibilityHidden(true)
    }
}

// MARK: - Sheets

/// ExtensionDetailView, as a sheet: what the extension is, what it may do,
/// and the same install / toggle / remove actions as the card.
private struct ACIExtensionDetailSheet: View {
    let `extension`: ACIExtension
    let isMarketplace: Bool
    let perform: (ACIExtensionsView.Action) -> Void
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(spacing: 12) {
                        ACIExtensionLogo(extension: `extension`)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(`extension`.name)
                                .font(.system(size: 18, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Text("v\(`extension`.version) · \(`extension`.author)")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextTertiary"))
                        }
                    }
                    Text(`extension`.description)
                        .font(.system(size: 14))
                        .lineSpacing(4)
                        .foregroundColor(Color("TextSecondary"))
                    if !`extension`.permissions.isEmpty {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("Permissions")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            ForEach(`extension`.permissions, id: \.self) { permission in
                                Text(permission.capitalized)
                                    .font(.system(size: 13))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                    }
                    if let raw = `extension`.storeURL, let url = URL(string: raw) {
                        Button { openURL(url) } label: {
                            HStack(spacing: 6) {
                                Text("Open store page")
                                Image(systemName: "arrow.up.right.square")
                            }
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        }
                        .buttonStyle(.plain)
                    }
                    if isMarketplace || !`extension`.installed {
                        ACIPrimaryButton(title: "Install Extension", height: 44) {
                            perform(.install)
                            dismiss()
                        }
                    } else {
                        HStack(spacing: 8) {
                            ACIPillButton(title: `extension`.enabled ? "Disable" : "Enable") {
                                perform(.toggle)
                                dismiss()
                            }
                            ACIPillButton(title: `extension`.owned ? "Remove" : "Uninstall") {
                                perform(.remove)
                                dismiss()
                            }
                        }
                    }
                }
                .padding(20)
            }
            .background(Color("BgPanel").ignoresSafeArea())
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { dismiss() } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                            Text("Back")
                        }
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// "Upload extension": name, description, package or store URL.
private struct ACIUploadExtensionSheet: View {
    let onAdd: (String, String, String?) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var description = ""
    @State private var url = ""

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                field("Name") {
                    TextField("", text: $name)
                        .padding(.horizontal, 12)
                        .frame(height: 40)
                }
                field("Description") {
                    TextField("", text: $description, axis: .vertical)
                        .lineLimit(3, reservesSpace: true)
                        .padding(12)
                }
                field("Package or store URL") {
                    TextField("https://…", text: $url)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .padding(.horizontal, 12)
                        .frame(height: 40)
                }
                Spacer()
            }
            .padding(20)
            .background(Color("BgPanel").ignoresSafeArea())
            .navigationTitle("Upload extension")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add extension") {
                        let trimmedURL = url.trimmingCharacters(in: .whitespaces)
                        onAdd(name.trimmingCharacters(in: .whitespaces),
                              description.trimmingCharacters(in: .whitespaces),
                              trimmedURL.isEmpty ? nil : trimmedURL)
                        dismiss()
                    }
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func field<Content: View>(_ label: String, @ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
            content()
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .background(Color("BgPrimary"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderStrong"), lineWidth: 1))
        }
    }
}
