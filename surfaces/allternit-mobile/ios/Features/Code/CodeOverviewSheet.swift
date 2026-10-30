import SwiftUI

/// What the Code overview shows: threads, or terminal sessions while the
/// terminal is on screen (code-overview-kind.tsx).
enum CodeOverviewKind: String, Identifiable {
    case threads, terminals
    var id: String { rawValue }
}

/// The Code header's sessions button: a task list, opening the sessions
/// overview (open threads, and the way to Terminal and Shell).
struct CodeSessionsButton: View {
    var tint: Color = Color("TextPrimary")
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "checklist")
                .font(.system(size: 18))
                .foregroundColor(tint)
                .frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Code sessions")
    }
}

/// The Code sessions overview (PhoneOverviewOverlay / SessionOverview on the
/// phone): ☰ · "Code sessions" · "N open", the layout switch under it;
/// every open session as a card, and a floating
/// "Search code" pill with New at the bottom. Closing a card archives the
/// session.
struct CodeOverviewSheet: View {
    let kind: CodeOverviewKind
    let sessions: [AgentSession]
    @ObservedObject var terminals: CodeTerminalStore
    let onOpenSession: (AgentSession) -> Void
    let onNew: () -> Void
    let onArchive: (AgentSession) -> Void
    let onOpenMenu: () -> Void

    @Environment(\.dismiss) private var dismiss
    @AppStorage("allternit.code.overview-layout") private var layout = "grid"
    @State private var query = ""
    @State private var previews: [String: [MessageRecordPreview]] = [:]
    @FocusState private var searchFocused: Bool

    private var filtered: [AgentSession] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return sessions }
        return sessions.filter { ($0.name ?? "").lowercased().contains(q) }
    }

    private var openCount: Int { kind == .terminals ? terminals.tiles.count : sessions.count }

    var body: some View {
        VStack(spacing: 0) {
            header
            if kind == .threads {
                HStack {
                    Spacer()
                    OverviewLayoutSwitch(raw: $layout, fallback: .grid)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 4)
            }
            ScrollView {
                if kind == .terminals {
                    terminalCards
                } else if filtered.isEmpty {
                    Text(query.isEmpty ? "No code threads yet. Start one and it shows here." : "No code threads match.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("BrandAccent"))
                        .frame(maxWidth: .infinity)
                        .padding(.top, 40)
                        .padding(.horizontal, 24)
                } else if layout == "deck" {
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(spacing: 12) {
                            ForEach(filtered) { card($0).frame(width: OverviewDeck.width, height: OverviewDeck.height, alignment: .top) }
                        }
                        .padding(.horizontal, 12)
                        .padding(.top, 8)
                    }
                    .padding(.bottom, 96)
                } else if layout == "grid" {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(filtered) { card($0) }
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .padding(.bottom, 96)
                } else {
                    LazyVStack(spacing: 8) {
                        ForEach(filtered) { card($0) }
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .padding(.bottom, 96)
                }
            }
        }
        .background(Color("BgPrimary"))
        .overlay(alignment: .bottom) {
            if kind == .threads { bottomBar }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            roundButton("line.3.horizontal", label: "Open menu") {
                dismiss()
                onOpenMenu()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(kind == .terminals ? "Terminal sessions" : "Code sessions")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(openCount) open")
                    .font(.system(size: 12))
                    .foregroundColor(Color("BrandAccent"))
            }
            .padding(.top, 4)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.top, 12)
        .padding(.bottom, 8)
    }

    private func roundButton(_ icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 36, height: 36)
                .background(Color("BgTertiary"))
                .clipShape(Circle())
                .frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    /// One session card: title, ⋯ menu, close (archive), the last exchange,
    /// and its age.
    private func card(_ session: AgentSession) -> some View {
        let title = session.name ?? "Untitled Session"
        return Button(action: {
            dismiss()
            onOpenSession(session)
        }) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 4) {
                    Text(title)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Menu {
                        Button(action: {
                            dismiss()
                            onOpenSession(session)
                        }) { Label("Open", systemImage: "arrow.up.right") }
                        Button(role: .destructive, action: { onArchive(session) }) { Label("Archive", systemImage: "archivebox") }
                    } label: {
                        Image(systemName: "ellipsis")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 28, height: 28)
                    }
                    .accessibilityLabel("More actions for \(title)")
                    Button(action: { onArchive(session) }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 28, height: 28)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Close \(title)")
                }
                preview(for: session)
                    .frame(maxWidth: .infinity, minHeight: layout == "list" ? 0 : 100, alignment: .topLeading)
                HStack {
                    Spacer()
                    Text(CodeSessionDates.age(session))
                        .font(.system(size: 10))
                        .foregroundColor(Color("BrandAccent"))
                }
            }
            .padding(10)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .task(id: session.id) { await loadPreview(session) }
    }

    @ViewBuilder
    private func preview(for session: AgentSession) -> some View {
        let lines = previews[session.id] ?? []
        VStack(alignment: .leading, spacing: 6) {
            ForEach(lines) { line in
                if line.isUser {
                    HStack {
                        Spacer(minLength: 16)
                        Text(line.text)
                            .font(.system(size: 10))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(2)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .background(Color("BgTertiary"))
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                    }
                } else {
                    Text(line.text)
                        .font(.system(size: 10))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(layout == "list" ? 2 : (layout == "deck" ? 12 : 5))
                }
            }
        }
        .mask(LinearGradient(colors: [.black, .black, .black.opacity(0.2)], startPoint: .top, endPoint: .bottom))
    }

    private var terminalCards: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
            ForEach(terminals.tiles) { tile in
                Button(action: {
                    terminals.selectedId = tile.id
                    dismiss()
                }) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text(tile.name)
                                .font(.system(size: 13, weight: .semibold, design: .monospaced))
                                .foregroundColor(TerminalPalette.foreground)
                            Spacer()
                            Button(action: { terminals.closeTile(tile.id) }) {
                                Image(systemName: "xmark")
                                    .font(.system(size: 11, weight: .semibold))
                                    .foregroundColor(TerminalPalette.secondary)
                                    .frame(width: 28, height: 28)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Close \(tile.name)")
                        }
                        Text("$ █")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(TerminalPalette.secondary)
                        Spacer(minLength: 40)
                    }
                    .padding(10)
                    .background(TerminalPalette.background)
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    private var bottomBar: some View {
        HStack(spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search code", text: $query)
                    .font(.system(size: 16))
                    .focused($searchFocused)
                    .submitLabel(.search)
            }
            .padding(.horizontal, 16)
            .frame(height: 48)
            .background(Color("BgPanel"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
            Button(action: {
                dismiss()
                onNew()
            }) {
                Image(systemName: "plus")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 48, height: 48)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                    .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("New code thread")
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 12)
    }

    /// The card's last exchange: the newest user message and the reply after it.
    @MainActor
    private func loadPreview(_ session: AgentSession) async {
        guard previews[session.id] == nil, session.messageCount > 0 else { return }
        guard let records = try? await AgentChatClient().listMessages(sessionId: session.id) else { return }
        let lastUser = records.lastIndex { $0.role == "user" }
        let slice = lastUser.map { Array(records[$0...].prefix(2)) } ?? Array(records.suffix(1))
        previews[session.id] = slice.enumerated().compactMap { index, record in
            let text = record.content.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return nil }
            return MessageRecordPreview(id: "\(session.id)-\(index)", text: text, isUser: record.role == "user")
        }
    }
}

struct MessageRecordPreview: Identifiable, Equatable {
    let id: String
    let text: String
    let isUser: Bool
}
