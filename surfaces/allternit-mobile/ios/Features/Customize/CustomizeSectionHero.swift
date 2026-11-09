import SwiftUI

// -----------------------------------------------------------------------------
// The Customize section hero for Skills and Plugins: the web's SectionHero
// (allternit-ai src/views/customize/SectionHero.tsx + hero.css). One panel,
// ink in light mode and white in dark mode, led by the section's A://TERNIT
// lockup and its sweep. It shows the official Allternit items in turn (every
// 4.5 s), each with its cover art on the right fading in from a blurred copy;
// dots jump to an item. Items with no art show only the copy. Reduced motion:
// no cycling.
// -----------------------------------------------------------------------------

/// OFFICIAL_COVERS (OfficialShelf.tsx): cover art per official item, keyed by
/// display name without "Allternit". Served from the web app's public folder.
enum OfficialCovers {
    private static let files: [String: String] = [
        "alabs course pipeline": "alabs",
        "codebase to course": "course",
        "excel": "excel",
        "powerpoint": "powerpoint",
        "word": "word",
        "chrome": "chrome",
        "word documents": "word",
        "powerpoint decks": "powerpoint",
        "agent orchestrator": "orchestrator",
        "steer parallel agent": "steer",
        "clone website": "clone",
        "mobile app design": "mobile",
    ]

    /// "Allternit for Excel" → "Excel".
    static func displayName(_ name: String) -> String {
        let trimmed = name.replacingOccurrences(of: #"^Allternit\s+(for\s+)?"#, with: "", options: [.regularExpression, .caseInsensitive])
            .trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty ? name : trimmed
    }

    /// The cover for a panel: dark art on the ink panel, light art on the white one.
    @MainActor static func art(for name: String, darkPanel: Bool) -> URL? {
        guard let file = files[displayName(name).lowercased()] else { return nil }
        return URL(string: "/official-covers/\(file)\(darkPanel ? "" : "-light").jpg", relativeTo: ArtifactLiveWebView.webOrigin)
    }
}

struct CustomizeSectionHero<Actions: View>: View {
    /// Section name for the lockup ("Skills" → A://TERNIT SKILLS).
    let lockup: String
    let items: [CustomizeItem]
    let eyebrow: (CustomizeItem) -> String
    @ViewBuilder let actions: (CustomizeItem, HeroPalette) -> Actions

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var at = 0

    var body: some View {
        if !items.isEmpty {
            let palette = HeroPalette(dark: colorScheme == .dark)
            let index = min(at, items.count - 1)
            let item = items[index]
            let art = OfficialCovers.art(for: item.name, darkPanel: colorScheme != .dark)
            ZStack(alignment: .topLeading) {
                if let art {
                    // Blurred copy behind everything, then the sharp art, faded in from the left.
                    CoverImage(url: art)
                        .blur(radius: 40)
                        .scaleEffect(1.35)
                        .opacity(0.5)
                    CoverImage(url: art)
                        .opacity(0.4)
                        .mask(LinearGradient(colors: [.clear, .black], startPoint: .leading, endPoint: UnitPoint(x: 0.48, y: 0.5)))
                    LinearGradient(stops: [.init(color: palette.bg, location: 0.22), .init(color: palette.bg.opacity(0), location: 0.7)],
                                   startPoint: .leading, endPoint: .trailing)
                }
                VStack(alignment: .leading, spacing: 24) {
                    LockupSweep(suffix: lockup.uppercased(), height: 12, ink: palette.fg)
                    copy(item, palette: palette)
                        .id(item.id)
                        .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: 8)), removal: .identity))
                    if items.count > 1 {
                        HStack(spacing: 6) {
                            ForEach(Array(items.enumerated()), id: \.element.id) { k, other in
                                let on = k == index
                                Button {
                                    withAnimation(.easeOut(duration: 0.3)) { at = k }
                                } label: {
                                    Capsule()
                                        .fill(on ? palette.fg : palette.dot)
                                        .frame(width: on ? 18 : 6, height: 6)
                                        .frame(minHeight: 24)
                                        .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel(other.name)
                                .accessibilityAddTraits(on ? .isSelected : [])
                            }
                        }
                    }
                }
                .padding(24)
            }
            .frame(maxWidth: .infinity, minHeight: 220, alignment: .topLeading)
            .background(palette.bg)
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .animation(reduceMotion ? nil : .easeOut(duration: 0.6), value: index)
            .task(id: items.count) {
                guard items.count > 1, !reduceMotion else { return }
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(4.5))
                    if Task.isCancelled { break }
                    at = (min(at, items.count - 1) + 1) % items.count
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Featured \(lockup.lowercased())")
        }
    }

    private func copy(_ item: CustomizeItem, palette: HeroPalette) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(eyebrow(item).uppercased())
                .font(.system(size: 11, design: .monospaced))
                .tracking(1.3)
                .foregroundColor(palette.sub)
            Text(OfficialCovers.displayName(item.name))
                .font(.system(size: 22, weight: .medium))
                .tracking(-0.3)
                .foregroundColor(palette.fg)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 4)
            if !item.description.isEmpty {
                Text(item.description)
                    .font(.system(size: 14))
                    .lineSpacing(3)
                    .lineLimit(2)
                    .foregroundColor(palette.sub)
                    .padding(.top, 6)
            }
            HStack(spacing: 8) { actions(item, palette) }
                .padding(.top, 16)
        }
        .frame(maxWidth: 440, alignment: .leading)
    }
}

/// Cover art filling the panel; nothing while it loads or when it fails.
private struct CoverImage: View {
    let url: URL

    var body: some View {
        AsyncImage(url: url) { phase in
            if let image = phase.image {
                image.resizable().scaledToFill()
            } else {
                Color.clear
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipped()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// Hero buttons: solid (Add) and outline (View), coloured for the panel.
struct HeroButtonLabel: View {
    let title: String
    var icon: String? = nil
    var solid = true
    var busy = false
    let palette: HeroPalette

    var body: some View {
        HStack(spacing: 6) {
            if busy {
                ProgressView().controlSize(.mini).tint(palette.bg)
            } else if let icon {
                PhIcon(icon, size: 13)
            }
            Text(title).font(.system(size: 13, weight: solid ? .medium : .regular))
        }
        .foregroundColor(solid ? palette.bg : palette.fg)
        .padding(.horizontal, 16)
        .frame(height: 36)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(solid ? palette.fg : .clear))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(solid ? .clear : palette.line, lineWidth: 1))
    }
}
