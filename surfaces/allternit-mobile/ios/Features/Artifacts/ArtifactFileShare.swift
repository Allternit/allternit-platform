import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Files the artifact viewers hand to the system share sheet (Save to
/// Files, Save Image/Video to Photos, AirDrop, Mail…). Each file is written
/// to its own folder under the app's temp directory with the name the web
/// gave it, so the share sheet shows the real file name.
struct SharedArtifactFile: Identifiable, Equatable {
    let id = UUID()
    let url: URL
}

enum ArtifactFileShare {
    /// Downloads larger than this are refused rather than held in memory.
    static let maxBytes = 512 * 1024 * 1024

    enum ShareError: LocalizedError {
        case tooLarge
        case empty

        var errorDescription: String? {
            switch self {
            case .tooLarge: return "This file is too large to save on the phone. Download it on Desktop or the web."
            case .empty: return "The download was empty."
            }
        }
    }

    /// A safe file name: no path separators, a sensible extension from the
    /// MIME type when the name has none.
    static func fileName(_ suggested: String?, mimeType: String?, fallback: String = "Download") -> String {
        var name = (suggested ?? "")
            .components(separatedBy: CharacterSet(charactersIn: "/\\:\0"))
            .joined(separator: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        while name.hasPrefix(".") { name.removeFirst() }
        if name.isEmpty { name = fallback }
        if name.count > 120 { name = String(name.prefix(120)) }
        if (name as NSString).pathExtension.isEmpty, let mimeType,
           let ext = UTType(mimeType: mimeType.components(separatedBy: ";").first ?? mimeType)?.preferredFilenameExtension {
            name += ".\(ext)"
        }
        return name
    }

    /// A fresh temp folder per file, so two downloads with one name don't clash.
    static func destination(named name: String) throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("ArtifactDownloads", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder.appendingPathComponent(name)
    }

    static func write(_ data: Data, suggestedName: String?, mimeType: String?) throws -> URL {
        guard !data.isEmpty else { throw ShareError.empty }
        guard data.count <= maxBytes else { throw ShareError.tooLarge }
        let url = try destination(named: fileName(suggestedName, mimeType: mimeType))
        try data.write(to: url, options: .atomic)
        return url
    }
}

/// `UIActivityViewController` for one or more files.
struct ArtifactShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

/// The "Preparing download…" pill shown while a file is read or exported.
struct ArtifactExportProgressPill: View {
    let label: String

    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
            Text(label).font(.subheadline.weight(.medium))
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Capsule().fill(Color("BgPanel")))
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.08), radius: 8, y: 2)
        .accessibilityElement(children: .combine)
    }
}

// MARK: - Share sheet (phone calm I12)

/// One Export-tab row: saving a file, a PDF from a template, copying text.
/// The viewer runs it (a file share needs the panel closed first).
struct ArtifactExportAction: Identifiable {
    let id: String
    let title: String
    let systemImage: String
    /// True when the action opens another sheet, so the panel closes first.
    var closesPanel = true
}

/// The viewer's share sheet (web ShareDialog + the phone mock): Share |
/// Export. Share invites people by email with a level, sets General access
/// and copies the private link; only the owner can change sharing
/// (`/api/v2/artifacts/:id/sharing` is owner-only). Export lists the file
/// actions the viewer offers (the existing UIActivity share sheet).
struct ArtifactSharePanel: View {
    enum Tab: String, CaseIterable, Identifiable {
        case share = "Share"
        case export = "Export"
        var id: String { rawValue }
    }

    let artifactId: String
    let title: String
    let kind: String
    let myAccess: String
    let ownerName: String?
    /// The record's visibility when the panel opens.
    let visibility: String
    let exports: [ArtifactExportAction]
    var initialTab: Tab = .share
    let onExport: (ArtifactExportAction) -> Void

    @Environment(\.dismiss) private var dismiss
    @ReducedMotion private var reducedMotion
    @State private var tab: Tab = .share
    @State private var sharing: ArtifactV2Sharing?
    @State private var loadError: String?
    @State private var saveError: String?
    @State private var saving = false
    @State private var invite = ""
    @State private var inviteLevel = "view"
    @State private var copied = false

    private var isOwner: Bool { ArtifactAccessLevel(myAccess: myAccess) == .owner }
    private var currentVisibility: String { sharing?.visibility ?? visibility }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text(title.isEmpty ? "Artifact" : title)
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer()
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 32, height: 32)
                        .background(Circle().fill(Color("FillNeutral")))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
            }

            Picker("Share or export", selection: Binding(get: { tab }, set: { next in
                Haptics.selection()
                tab = next
            })) {
                ForEach(Tab.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)

            ScrollView {
                Group {
                    switch tab {
                    case .share: shareTab
                    case .export: exportTab
                    }
                }
                .transition(.opacity)
                .animation(Motion.pick(Motion.deck, reduced: reducedMotion), value: tab)
            }
        }
        .padding(16)
        .background(Color("BgPrimary").ignoresSafeArea())
        .task {
            tab = initialTab
            await loadSharing()
        }
    }

    // MARK: Share

    @ViewBuilder
    private var shareTab: some View {
        VStack(alignment: .leading, spacing: 14) {
            if isOwner {
                if let sharing {
                    if kind != "doc" { inviteRow(policy: sharing.policy) }
                    peopleList(sharing)
                    generalAccess(sharing)
                } else if let loadError {
                    errorText(loadError)
                    Button("Retry") { Task { await loadSharing() } }
                        .font(.subheadline.weight(.semibold))
                } else {
                    ProgressView().frame(maxWidth: .infinity)
                }
            } else {
                HStack(spacing: 8) {
                    Image(systemName: visibilityIcon(visibility))
                    Text(ArtifactShareRules.visibilityLabel(visibility))
                }
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                Text("Only the owner can change who has access.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            if let saveError { errorText(saveError) }
            copyLinkButton
        }
    }

    private func inviteRow(policy: ArtifactV2SharingPolicy) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                TextField("Invite people by email", text: $invite)
                    .keyboardType(.emailAddress)
                    .textContentType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.send)
                    .onSubmit { Task { await addInvite() } }
                    .disabled(saving || !policy.outsideInvitesAllowed)
                Menu {
                    Picker("Access for new person", selection: $inviteLevel) {
                        ForEach(ArtifactShareRules.levels(kind: kind), id: \.self) { level in
                            Text(ArtifactShareRules.levelLabel(level)).tag(level)
                        }
                    }
                } label: {
                    HStack(spacing: 3) {
                        Text(ArtifactShareRules.levelLabel(ArtifactShareRules.clampLevel(kind: kind, level: inviteLevel)))
                        Image(systemName: "chevron.down").font(.caption2)
                    }
                    .font(.footnote.weight(.medium))
                    .foregroundColor(Color("TextPrimary"))
                }
                .accessibilityLabel("Access for new person")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
            if !policy.outsideInvitesAllowed {
                Text(policy.reason ?? "Your org doesn’t allow invites to people outside it.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else if !invite.trimmingCharacters(in: .whitespaces).isEmpty {
                Button(saving ? "Adding…" : "Add") { Task { await addInvite() } }
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .disabled(saving)
            }
        }
    }

    private func peopleList(_ sharing: ArtifactV2Sharing) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            personRow(name: ownerName ?? "You", detail: nil) {
                Text("Owner").font(.footnote).foregroundColor(Color("TextSecondary"))
            }
            if sharing.visibility != "private" {
                ForEach(sharing.shares, id: \.self) { person in
                    personRow(name: person.display ?? person.principalId, detail: inviteDetail(person)) {
                        HStack(spacing: 10) {
                            Menu {
                                ForEach(ArtifactShareRules.levels(kind: kind), id: \.self) { level in
                                    Button {
                                        Task { await setLevel(person, level: level) }
                                    } label: {
                                        if level == person.level {
                                            Label(ArtifactShareRules.levelLabel(level), systemImage: "checkmark")
                                        } else {
                                            Text(ArtifactShareRules.levelLabel(level))
                                        }
                                    }
                                }
                            } label: {
                                HStack(spacing: 3) {
                                    Text(ArtifactShareRules.levelLabel(person.level))
                                    Image(systemName: "chevron.down").font(.caption2)
                                }
                                .font(.footnote)
                                .foregroundColor(Color("TextPrimary"))
                            }
                            .accessibilityLabel("Access for \(person.display ?? person.principalId)")
                            Button { Task { await remove(person) } } label: {
                                Image(systemName: "xmark").font(.caption.weight(.semibold))
                            }
                            .buttonStyle(.plain)
                            .foregroundColor(Color("TextSecondary"))
                            .accessibilityLabel("Remove \(person.display ?? person.principalId)")
                        }
                        .disabled(saving)
                    }
                }
            }
        }
    }

    private func personRow<Trailing: View>(name: String, detail: String?, @ViewBuilder trailing: () -> Trailing) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "person.crop.circle")
                .font(.system(size: 22))
                .foregroundColor(Color("TextSecondary"))
            VStack(alignment: .leading, spacing: 1) {
                Text(name).font(.subheadline).foregroundColor(Color("TextPrimary")).lineLimit(1)
                if let detail { Text(detail).font(.caption).foregroundColor(Color("TextSecondary")) }
            }
            Spacer(minLength: 8)
            trailing()
        }
        .padding(.vertical, 8)
    }

    private func generalAccess(_ sharing: ArtifactV2Sharing) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("General access")
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: visibilityIcon(sharing.visibility))
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 2) {
                    Menu {
                        ForEach(ArtifactShareRules.visibilities, id: \.self) { option in
                            Button {
                                Task { await save(visibility: option, shares: sharing.shares.map(\.input)) }
                            } label: {
                                if option == sharing.visibility {
                                    Label(ArtifactShareRules.visibilityLabel(option), systemImage: "checkmark")
                                } else {
                                    Text(ArtifactShareRules.visibilityLabel(option))
                                }
                            }
                            .disabled(ArtifactShareRules.visibilityBlockReason(option, kind: kind, policy: sharing.policy) != nil)
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text(ArtifactShareRules.visibilityLabel(sharing.visibility))
                            Image(systemName: "chevron.down").font(.caption2)
                        }
                        .font(.subheadline.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                    }
                    .disabled(saving)
                    .accessibilityLabel("Who can open it")
                    Text(ArtifactShareRules.visibilityHint(sharing.visibility))
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                    if let reason = ArtifactShareRules.visibilityBlockReason("link", kind: kind, policy: sharing.policy) {
                        Text(reason).font(.footnote).foregroundColor(Color("TextSecondary"))
                    }
                }
            }
        }
        .padding(.top, 4)
    }

    private var copyLinkButton: some View {
        let link = ArtifactShareRules.copyLink(artifactId: artifactId, sharing: sharing, visibility: currentVisibility,
                                               origin: ArtifactLiveWebView.webOrigin)
        return Button {
            UIPasteboard.general.url = link.url
            Haptics.success()
            copied = true
            Task {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                copied = false
            }
        } label: {
            Label(copied ? "Copied" : link.label, systemImage: copied ? "checkmark" : "doc.on.doc")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 13)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
        }
        .buttonStyle(.plain)
        .padding(.top, 10)
    }

    // MARK: Export

    @ViewBuilder
    private var exportTab: some View {
        VStack(alignment: .leading, spacing: 0) {
            if exports.isEmpty {
                Text("Nothing to export from the phone for this one. Open it on Desktop or the web to export.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.vertical, 8)
            }
            ForEach(exports) { action in
                Button {
                    Haptics.light()
                    if action.closesPanel { dismiss() }
                    onExport(action)
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: action.systemImage).frame(width: 22)
                        Text(action.title)
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption).foregroundColor(Color("TextSecondary"))
                    }
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.vertical, 12)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: Helpers

    private func errorText(_ text: String) -> some View {
        Text(text)
            .font(.footnote)
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
    }

    private func visibilityIcon(_ visibility: String) -> String {
        switch visibility {
        case "people": return "person.2"
        case "org": return "person.3"
        case "link": return "globe"
        default: return "lock"
        }
    }

    private func inviteDetail(_ person: ArtifactV2Share) -> String? {
        guard person.acceptedAt == nil, let raw = person.expiresAt, let date = ArtifactV2Dates.parse(raw) else { return nil }
        return "Invite expires \(date.formatted(date: .abbreviated, time: .omitted))"
    }

    // MARK: Actions

    private func loadSharing() async {
        guard isOwner else { return }
        do {
            sharing = try await ArtifactsV2Client.shared.sharing(id: artifactId)
            loadError = nil
        } catch is CancellationError {
            return
        } catch {
            loadError = plainErrorText(error)
        }
    }

    private func save(visibility: String, shares: [ArtifactV2ShareInput]) async {
        saving = true
        defer { saving = false }
        do {
            sharing = try await ArtifactsV2Client.shared.setSharing(id: artifactId, visibility: visibility, shares: shares)
            saveError = nil
            Haptics.selection()
        } catch {
            saveError = plainErrorText(error)
            Haptics.warning()
        }
    }

    private func addInvite() async {
        guard let sharing, !saving else { return }
        switch ArtifactShareRules.checkInvite(invite, kind: kind, existing: sharing.shares) {
        case .refused(let reason):
            saveError = reason
        case .ok(let email):
            let added = ArtifactV2ShareInput(principalType: "email", principalId: email,
                                             level: ArtifactShareRules.clampLevel(kind: kind, level: inviteLevel))
            await save(visibility: ArtifactShareRules.visibilityAfterInvite(sharing.visibility),
                       shares: sharing.shares.map(\.input) + [added])
            if saveError == nil { invite = "" }
        }
    }

    private func setLevel(_ person: ArtifactV2Share, level: String) async {
        guard let sharing, level != person.level else { return }
        let shares = sharing.shares.map { $0 == person
            ? ArtifactV2ShareInput(principalType: $0.principalType, principalId: $0.principalId, level: level)
            : $0.input }
        await save(visibility: sharing.visibility, shares: shares)
    }

    private func remove(_ person: ArtifactV2Share) async {
        guard let sharing else { return }
        await save(visibility: sharing.visibility, shares: sharing.shares.filter { $0 != person }.map(\.input))
    }
}
