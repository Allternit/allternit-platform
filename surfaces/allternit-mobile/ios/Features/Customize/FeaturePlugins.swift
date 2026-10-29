import SwiftUI

/// The built-in plugins the web's Plugins tab lists (src/plugins/
/// feature.registry.ts FEATURE_PLUGIN_REGISTRY: Core, Advanced, the three
/// Office add-ins and Chrome). Their on/off state is per device, as on the
/// web (feature.store.ts, localStorage `allternit:feature-plugins:v1`).
struct FeaturePlugin: Sendable {
    let id: String
    let name: String
    let version: String
    let description: String
    let category: String
    let author: String?
    let enabledByDefault: Bool
}

enum FeaturePlugins {
    static let all: [FeaturePlugin] = [
        FeaturePlugin(id: "core", name: "Core Features", version: "1.0.0",
                      description: "Basic agent functionality", category: "productivity",
                      author: nil, enabledByDefault: true),
        FeaturePlugin(id: "advanced", name: "Advanced Features", version: "1.0.0",
                      description: "Extended capabilities", category: "experimental",
                      author: nil, enabledByDefault: false),
        FeaturePlugin(id: "allternit-office-excel", name: "Allternit for Excel", version: "1.0.0",
                      description: "AI-powered Excel automation — formulas, charts, tables, financial modeling, and data validation. Generates and executes Office.js code directly in the workbook.",
                      category: "productivity", author: "Allternit", enabledByDefault: false),
        FeaturePlugin(id: "allternit-office-powerpoint", name: "Allternit for PowerPoint", version: "1.0.0",
                      description: "AI-powered slide creation, deck design, content generation and presentation automation. Generates and executes Office.js code directly in the presentation.",
                      category: "productivity", author: "Allternit", enabledByDefault: false),
        FeaturePlugin(id: "allternit-office-word", name: "Allternit for Word", version: "1.0.0",
                      description: "AI-powered document drafting, editing, redlining, style application and structured content. Generates and executes Office.js code directly in the document.",
                      category: "productivity", author: "Allternit", enabledByDefault: false),
        FeaturePlugin(id: "allternit-chrome", name: "Allternit for Chrome", version: "1.0.0",
                      description: "AI-powered browser companion — summarize pages, extract structured data, research across tabs, automate repetitive tasks, and save content to your connected tools.",
                      category: "productivity", author: "Allternit", enabledByDefault: false),
    ]

    private static let storageKey = "allternit:feature-plugins:v1"

    /// First run: the defaults (Core on).
    static func enabledIds() -> Set<String> {
        guard let ids = UserDefaults.standard.stringArray(forKey: storageKey) else {
            return Set(all.filter(\.enabledByDefault).map(\.id))
        }
        return Set(ids)
    }

    static func setEnabled(_ id: String, _ on: Bool) {
        var ids = enabledIds()
        if on { ids.insert(id) } else { ids.remove(id) }
        UserDefaults.standard.set(Array(ids).sorted(), forKey: storageKey)
    }
}

/// A built-in plugin's page: name, version and author, what it does, and
/// its switch (web ItemDetail for a FEATURE_PLUGIN_REGISTRY entry).
struct FeaturePluginDetailView: View {
    let item: CustomizeItem
    @Binding var enabled: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    HStack(spacing: 12) {
                        Image(systemName: "puzzlepiece.extension")
                            .font(.system(size: 18))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 44, height: 44)
                            .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.name)
                                .font(.system(size: 18, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Text([item.feature.map { "v\($0.version)" }, item.author.map { "from \($0)" }, "Built in"]
                                .compactMap { $0 }.joined(separator: " · "))
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextTertiary"))
                        }
                        Spacer(minLength: 0)
                        Toggle("", isOn: $enabled)
                            .labelsHidden()
                            .accessibilityLabel(enabled ? "Turn off \(item.name)" : "Turn on \(item.name)")
                    }
                    Text(item.description)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                    if let category = item.category {
                        Text(CustomizeScreen.titleCase(category))
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(Capsule().fill(Color("FillNeutral")))
                    }
                    Text("On or off for this device only, like the web app.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(24)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}
