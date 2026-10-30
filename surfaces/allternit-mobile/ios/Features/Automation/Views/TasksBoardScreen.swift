import SwiftUI

/// The phone Tasks screen (allternit-ai src/views/tasks/TasksBoardScreen.tsx):
/// the cowork Tasks board as a list grouped by column, with the status
/// filter row and "Scheduled ↗" to Automation Tasks. Tasks are Factory
/// nodes; this reads `/api/factory/tasks/board` (the personal list).
struct TasksBoardScreen: View {
    @Binding var isSidebarOpen: Bool
    @EnvironmentObject private var modeStore: AppModeStore

    @State private var cards: [TaskBoardCard]? = nil
    @State private var error: String? = nil
    @State private var status: TaskBoardColumn? = nil

    var body: some View {
        VStack(spacing: 0) {
            LibraryPlainHeader(onMenu: { openLibraryDrawer($isSidebarOpen) })
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    content
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 24)
            }
            .refreshable { await load() }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .task { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if let cards {
            filters(cards)
            if cards.isEmpty {
                Text("No tasks yet. Tasks you or your bots add in a chat show up here.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 14)
            }
            ForEach(TaskBoardColumn.allCases, id: \.self) { column in
                let inColumn = cards.filter { $0.column == column }
                if (status == nil || status == column) && !inColumn.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("\(column.label) · \(inColumn.count)".uppercased())
                            .font(.system(size: 13, weight: .semibold))
                            .tracking(0.4)
                            .foregroundColor(Color("TextSecondary"))
                        ForEach(inColumn) { card($0) }
                    }
                    .padding(.top, 16)
                }
            }
            if let status, !cards.isEmpty, !cards.contains(where: { $0.column == status }) {
                Text("No tasks in this state.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 14)
            }
        } else if let error {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load your tasks",
                message: FriendlyErrorMessage.from(error)
            )
            .padding(.top, 24)
        } else {
            ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
        }
    }

    /// TaskStatusFilters: All + each column with its count, then Scheduled ↗.
    private func filters(_ cards: [TaskBoardCard]) -> some View {
        HStack(spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    filterChip("All", count: cards.count, selected: status == nil) { status = nil }
                    ForEach(TaskBoardColumn.allCases, id: \.self) { column in
                        filterChip(column.label, count: cards.filter { $0.column == column }.count, selected: status == column) {
                            status = column
                        }
                    }
                }
                .padding(.bottom, 4)
            }
            Button("Scheduled ↗") { modeStore.selectBarItem(.automation) }
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .frame(minHeight: 44)
                .padding(.horizontal, 8)
        }
    }

    private func filterChip(_ title: String, count: Int, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Text(title).foregroundColor(selected ? Color("TextPrimary") : Color("TextSecondary"))
                Text("\(count)").foregroundColor(Color("TextTertiary"))
            }
            .font(.system(size: 12))
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 8).fill(selected ? Color("FillNeutral") : Color.clear))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func card(_ card: TaskBoardCard) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(card.title)
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            Text(card.detailLine)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private func load() async {
        do {
            cards = try await TaskBoardClient.board()
            error = nil
        } catch {
            if cards == nil { self.error = error.localizedDescription }
        }
    }
}

// MARK: - Data

/// The board's columns (TASK_STATUS_COLUMNS), in display order.
enum TaskBoardColumn: String, CaseIterable {
    case inReview = "in-review", inProgress = "in-progress", todo, backlog, done

    var label: String {
        switch self {
        case .inReview: return "In review"
        case .inProgress: return "In progress"
        case .todo: return "To do"
        case .backlog: return "Backlog"
        case .done: return "Done"
        }
    }
}

/// A Factory node card (lib/factory/types.ts NodeCard, the fields shown).
struct TaskBoardCard: Decodable, Identifiable, Hashable {
    struct Proof: Decodable, Hashable {
        let proven: Int
        let total: Int
    }

    let dagId: String
    let nodeId: String
    let title: String
    let status: String
    let assignee: String?
    let proof: Proof?
    let needsYou: Bool?

    var id: String { "\(dagId)/\(nodeId)" }

    /// columnForCard.
    var column: TaskBoardColumn {
        switch status {
        case "new": return .backlog
        case "working": return .inProgress
        case "checking", "needs_you": return .inReview
        case "done", "failed": return .done
        default: return .todo
        }
    }

    var detailLine: String {
        [
            needsYou == true ? "Needs you" : nil,
            status == "failed" ? "Failed" : nil,
            assignee.map { $0.hasPrefix("user:") ? String($0.dropFirst(5)) : $0 } ?? "Unassigned",
            (proof?.total ?? 0) > 0 ? "proof \(proof?.proven ?? 0)/\(proof?.total ?? 0)" : nil,
        ].compactMap { $0 }.joined(separator: " · ")
    }
}

/// `GET /api/factory/tasks/board` — mounted beside `/api/v1`, not under
/// it, so the URL is built from the API base's parent.
enum TaskBoardClient {
    private struct Board: Decodable {
        struct Wave: Decodable { let nodes: [TaskBoardCard] }
        let waves: [Wave]
    }

    static func board() async throws -> [TaskBoardCard] {
        let client = APIClient.shared
        let apiBase = client.baseURL.lastPathComponent == "v1" ? client.baseURL.deletingLastPathComponent() : client.baseURL
        let url = apiBase.appendingPathComponent("factory/tasks/board")
        let request = try await client.authorizedRequest(url: url)
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        let board = try JSONDecoder().decode(Board.self, from: data)
        // A card can appear in more than one wave: keep each once.
        var seen = Set<String>()
        return board.waves.flatMap(\.nodes).filter { seen.insert($0.id).inserted }
    }
}
