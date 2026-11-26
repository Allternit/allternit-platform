import SwiftUI
import UIKit

/// "Your apps" shelf on the Artifacts page (web src/views/library/parts/YourAppsRow.tsx;
/// look R2-C-phone.png): a dashed "Make an app" card first (starts App mode), then
/// one card per app from the apps shelf client. Artifact apps read
/// "Updated <time> · shared with a link / private" from the artifact list; vendor
/// rows keep their provenance. States: loading, error with Retry, empty (the Make
/// card alone). Signed out hides the row.
struct ArtifactAppsRow: View {
    @ObservedObject var model: AppsShelfModel
    /// updated date and visibility by artifact id, from the artifact list.
    let meta: [String: (updated: Date?, visibility: String)]
    let busy: Bool
    let onMakeApp: () -> Void
    let onOpen: (AppsShelfRow) -> Void

    private static let cardWidth: CGFloat = 232

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Your apps")
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextTertiary"))
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 10) {
                    makeCard
                    if !model.loaded && model.failure == nil {
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(Color("FillNeutral"))
                            .frame(width: Self.cardWidth, height: 64)
                            .accessibilityLabel("Loading your apps")
                    }
                    ForEach(model.yours) { row in appCard(row) }
                }
            }
            if let failure = model.failure, model.yours.isEmpty {
                HStack(spacing: 6) {
                    Text("Couldn’t load your apps.")
                        .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    Button("Retry") { Task { await model.refresh() } }
                        .font(.system(size: 12, weight: .medium)).foregroundColor(Color("TextPrimary")).underline()
                        .frame(minHeight: 44)
                }
                .accessibilityLabel("Couldn’t load your apps. \(failure)")
            }
        }
        .task { await model.refresh() }
    }

    private var makeCard: some View {
        Button(action: onMakeApp) {
            HStack(spacing: 12) {
                Image(systemName: "plus")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 34, height: 34)
                    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Color("FillNeutral")))
                VStack(alignment: .leading, spacing: 2) {
                    Text("Make an app").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    Text("Describe it, Gizzi builds it").font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .frame(width: Self.cardWidth, height: 64)
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [5, 4])))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(busy)
        .accessibilityLabel("Make an app")
    }

    private func appCard(_ row: AppsShelfRow) -> some View {
        Button { Haptics.light(); onOpen(row) } label: {
            HStack(spacing: 12) {
                Text(String(row.name.prefix(1)).uppercased())
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 34, height: 34)
                    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Color("FillNeutral")))
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name).font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                    Text(Self.subtitle(row, meta: meta)).font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12)
            .frame(width: Self.cardWidth, height: 64)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Open \(row.name)")
    }

    static func visibilityLabel(_ visibility: String) -> String {
        switch visibility {
        case "link": return "shared with a link"
        case "people": return "shared"
        case "org": return "shared with your org"
        default: return "private"
        }
    }

    /// "Updated 2 days ago · shared with a link"; vendor rows (or an app not yet in the list) keep their provenance.
    static func subtitle(_ row: AppsShelfRow, meta: [String: (updated: Date?, visibility: String)]) -> String {
        if row.kind == "artifact", let m = meta[row.slug] {
            let when = m.updated.map { "Updated \($0.formatted(.relative(presentation: .named)))" } ?? "Updated"
            return "\(when) · \(visibilityLabel(m.visibility))"
        }
        return [row.provenanceLabel, row.appTier.label].compactMap { $0 }.joined(separator: " · ")
    }
}
