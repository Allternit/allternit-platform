import SwiftUI
import UIKit

/// The Apps shelf (ACI › MCP Apps › Apps, behind `apps.shelf`): two sections,
/// "Made by you / your org" and "From a connected server", then the person's
/// share links. Rows install (consent sheet first), pin, and open.
struct AppsShelfView: View {
    @StateObject private var model = AppsShelfModel()
    @State private var opened: OpenedApp?
    @State private var stopping: AppsShareLink?

    struct OpenedApp: Identifiable {
        let url: URL
        let name: String
        var id: String { url.absoluteString }
    }

    var body: some View {
        ScrollView {
            content
                .padding(.horizontal, 16)
                .padding(.vertical, 16)
        }
        .background(Color("BgPrimary"))
        .refreshable { await model.refresh() }
        .task { await model.refresh() }
        .sheet(item: $model.consent) { request in
            AppsConsentSheet(request: request,
                             onAllow: { Task { await model.confirmInstall(request) } },
                             onCancel: { model.consent = nil })
                .presentationDetents([.medium])
        }
        .fullScreenCover(item: $opened) { app in
            AppsOpenedView(app: app) { opened = nil }
        }
        .confirmationDialog("Stop this link?", isPresented: Binding(get: { stopping != nil }, set: { if !$0 { stopping = nil } }),
                            titleVisibility: .visible, presenting: stopping) { link in
            Button("Stop link", role: .destructive) { Task { await model.revoke(link) } }
            Button("Keep it", role: .cancel) {}
        } message: { _ in
            Text("Anyone with the link loses access. You can make a new one later.")
        }
    }

    @ViewBuilder
    private var content: some View {
        if !model.loaded {
            VStack(spacing: 10) {
                ForEach(0..<4, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("FillNeutral")).frame(height: 68)
                }
            }
            .accessibilityLabel("Loading apps")
        } else if let failure = model.failure, model.yours.isEmpty, model.servers.isEmpty {
            VStack(spacing: 12) {
                EmptyBox(icon: "exclamationmark.triangle", title: "Couldn’t load your apps", line: failure)
                Button("Try again") { Task { await model.refresh() } }
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 44)
            }
        } else {
            VStack(alignment: .leading, spacing: 24) {
                if let notice = model.notice {
                    HStack(spacing: 12) {
                        Text(notice).font(.system(size: 13)).foregroundColor(Color("TextPrimary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button("Dismiss") { model.notice = nil }
                            .font(.system(size: 13)).foregroundColor(Color("TextSecondary")).underline()
                            .frame(minHeight: 44)
                    }
                    .padding(12)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                }
                section("Made by you / your org", rows: model.yours, art: .apps,
                        emptyTitle: "No apps yet",
                        empty: "Apps you make, or your org shares, show up here.")
                section("From a connected server", rows: model.servers, art: .network,
                        emptyTitle: "No server apps",
                        empty: "Connect an MCP server and its apps show up here.")
                shareList
            }
        }
    }

    private func section(_ title: String, rows: [AppsShelfRow], art: EmptyArtKind, emptyTitle: String, empty: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextSecondary"))
                .accessibilityAddTraits(.isHeader)
            if rows.isEmpty {
                EmptyRowCard(art: art, title: emptyTitle, hint: empty)
            } else {
                ForEach(rows) { row in
                    AppsShelfRowView(row: row, busy: model.busy.contains(row.id),
                                     onOpen: { open(row) },
                                     onInstall: { model.requestInstall(row) },
                                     onPin: { Task { await model.togglePin(row) } },
                                     onRemove: { Task { await model.uninstall(row) } })
                }
            }
        }
    }

    private var shareList: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Your links").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextSecondary"))
                .accessibilityAddTraits(.isHeader)
            if let failure = model.linksFailure, model.links.isEmpty {
                Text("Couldn’t load your links. \(failure)").font(.system(size: 14)).foregroundColor(Color("TextTertiary"))
            } else if model.links.isEmpty {
                EmptyRowCard(art: .flow, title: "No links yet",
                             hint: "Share an app from Desktop and its link shows up here.")
            } else {
                ForEach(model.links) { link in
                    AppsShareLinkRow(link: link, busy: model.busy.contains(link.id),
                                     onPause: { Task { await model.togglePause(link) } },
                                     onStop: { stopping = link })
                }
            }
        }
    }

    private func open(_ row: AppsShelfRow) {
        Task {
            let link = await model.openLink(for: row)
            guard let url = AppsShelfLinks.openURL(for: link) else {
                model.notice = "\(row.name) has no link to open yet."
                return
            }
            opened = OpenedApp(url: url, name: row.name)
        }
    }
}

// MARK: - Rows

private struct AppsShelfRowView: View {
    let row: AppsShelfRow
    let busy: Bool
    let onOpen: () -> Void
    let onInstall: () -> Void
    let onPin: () -> Void
    let onRemove: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                icon
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.name).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                    Text([row.provenanceLabel, row.appTier.label].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                Spacer(minLength: 0)
                if busy { ProgressView() }
            }
            HStack(spacing: 8) {
                if row.installed {
                    pill("Open", filled: true, action: onOpen)
                    pill(row.pinned ? "Unpin" : "Pin", filled: false, action: onPin)
                    pill("Remove", filled: false, action: onRemove)
                } else {
                    pill("Install", filled: true, action: onInstall)
                    if row.link != nil { pill("Open", filled: false, action: onOpen) }
                }
            }
            .disabled(busy)
        }
        .padding(12)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .contain)
    }

    private var icon: some View {
        let glyph = (row.icon?.isEmpty == false ? row.icon : nil) ?? String(row.name.prefix(1)).uppercased()
        return Text(glyph.count <= 2 ? glyph : String(row.name.prefix(1)).uppercased())
            .font(.system(size: 18, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 40, height: 40)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .accessibilityHidden(true)
    }

    private func pill(_ title: String, filled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(filled ? Color("OnAccent") : Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(filled ? Color("AccentPrimary") : Color("BgPanel"))
                .clipShape(Capsule())
                .overlay(Capsule().stroke(filled ? Color.clear : Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title) \(row.name)")
    }
}

private struct AppsShareLinkRow: View {
    let link: AppsShareLink
    let busy: Bool
    let onPause: () -> Void
    let onStop: () -> Void
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(link.title).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                    Text(status).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    if let url = link.url {
                        Text(url).font(.system(size: 12)).foregroundColor(Color("TextTertiary")).lineLimit(1).truncationMode(.middle)
                    }
                }
                Spacer(minLength: 0)
                if busy { ProgressView() }
            }
            HStack(spacing: 8) {
                if let url = link.url {
                    button(copied ? "Copied" : "Copy link") {
                        UIPasteboard.general.string = url
                        copied = true
                        Haptics.light()
                        Task { try? await Task.sleep(nanoseconds: 1_500_000_000); copied = false }
                    }
                }
                button(link.paused ? "Resume" : "Pause", action: onPause)
                button("Stop", action: onStop)
            }
            .disabled(busy)
        }
        .padding(12)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private var status: String {
        let kind = link.isLive ? "Live from your computer" : "Hosted"
        return link.paused ? "\(kind) · Paused" : kind
    }

    private func button(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14).frame(minHeight: 44)
                .background(Color("BgPanel")).clipShape(Capsule())
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Consent sheet

/// Says what the app may do before it installs. Tier names match the web sheet.
struct AppsConsentSheet: View {
    let request: AppsShelfModel.ConsentRequest
    let onAllow: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(request.raised ? "\(request.row.name) wants more access" : "Install \(request.row.name)?")
                .font(.system(size: 20, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    ForEach(AppsTier.allCases, id: \.self) { tier in
                        Text(tier.label)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(tier == request.tier ? Color("OnAccent") : Color("TextSecondary"))
                            .padding(.horizontal, 12).padding(.vertical, 6)
                            .background(tier == request.tier ? Color("AccentPrimary") : Color("FillNeutral"))
                            .clipShape(Capsule())
                    }
                }
                Text(request.tier.detail).font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            }
            if !request.added.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("New access").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextSecondary"))
                    ForEach(request.added, id: \.self) { name in
                        Text("• \(name)").font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                    }
                }
            }
            Spacer(minLength: 0)
            HStack(spacing: 10) {
                Button(action: onCancel) {
                    Text("Cancel").font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .background(Color("FillNeutral")).clipShape(Capsule())
                }
                Button(action: onAllow) {
                    Text(request.raised ? "Allow" : "Install").font(.system(size: 15, weight: .semibold)).foregroundColor(Color("OnAccent"))
                        .frame(maxWidth: .infinity, minHeight: 48)
                        .background(Color("AccentPrimary")).clipShape(Capsule())
                }
            }
        }
        .padding(20)
        .background(Color("BgPrimary"))
    }
}

// MARK: - Opened app

/// An app's hosted page, full screen, inside the app (no Safari hand-off).
/// Also used by the Artifacts library when an app card is opened.
struct AppsOpenedView: View {
    let app: AppsShelfView.OpenedApp
    let onClose: () -> Void
    @State private var failed = false
    @State private var reload = 0

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(app.name).font(.system(size: 16, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                Spacer()
                Button("Done", action: onClose)
                    .font(.system(size: 16, weight: .medium)).foregroundColor(Color("TextPrimary")).frame(minHeight: 44)
            }
            .padding(.horizontal, 16)
            .background(Color("BgPrimary"))
            ZStack {
                AppScreenWebView(url: app.url, scrolls: true, reloadToken: reload, onMessage: { _ in }, onFail: { failed = true })
                if failed {
                    VStack(spacing: 8) {
                        Text("Couldn’t load \(app.name).").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                        Button("Try again") { failed = false; reload += 1 }
                            .font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary")).frame(minHeight: 44)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color("BgPrimary"))
                }
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
    }
}
