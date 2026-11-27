import SwiftUI

/// The kinds "+ New" and the empty library offer: the deck's mode list, in the
/// registry's order, artifact modes only (Agent and Swarm are chat modes).
/// Approved look: R2-C-phone.png / R2-C-empty.png.
enum ArtifactNewModes {
    static func list(allowed: (ArtifactStudioKind) -> Bool) -> [ChatDeckMode] {
        ChatDeckMode.chat.filter { mode in
            guard mode.entry.artifact else { return false }
            if let kind = DeckLaunch.kind(for: mode.entry) { return allowed(kind) }
            return true // PDF opens the file picker
        }
    }
}

/// "+ New" sheet: one row per kind, then the uploads.
struct ArtifactNewModeSheet: View {
    let modes: [ChatDeckMode]
    let busy: Bool
    let onPick: (ChatDeckMode) -> Void
    let onUpload: (Upload) -> Void
    let onOffice: () -> Void

    enum Upload { case image, video }

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            Capsule().fill(Color("BorderStrong")).frame(width: 36, height: 5)
                .padding(.top, 8).padding(.bottom, 12)
            Text("New artifact")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.bottom, 8)
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(modes) { mode in
                        Button { dismiss(); onPick(mode) } label: { row(mode) }
                            .buttonStyle(.plain)
                            .disabled(busy)
                            .accessibilityLabel("New \(mode.label)")
                    }
                    Divider().padding(.vertical, 8)
                    plainRow("Upload an image", "photo") { dismiss(); onUpload(.image) }
                    plainRow("Upload a video", "film") { dismiss(); onUpload(.video) }
                    plainRow("Allternit Office", "doc.text") { dismiss(); onOffice() }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 20)
            }
        }
        .background(Color("BgPrimary"))
        .presentationDetents([.medium, .large])
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
    }

    private func row(_ mode: ChatDeckMode) -> some View {
        HStack(spacing: 12) {
            Image(mode.icon).resizable().frame(width: 28, height: 28)
            Text(mode.label)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            if let detail = mode.detail {
                Text(detail).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            }
            Spacer(minLength: 0)
        }
        .frame(minHeight: 48)
        .contentShape(Rectangle())
    }

    private func plainRow(_ title: String, _ symbol: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: symbol)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 28, height: 28)
                Text(title).font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// The empty library: the kinds, tap one to start it (R2-C-empty).
struct ArtifactEmptyKinds: View {
    let modes: [ChatDeckMode]
    let busy: Bool
    let onPick: (ChatDeckMode) -> Void

    private let columns = [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text("No artifacts yet")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Pick a kind to start, or ask for one in any chat.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
            }
            LazyVGrid(columns: columns, spacing: 14) {
                ForEach(modes) { mode in
                    Button { onPick(mode) } label: {
                        VStack(alignment: .leading, spacing: 8) {
                            // PDF has no studio kind; it shows the page scene.
                            ArtifactMotionTile(kind: DeckLaunch.kind(for: mode.entry) ?? .docs)
                            Text(mode.label)
                                .font(.system(size: 15, weight: .medium))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                                .padding(.horizontal, 4)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(busy)
                    .accessibilityLabel("New \(mode.label)")
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 8)
    }
}
