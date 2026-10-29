import SwiftUI

/// One curated template (web src/lib/templates/catalog.json, featured rows).
/// `prompt` is the full brief the composer receives (templatePrompt: the
/// prompt plus its style paragraph). Data: ArtifactTemplateCatalog.swift.
struct ArtifactTemplate: Identifiable, Sendable {
    let id: String
    let mode: AgentModeTile
    let title: String
    let description: String
    let prompt: String
    /// Site-relative preview image path, served by the web app.
    let preview: String
    let styleName: String?
    /// background, text, accent, accent-alt (or surface) — the card's dots.
    let swatches: [String]
    /// Gradient behind the preview while it loads.
    let placeholder: [String]
    let accent: String?

    /// Previews are the web app's public assets (m.allternit.com/images/…).
    var previewURL: URL? { URL(string: "https://m.allternit.com" + preview) }

    static func templates(for mode: AgentModeTile) -> [ArtifactTemplate] {
        catalog.filter { $0.mode == mode }
    }
}

/// The chat launch's template gallery (web ArtifactModeGallery +
/// TemplateGallery): quiet Docs · Slides · Sites · Sheets tabs, each showing
/// that kind's featured templates. The tab follows the deck's selected mode
/// until the viewer picks one; picking a template hands back its mode so the
/// send runs under that mode.
struct ArtifactModeGallery: View {
    /// The deck's selected mode, when agent mode is on.
    let preferredMode: AgentModeTile?
    let onSelectTemplate: (ArtifactTemplate) -> Void

    @State private var picked: AgentModeTile? = nil

    struct Tab: Identifiable {
        let mode: AgentModeTile
        let label: String
        var id: String { mode.rawValue }
    }

    static let tabs: [Tab] = [
        Tab(mode: .docs, label: "Docs"),
        Tab(mode: .slides, label: "Slides"),
        Tab(mode: .website, label: "Sites"),
        Tab(mode: .data, label: "Sheets"),
    ]

    private var active: AgentModeTile {
        if let picked { return picked }
        if let preferredMode, Self.tabs.contains(where: { $0.mode == preferredMode }) { return preferredMode }
        return Self.tabs[0].mode
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 4) {
                ForEach(Self.tabs) { tab in
                    let selected = tab.mode == active
                    Button(action: { picked = tab.mode }) {
                        HStack(spacing: 6) {
                            Text(tab.label)
                                .font(.system(size: 13))
                                .foregroundColor(selected ? Color("TextPrimary") : Color("TextSecondary"))
                            Text("\(ArtifactTemplate.templates(for: tab.mode).count)")
                                .font(.system(size: 11).monospacedDigit())
                                .foregroundColor(Color("TextTertiary"))
                                .opacity(0.7)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(selected ? Color("FillNeutral") : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(selected ? [.isSelected] : [])
                }
            }

            TemplateGallery(mode: active, onSelectTemplate: onSelectTemplate)
        }
    }
}

/// "FEATURED DOCS TEMPLATES · 6 templates" over a two-column card grid.
private struct TemplateGallery: View {
    let mode: AgentModeTile
    let onSelectTemplate: (ArtifactTemplate) -> Void

    private let columns = [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)]

    private var templates: [ArtifactTemplate] { ArtifactTemplate.templates(for: mode) }
    /// MODE_TABS label (Sites is "Build" there, as on the web).
    private var modeLabel: String { ChatDeckMode.forTile(mode)?.label ?? mode.label }

    var body: some View {
        if !templates.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 12) {
                    Text("Featured \(modeLabel) templates".uppercased())
                        .font(.system(size: 11, weight: .bold))
                        .tracking(1.5)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text("\(templates.count) templates")
                        .font(.system(size: 10))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(Color("FillNeutral").opacity(0.6), in: Capsule())
                        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                }

                LazyVGrid(columns: columns, alignment: .leading, spacing: 12) {
                    ForEach(templates) { template in
                        TemplateCard(template: template, modeLabel: modeLabel) {
                            UIImpactFeedbackGenerator(style: .light).impactOccurred()
                            onSelectTemplate(template)
                        }
                    }
                }
            }
        }
    }
}

private struct TemplateCard: View {
    let template: ArtifactTemplate
    let modeLabel: String
    let onPick: () -> Void

    var body: some View {
        Button(action: onPick) {
            VStack(alignment: .leading, spacing: 0) {
                preview
                VStack(alignment: .leading, spacing: 2) {
                    Text(template.title)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(template.description)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                        .lineSpacing(1)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        swatches
                        Text(template.styleName ?? modeLabel)
                            .font(.system(size: 11.5))
                            .foregroundColor(Color("TextTertiary"))
                            .lineLimit(1)
                    }
                    .padding(.top, 6)
                }
                .padding(.horizontal, 8)
                .padding(.top, 10)
                .padding(.bottom, 6)
                Spacer(minLength: 0)
            }
            .padding(4)
            .frame(maxHeight: .infinity, alignment: .top)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.04), radius: 1, y: 1)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(template.title). \(template.description)")
        .accessibilityHint("Use template")
    }

    /// 16:10 preview: the template's palette gradient with its initial until
    /// the image arrives (or if it can't load), as on the web.
    private var preview: some View {
        Color.clear
            .aspectRatio(16 / 10, contentMode: .fit)
            .background(
                LinearGradient(
                    colors: template.placeholder.map { Color(hex: $0) },
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            )
            .overlay {
                AsyncImage(url: template.previewURL, transaction: Transaction(animation: .easeOut(duration: 0.5))) { phase in
                    if let image = phase.image {
                        GeometryReader { proxy in
                            image
                                .resizable()
                                .scaledToFill()
                                .frame(width: proxy.size.width, height: proxy.size.height, alignment: .top)
                                .clipped()
                        }
                    } else {
                        Text(String(template.title.prefix(1)))
                            .font(.system(size: 17, weight: .semibold))
                            .foregroundColor(.white.opacity(0.9))
                            .frame(width: 40, height: 40)
                            .background(Color(hex: template.accent ?? "#1F1E1D"), in: RoundedRectangle(cornerRadius: 12))
                    }
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
    }

    private var swatches: some View {
        HStack(spacing: -4) {
            ForEach(Array(template.swatches.enumerated()), id: \.offset) { _, hex in
                Circle()
                    .fill(Color(hex: hex))
                    .frame(width: 12, height: 12)
                    .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 1))
            }
        }
        .accessibilityHidden(true)
    }
}
