import SwiftUI
import UIKit

// Desktop's own artwork and card shell, shared by MCP Apps, Customize and
// Teach: Phosphor glyphs and app logos (the same files, in
// Assets.xcassets/PhIcons and AppLogos) and PreviewCard.

// MARK: - Phosphor glyphs

/// One of Desktop's Phosphor icons. `name` is the asset without its `ph-`
/// prefix: "book-open", "record-fill", "puzzle-piece-thin".
struct PhIcon: View {
    let name: String
    var size: CGFloat = 16

    init(_ name: String, size: CGFloat = 16) {
        self.name = name
        self.size = size
    }

    var body: some View {
        Image("ph-\(name)")
            .renderingMode(.template)
            .resizable()
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

// MARK: - App logos

/// Finds an app's bundled logo, the way connector-logo.ts does on Desktop:
/// the app's own id, then its name, then its site. Assets are named
/// `applogo-<key>`, with the key lowercased and punctuation folded to `_`
/// (so `agent-mail` and `agent_mail` are one logo).
enum AppLogoCatalog {
    static func key(_ value: String) -> String {
        value.lowercased()
            .replacingOccurrences(of: "[^a-z0-9]+", with: "_", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: "_"))
    }

    /// Hosts shared by many vendors say nothing about the app.
    private static let platformLabels: Set<String> = ["googleapis", "githubcopilot", "amazonaws", "azure", "cloudfront"]

    /// The registrable label of a site: `mcp.airtable.com` → `airtable`.
    static func siteHost(_ url: String?) -> String? {
        guard let url, let host = URL(string: url)?.host?.lowercased() else { return nil }
        let parts = host.split(separator: ".").map(String.init)
        guard parts.count >= 2 else { return nil }
        let second = parts[parts.count - 2]
        return ["co", "com", "net", "org"].contains(second) && parts.count >= 3 ? parts[parts.count - 3] : second
    }

    /// `siteHost` when the site names the app; shared hosts don't.
    static func siteLabel(_ url: String?) -> String? {
        guard let label = siteHost(url), !platformLabels.contains(label) else { return nil }
        return label
    }

    static func candidates(id: String?, name: String, url: String?) -> [String] {
        var out: [String] = []
        func push(_ raw: String?) {
            guard let raw else { return }
            let k = key(raw)
            if !k.isEmpty && !out.contains(k) { out.append(k) }
        }
        push(id?.replacingOccurrences(of: #"^(connector:|mcp:|url:)"#, with: "", options: .regularExpression))
        push(name)
        push(siteLabel(url))
        return out
    }

    static func image(id: String?, name: String, url: String?) -> UIImage? {
        for candidate in candidates(id: id, name: name, url: url) {
            if let image = UIImage(named: "applogo-\(candidate)") { return image }
        }
        return nil
    }

    /// The site's own favicon, Desktop's next source.
    static func faviconURL(_ url: String?, size: CGFloat) -> URL? {
        guard let url, let host = URL(string: url)?.host else { return nil }
        let domain = host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
        var components = URLComponents(string: "https://www.google.com/s2/favicons")
        components?.queryItems = [
            URLQueryItem(name: "domain", value: domain),
            URLQueryItem(name: "sz", value: String(max(64, Int(size * 2)))),
        ]
        return components?.url
    }

    static func initials(_ name: String) -> String {
        let words = name.split(whereSeparator: { $0.isWhitespace || $0 == "." || $0 == "_" || $0 == "-" })
        let letters = words.prefix(2).compactMap(\.first).map { String($0).uppercased() }.joined()
        return letters.isEmpty ? "?" : letters
    }
}

/// ConnectorLogo: the brand mark on a white tile, 24 % corners, in both
/// themes. Bundled logo, else the site's favicon, else the app's initials.
struct AppLogoView: View {
    var id: String? = nil
    let name: String
    var url: String? = nil
    var size: CGFloat = 40

    @State private var favicon: UIImage? = nil

    private var bundled: UIImage? { AppLogoCatalog.image(id: id, name: name, url: url) }

    var body: some View {
        ZStack {
            if let image = bundled ?? favicon {
                Image(uiImage: image)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
                    .frame(width: (size * 0.58).rounded(), height: (size * 0.58).rounded())
            } else {
                Text(AppLogoCatalog.initials(name))
                    .font(.system(size: (size * 0.34).rounded(), weight: .semibold))
                    .foregroundColor(Color(hex: "#6B6B6B"))
            }
        }
        .frame(width: size, height: size)
        .background(Color.white)
        .clipShape(RoundedRectangle(cornerRadius: (size * 0.24).rounded(), style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: (size * 0.24).rounded(), style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityHidden(true)
        .task(id: url) { await loadFavicon() }
    }

    private func loadFavicon() async {
        guard bundled == nil, favicon == nil, let source = AppLogoCatalog.faviconURL(url, size: size) else { return }
        guard let (data, _) = try? await URLSession.shared.data(from: source),
              let image = UIImage(data: data) else { return }
        // The favicon service answers unknown sites with a 16 px globe.
        if image.size.width * image.scale > 16 { favicon = image }
    }
}

// MARK: - Marks

/// `--verified`: the blue seal of a vendor's own server.
extension Color {
    static let verifiedBlue = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0x4D / 255, green: 0xB2 / 255, blue: 0xFF / 255, alpha: 1)
            : UIColor(red: 0x1D / 255, green: 0x9B / 255, blue: 0xF0 / 255, alpha: 1)
    })
}

/// VerifiedMark: Desktop's seal path, on its 24 pt grid.
struct VerifiedMark: View {
    var size: CGFloat = 14

    private struct Seal: Shape {
        func path(in rect: CGRect) -> Path {
            let scale = rect.width / 24
            let steps: [(CGFloat, CGFloat)] = [
                (2.6, 1.9), (3.2, -0.1), (1, 3), (2.6, 1.9), (-1, 3), (1, 3), (-2.6, 1.9), (-1, 3), (-3.2, -0.1),
            ]
            let back: [(CGFloat, CGFloat)] = [
                (-2.6, -1.9), (-3.2, 0.1), (-1, -3), (-2.6, -1.9), (1, -3), (-1, -3), (2.6, -1.9), (1, -3), (3.2, 0.1),
            ]
            var path = Path()
            var x: CGFloat = 12, y: CGFloat = 1.5
            path.move(to: CGPoint(x: x * scale, y: y * scale))
            for (dx, dy) in steps {
                x += dx; y += dy
                path.addLine(to: CGPoint(x: x * scale, y: y * scale))
            }
            x = 12; y = 22.5
            path.addLine(to: CGPoint(x: x * scale, y: y * scale))
            for (dx, dy) in back {
                x += dx; y += dy
                path.addLine(to: CGPoint(x: x * scale, y: y * scale))
            }
            path.closeSubpath()
            return path
        }
    }

    private struct Tick: Shape {
        func path(in rect: CGRect) -> Path {
            let scale = rect.width / 24
            var path = Path()
            path.move(to: CGPoint(x: 7.8 * scale, y: 12.3 * scale))
            path.addLine(to: CGPoint(x: 10.8 * scale, y: 15.3 * scale))
            path.addLine(to: CGPoint(x: 16.4 * scale, y: 9.3 * scale))
            return path
        }
    }

    var body: some View {
        ZStack {
            Seal().fill(Color.verifiedBlue)
            Tick().stroke(Color.white, style: StrokeStyle(lineWidth: size * 0.1, lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .accessibilityElement()
        .accessibilityLabel("Official")
    }
}

/// PolishStatus on a card: ok · attention · idle · error.
enum PolishStatus {
    case ok, attention, idle, error

    var color: Color {
        switch self {
        case .ok: return Color(hex: "#10B981")
        case .attention: return Color(hex: "#F59E0B")
        case .idle: return Color("TextTertiary")
        case .error: return Theme.statusError
        }
    }
}

// MARK: - PreviewCard

/// PreviewCard (components/ui/polish): an inset logo tile, status dot + name,
/// two lines of description, hairline, one footer fact, then any controls.
struct PolishCard<Logo: View, Action: View, Extras: View, Controls: View>: View {
    let name: String
    var description: String? = nil
    var status: PolishStatus? = nil
    var statusLabel: String? = nil
    var footer: String? = nil
    var footerIcon: String? = nil
    var tileHeight: CGFloat = 88
    let onOpen: () -> Void
    @ViewBuilder let logo: () -> Logo
    @ViewBuilder let action: () -> Action
    @ViewBuilder let extras: () -> Extras
    @ViewBuilder let controls: () -> Controls

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onOpen) {
                VStack(alignment: .leading, spacing: 0) {
                    logo()
                        .frame(maxWidth: .infinity)
                        .frame(height: tileHeight)
                        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")))
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(spacing: 8) {
                            if let status {
                                Circle().fill(status.color).frame(width: 8, height: 8)
                                    .accessibilityLabel(statusLabel ?? "")
                            }
                            Text(name)
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            extras()
                        }
                        if let description, !description.isEmpty {
                            Text(description)
                                .font(.system(size: 12.5))
                                .lineSpacing(1.5)
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(2)
                                .multilineTextAlignment(.leading)
                                .fixedSize(horizontal: false, vertical: true)
                                .padding(.leading, status == nil ? 0 : 16)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.top, 12)
                    .padding(.bottom, 10)
                    Spacer(minLength: 0)
                    if let footer, !footer.isEmpty {
                        Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                        HStack(spacing: 8) {
                            if let footerIcon { PhIcon(footerIcon, size: 13).opacity(0.7) }
                            Text(footer).lineLimit(1)
                        }
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 10)
                        .padding(.top, 8)
                        .padding(.bottom, 6)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(PolishPressStyle())
            .accessibilityElement(children: .combine)
            controls()
        }
        .padding(4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.04), radius: 6, y: 2)
        .overlay(alignment: .topTrailing) {
            action().padding(12)
        }
    }
}

/// Press feedback on a whole card (`active:scale-[0.99]`).
private struct PolishPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.99 : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
    }
}

extension PolishCard where Extras == EmptyView, Controls == EmptyView {
    init(name: String, description: String? = nil, status: PolishStatus? = nil, statusLabel: String? = nil,
         footer: String? = nil, footerIcon: String? = nil, tileHeight: CGFloat = 88, onOpen: @escaping () -> Void,
         @ViewBuilder logo: @escaping () -> Logo, @ViewBuilder action: @escaping () -> Action) {
        self.init(name: name, description: description, status: status, statusLabel: statusLabel, footer: footer,
                  footerIcon: footerIcon, tileHeight: tileHeight, onOpen: onOpen, logo: logo, action: action,
                  extras: { EmptyView() }, controls: { EmptyView() })
    }
}

/// The card's round top-right action: + to add, a spinner while adding, ✓ once added.
struct PolishCardAddButton: View {
    let added: Bool
    var pending: Bool = false
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: { if !added && !pending { action() } }) {
            Group {
                if pending {
                    ProgressView().controlSize(.mini)
                } else {
                    PhIcon(added ? "check" : "plus", size: 14)
                }
            }
            .foregroundColor(Color("TextSecondary"))
            .frame(width: 28, height: 28)
            .background(Circle().fill(Color("BgPanel")))
            .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
            .frame(width: 44, height: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(-8)
        .accessibilityLabel(label)
    }
}

/// Two cards to a row, as PreviewCardGrid `fill` lays out on a phone.
struct PolishCardGrid<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12, content: content)
    }
}

/// PreviewCardSkeleton: the same shape as the real cards, so nothing jumps.
struct PolishCardSkeletons: View {
    var count: Int = 6
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        PolishCardGrid {
            ForEach(0..<count, id: \.self) { _ in
                VStack(alignment: .leading, spacing: 8) {
                    RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")).frame(height: 88)
                    RoundedRectangle(cornerRadius: 4).fill(Color("FillNeutral")).frame(width: 90, height: 12).padding(.horizontal, 10).padding(.top, 6)
                    RoundedRectangle(cornerRadius: 4).fill(Color("FillNeutral")).frame(height: 10).padding(.horizontal, 10)
                    RoundedRectangle(cornerRadius: 4).fill(Color("FillNeutral")).frame(width: 110, height: 10).padding(.horizontal, 10).padding(.bottom, 14)
                }
                .padding(4)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
        }
        .opacity(dim ? 0.55 : 1)
        .accessibilityElement()
        .accessibilityLabel("Loading")
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { dim = true }
        }
    }
}
