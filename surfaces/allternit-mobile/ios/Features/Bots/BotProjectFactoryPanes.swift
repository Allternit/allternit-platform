import SwiftUI
import SwiftTerm

// The Board, Flow and Team panes (components/factory/views): the web's views,
// fed by the same Factory routes, polled every 10 s in place of its live events.
// A refresh never moves the canvas, and a failed refresh keeps what's on screen.

private let projectFactoryPollSeconds: UInt64 = 10

private enum ProjectFactoryWords {
    static let noCampaignTitle = "This project has no campaign yet."
    static let noCampaignDetail = "A campaign starts when work for this project runs through the Factory (a template run, or gizzi workspace campaign new). Its board shows here."

    static func status(_ status: String) -> String { status.replacingOccurrences(of: "_", with: " ") }

    /// BINDING_TYPE_LABEL.
    static func bindingType(_ type: String?) -> String? {
        switch type {
        case "hosted": return "Hosted"
        case "terminal": return "Terminal"
        case "vendor": return "Vendor"
        default: return nil
        }
    }
}

private struct ProjectStaleNote: View {
    let failure: ProjectFactoryFailure?

    var body: some View {
        if let failure {
            Text("Showing the last update. \(failure.fact)")
                .font(.system(size: 11.5))
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Board

/// Board (views/Board.tsx `oneColumn`): the intent, the summary (NOW / NEXT /
/// PROVEN / NEEDS YOU, with the judge's progress bar), then a card per node in
/// waves. The web's Board is a card grid, not a graph, so it scrolls.
struct ProjectBoardPane: View {
    let projectId: String

    @EnvironmentObject private var navigator: BotNavigator
    @State private var board: FactoryBoard?
    @State private var noCampaign = false
    @State private var isLoading = true
    @State private var failure: ProjectFactoryFailure?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let board {
                    ProjectStaleNote(failure: failure)
                    boardBody(board)
                } else if isLoading {
                    ProjectFactorySkeleton(rows: 4, label: "Loading the board")
                } else if let failure {
                    ProjectFactoryFailureView(failure: failure) { Task { await load() } }
                } else if noCampaign {
                    ProjectFactoryEmptyView(title: ProjectFactoryWords.noCampaignTitle, detail: ProjectFactoryWords.noCampaignDetail)
                }
            }
            .padding(16)
        }
        .refreshable { await load() }
        .task { await poll() }
    }

    @ViewBuilder
    private func boardBody(_ board: FactoryBoard) -> some View {
        if let intent = board.campaign?.intent, !intent.isEmpty {
            Text(intent)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        summary(board.summary)
        let waves = board.sortedWaves
        if waves.isEmpty {
            ProjectFactoryEmptyView(title: "No nodes in this campaign yet.", detail: "Nodes show here once a plan is run.")
        } else {
            ForEach(Array(waves.enumerated()), id: \.element.depth) { index, wave in
                VStack(alignment: .leading, spacing: 8) {
                    (Text("Wave \(index + 1)").font(.system(size: 12, weight: .medium)).foregroundColor(Color("TextSecondary"))
                        + Text(" · \(wave.nodes.count == 1 ? "1 node" : "\(wave.nodes.count) nodes")").font(.system(size: 12)).foregroundColor(Color("TextTertiary")))
                    ForEach(wave.nodes) { card in
                        ProjectBoardNodeCard(card: card) { open(card) }
                    }
                }
            }
        }
    }

    private func summary(_ summary: FactoryBoard.Summary) -> some View {
        let proven = summary.proven
        return VStack(alignment: .leading, spacing: 8) {
            summaryRow("NOW", cards: summary.now, warning: false)
            summaryRow("NEXT", cards: summary.next, warning: false)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                summaryLabel("PROVEN")
                Text(proven.map { $0.n > 0 ? "\($0.k)/\($0.n)" : "—" } ?? "—")
                    .font(.system(size: 12.5).monospacedDigit())
                    .foregroundColor(Color("TextPrimary"))
            }
            .frame(minHeight: 28)
            summaryRow("NEEDS YOU", cards: summary.needsYou, warning: true)
            progress(proven)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private func summaryLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .tracking(0.3)
            .foregroundColor(Color("TextTertiary"))
    }

    private func summaryRow(_ label: String, cards: [FactoryNodeCard], warning: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            summaryLabel(label)
            if cards.isEmpty {
                Text("—").font(.system(size: 12.5)).foregroundColor(Color("TextTertiary"))
            } else {
                ProjectChipWrap(spacing: 2) {
                    ForEach(cards.sorted { $0.nodeId < $1.nodeId }) { card in
                        Button { open(card) } label: {
                            Text(card.nodeId)
                                .font(.system(size: 12.5).monospacedDigit())
                                .underline()
                                .foregroundColor(warning ? Theme.statusWarning : Color("TextPrimary"))
                                .padding(.horizontal, 6)
                                .frame(minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Open \(card.nodeId) \(card.title)")
                    }
                }
            }
        }
    }

    private func progress(_ proven: FactoryBoard.Proven?) -> some View {
        let k = proven?.k ?? 0
        let n = proven?.n ?? 0
        let fraction = n > 0 ? min(1, max(0, Double(k) / Double(n))) : 0
        return VStack(alignment: .leading, spacing: 4) {
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("BgPanel"))
                    Capsule().fill(Theme.statusSuccess).frame(width: geometry.size.width * fraction)
                }
            }
            .frame(height: 6)
            Text(n > 0 ? "\(k) of \(n) proven" : "Nothing to prove yet")
                .font(.system(size: 11.5).monospacedDigit())
                .foregroundColor(Color("TextSecondary"))
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(n > 0 ? "\(k) of \(n) proven" : "Nothing to prove yet")
    }

    private func open(_ card: FactoryNodeCard) {
        navigator.open(.projectNode(dagId: card.dagId, nodeId: card.nodeId, title: card.title))
    }

    private func poll() async {
        while !Task.isCancelled {
            await load()
            try? await Task.sleep(nanoseconds: projectFactoryPollSeconds * 1_000_000_000)
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            guard let campaign = try await BotProjectClient.shared.campaign(projectId: projectId) else {
                board = nil
                noCampaign = true
                failure = nil
                return
            }
            board = try await BotProjectClient.shared.board(campaignId: campaign.id)
            noCampaign = false
            failure = nil
        } catch is CancellationError {
        } catch where Task.isCancelled {
        } catch {
            failure = ProjectFactoryFailure(error)
        }
    }
}

/// BoardNodeCard: id and status, title, who, what it waits on, proof.
private struct ProjectBoardNodeCard: View {
    let card: FactoryNodeCard
    let onOpen: () -> Void

    private var who: String {
        guard let assignee = card.assignee else { return "—" }
        return [assignee, ProjectFactoryWords.bindingType(card.bindingType)].compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(card.nodeId)
                        .font(.system(size: 11.5).monospacedDigit())
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    ProjectFactoryPill(label: ProjectFactoryWords.status(card.status), tone: .node(card.status))
                }
                Text(card.title)
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Text(who)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                HStack(spacing: 8) {
                    Text("after: \((card.blockedBy ?? []).isEmpty ? "—" : (card.blockedBy ?? []).joined(separator: ", "))")
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    ProjectProofCount(proof: card.proof)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(card.needsYou == true ? Theme.statusWarning : Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Open \(card.nodeId) \(card.title), \(ProjectFactoryWords.status(card.status))")
    }
}

// MARK: - Flow

/// Flow (views/FlowGraph.tsx, CampaignFlowView): the live DAG left to right by
/// depth on a canvas. `blocked_by` runs forward; `on_fail` routes back under
/// the cards in the warning colour; a gate marks its step. List draws the same
/// columns as an indented list.
struct ProjectFlowPane: View {
    let projectId: String

    private enum Mode: String { case graph, list }

    @EnvironmentObject private var navigator: BotNavigator
    @State private var mode: Mode = .graph
    @State private var dagIds: [String] = []
    @State private var picked: String?
    @State private var flow: FactoryFlow?
    @State private var noCampaign = false
    @State private var isLoading = true
    @State private var failure: ProjectFactoryFailure?
    @State private var savingRun = false

    private var dagId: String? {
        if let picked, dagIds.contains(picked) { return picked }
        return dagIds.first
    }

    var body: some View {
        Group {
            if let flow {
                loaded(flow)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if isLoading {
                            ProjectFactorySkeleton(rows: 3, label: "Loading the flow")
                        } else if let failure {
                            ProjectFactoryFailureView(failure: failure) { Task { await load() } }
                        } else if noCampaign {
                            ProjectFactoryEmptyView(title: ProjectFactoryWords.noCampaignTitle, detail: ProjectFactoryWords.noCampaignDetail)
                        } else {
                            ProjectFactoryEmptyView(title: "No flow yet.", detail: "The flow draws once this campaign has nodes.")
                        }
                    }
                    .padding(16)
                }
                .refreshable { await load() }
            }
        }
        .task(id: picked) { await poll() }
    }

    private func loaded(_ flow: FactoryFlow) -> some View {
        let layout = ProjectGraphLayouts.flow(flow)
        return VStack(alignment: .leading, spacing: 10) {
            header(flow)
            ProjectStaleNote(failure: failure).padding(.horizontal, 16)
            if layout.nodes.isEmpty {
                ProjectFactoryEmptyView(title: "No steps yet.").padding(.horizontal, 16)
                Spacer(minLength: 0)
            } else if mode == .graph {
                ProjectGraphCanvas(contentSize: layout.contentSize) { canvasContent(layout) }
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .padding(.horizontal, 12)
                ProjectGraphLegend(items: [
                    .init(label: "blocked by", dash: []),
                    .init(label: "on fail", dash: [5, 4], warning: true),
                ], showsGate: true)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            } else {
                flowList(layout)
            }
        }
        .padding(.top, 10)
    }

    private func header(_ flow: FactoryFlow) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(flow.title)
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
            ProjectChipWrap(spacing: 8) {
                ProjectViewSwitch(options: [Mode.graph, .list], selection: $mode) { $0.rawValue.capitalized }
                Button("Save as template") { savingRun = true }
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 10)
                    .frame(minHeight: 44)
                if dagIds.count > 1 {
                    Menu {
                        ForEach(dagIds, id: \.self) { id in
                            Button {
                                picked = id
                            } label: {
                                if id == dagId {
                                    Label(id, systemImage: "checkmark")
                                } else {
                                    Text(id)
                                }
                            }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text("Flow").foregroundColor(Color("TextSecondary"))
                            Text(dagId ?? "").foregroundColor(Color("TextPrimary")).lineLimit(1)
                            Image(systemName: "chevron.down").font(.system(size: 10, weight: .semibold))
                        }
                        .font(.system(size: 12.5))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .accessibilityLabel("Flow: \(dagId ?? "")")
                }
            }
        }
        .padding(.horizontal, 16)
        .sheet(isPresented: $savingRun) {
            ProjectSaveRunSheet(flow: flow).presentationDetents([.medium, .large])
        }
    }

    private func canvasContent(_ layout: ProjectGraphLayout<ProjectFlowStep>) -> some View {
        ZStack(alignment: .topLeading) {
            ProjectGraphEdges(layout: layout, arrows: true)
            ForEach(layout.nodes, id: \.id) { placed in
                ProjectFlowStepCard(step: placed.item)
                    .frame(width: layout.card.width, height: layout.card.height)
                    .contentShape(Rectangle())
                    .onTapGesture { open(placed.item.card) }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isButton)
                    .accessibilityAction { open(placed.item.card) }
                    .offset(x: placed.origin.x, y: placed.origin.y)
            }
        }
    }

    /// graphAsList: ordered by depth, then id, indented by depth.
    private func flowList(_ layout: ProjectGraphLayout<ProjectFlowStep>) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(layout.nodes.sorted { ($0.column, $0.id) < ($1.column, $1.id) }, id: \.id) { placed in
                    let fails = layout.edges.filter { $0.style == .onFail && $0.from == placed.id }.map(\.to)
                    VStack(alignment: .leading, spacing: 2) {
                        Button { open(placed.item.card) } label: {
                            ProjectFlowStepCard(step: placed.item)
                        }
                        .buttonStyle(.plain)
                        if !fails.isEmpty {
                            Text("on fail → \(fails.joined(separator: ", "))")
                                .font(.system(size: 11.5))
                                .foregroundColor(Theme.statusWarning)
                                .padding(.leading, 8)
                        }
                    }
                    .padding(.leading, CGFloat(min(placed.column, 4)) * 12)
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 16)
        }
        .refreshable { await load() }
    }

    private func open(_ card: FactoryNodeCard) {
        navigator.open(.projectNode(dagId: card.dagId, nodeId: card.nodeId, title: card.title))
    }

    private func poll() async {
        while !Task.isCancelled {
            await load()
            try? await Task.sleep(nanoseconds: projectFactoryPollSeconds * 1_000_000_000)
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            guard let campaign = try await BotProjectClient.shared.campaign(projectId: projectId) else {
                flow = nil
                dagIds = []
                noCampaign = true
                failure = nil
                return
            }
            noCampaign = false
            dagIds = try await BotProjectClient.shared.board(campaignId: campaign.id).dagIds
            if let dagId {
                flow = try await BotProjectClient.shared.flow(dagId: dagId)
            } else {
                flow = nil
            }
            failure = nil
        } catch is CancellationError {
        } catch where Task.isCancelled {
        } catch {
            failure = ProjectFactoryFailure(error)
        }
    }
}

/// StepCard: id, gate and status, title, assignee, proof.
private struct ProjectFlowStepCard: View {
    let step: ProjectFlowStep

    private var card: FactoryNodeCard { step.card }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text(card.nodeId)
                    .font(.system(size: 11).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
                Spacer(minLength: 2)
                if let gate = step.gateKind {
                    HStack(spacing: 2) {
                        Image(systemName: "hand.raised").font(.system(size: 10))
                        Text(gate == "human" ? "you" : gate).font(.system(size: 11)).lineLimit(1)
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 1)
                    .background(Capsule().fill(Theme.statusWarning.opacity(0.22)))
                    .accessibilityLabel(gate == "human" ? "Waiting on you" : "Gate: \(gate)")
                }
                ProjectFactoryPill(label: ProjectFactoryWords.status(card.status), tone: .node(card.status))
            }
            Text(card.title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            HStack(spacing: 6) {
                Text(card.assignee ?? "—")
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 2)
                if card.proof != nil { ProjectProofCount(proof: card.proof) }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPrimary")))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(border, lineWidth: 1))
    }

    private var border: SwiftUI.Color {
        switch card.status {
        case "needs_you": return Theme.statusWarning
        case "failed": return Theme.statusError
        default: return Color("BorderSubtle")
        }
    }
}

// MARK: - Team

/// Team (views/TeamGraph.tsx, TeamLanes.tsx, connected.tsx): Graph draws every
/// bot in fixed layers on a canvas, Lanes the hand-offs and what's in them
/// (stacked, the web's phone form), List the same layers indented.
struct ProjectTeamPane: View {
    let projectId: String
    let botIds: [String]

    private enum Mode: String { case graph, lanes, list }

    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var mode: Mode = .graph
    @State private var agents: [FactoryAgent] = []
    @State private var edges: [FactoryTeam.Edge] = []
    @State private var nodes: [FactoryNodeCard] = []
    @State private var isLoading = true
    @State private var failure: ProjectFactoryFailure?

    var body: some View {
        Group {
            if agents.isEmpty {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if isLoading {
                            ProjectFactorySkeleton(rows: 2, label: "Loading the team")
                        } else {
                            if let failure {
                                ProjectFactoryFailureView(failure: failure) { Task { await load() } }
                            } else {
                                ProjectFactoryEmptyView(title: "None of this project's bots are in the Factory yet.",
                                                        detail: "Bots appear here once the Factory runs them.")
                            }
                            ForEach(botIds.compactMap { store.bot($0) }) { bot in
                                Button { navigator.open(.page(bot.id)) } label: {
                                    HStack(spacing: 10) {
                                        ProjectBotFigure(botId: bot.id, size: 32)
                                        BotNameText(bot.botDisplayName, size: 13)
                                        Spacer(minLength: 0)
                                    }
                                    .frame(minHeight: 44)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    .padding(16)
                }
                .refreshable { await load() }
            } else {
                loaded
            }
        }
        .task(id: botIds) { await poll() }
    }

    private var loaded: some View {
        let layout = ProjectGraphLayouts.team(agents: agents, edges: edges)
        return VStack(alignment: .leading, spacing: 10) {
            ProjectViewSwitch(options: [Mode.graph, .lanes, .list], selection: $mode) { $0.rawValue.capitalized }
                .padding(.horizontal, 16)
            ProjectStaleNote(failure: failure).padding(.horizontal, 16)
            switch mode {
            case .graph:
                ProjectGraphCanvas(contentSize: layout.contentSize) { canvasContent(layout) }
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .padding(.horizontal, 12)
                ProjectGraphLegend(items: [
                    .init(label: "delegates to", dash: []),
                    .init(label: "checks", dash: [6, 4]),
                    .init(label: "directs", dash: [1.5, 4]),
                ])
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
            case .lanes:
                ScrollView {
                    ProjectTeamLanes(agents: agents, edges: edges, nodes: nodes, onOpenAgent: { open($0) }) { node in
                        navigator.open(.projectNode(dagId: node.dagId, nodeId: node.nodeId, title: node.title))
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 16)
                }
                .refreshable { await load() }
            case .list:
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(layout.nodes.sorted { ($0.column, $0.row) < ($1.column, $1.row) }, id: \.id) { placed in
                            Button { open(placed.item) } label: {
                                ProjectTeamAgentCard(agent: placed.item)
                            }
                            .buttonStyle(.plain)
                            .padding(.leading, CGFloat(min(placed.column, 4)) * 12)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 16)
                }
                .refreshable { await load() }
            }
        }
        .padding(.top, 10)
    }

    private func canvasContent(_ layout: ProjectGraphLayout<FactoryAgent>) -> some View {
        ZStack(alignment: .topLeading) {
            ProjectGraphEdges(layout: layout)
            ForEach(layout.nodes, id: \.id) { placed in
                ProjectTeamAgentCard(agent: placed.item)
                    .frame(width: layout.card.width, height: layout.card.height)
                    .contentShape(Rectangle())
                    .onTapGesture { open(placed.item) }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isButton)
                    .accessibilityAction { open(placed.item) }
                    .offset(x: placed.origin.x, y: placed.origin.y)
            }
        }
    }

    /// A bot opens what it's on, else its own page.
    private func open(_ agent: FactoryAgent) {
        if let node = agent.currentNode {
            navigator.open(.projectNode(dagId: node.dagId, nodeId: node.nodeId, title: node.title))
        } else {
            navigator.open(.page(agent.id))
        }
    }

    private func poll() async {
        while !Task.isCancelled {
            await load()
            try? await Task.sleep(nanoseconds: projectFactoryPollSeconds * 1_000_000_000)
        }
    }

    private func load() async {
        defer { isLoading = false }
        do {
            let team = try await BotProjectClient.shared.team(botIds: botIds)
            agents = team.agents
            edges = team.edges
            failure = nil
            // Lanes read the campaign's nodes; with none, each bot's current node stands in.
            if let campaign = try? await BotProjectClient.shared.campaign(projectId: projectId),
               let board = try? await BotProjectClient.shared.board(campaignId: campaign.id) {
                nodes = board.allNodes
            } else {
                nodes = []
            }
        } catch is CancellationError {
        } catch where Task.isCancelled {
        } catch {
            failure = ProjectFactoryFailure(error)
        }
    }
}

/// AgentCard: the bot standing on its card, address, binding, state, what it's on.
private struct ProjectTeamAgentCard: View {
    let agent: FactoryAgent

    @ObservedObject private var store = BotModeStore.shared

    private var stateWord: String {
        ProjectFactoryWords.status(agent.state ?? "idle")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                ProjectBotFigure(botId: agent.id, size: 38, state: agent.state)
                VStack(alignment: .leading, spacing: 1) {
                    Text(store.bot(agent.id)?.botDisplayName ?? agent.name)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(agent.address)
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: 6) {
                if let binding = agent.binding {
                    Text(binding.label)
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                ProjectFactoryPill(label: stateWord, tone: .agent(agent.state))
            }
            HStack(spacing: 6) {
                Text(agent.currentNode.map { "\($0.nodeId) · \($0.title)" } ?? "—")
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 2)
                ProjectProofCount(proof: agent.proof)
                ProjectReachIcons(reach: agent.reach)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPrimary")))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .stroke(agent.state == "needs_you" ? Theme.statusWarning : Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
    }
}

// MARK: - Team lanes

/// lib/factory/lanes.ts: one lane per team edge in the direction work moves
/// (checks: the checked bot → its checker), holding the nodes handed along it.
private struct ProjectFactoryLane: Identifiable {
    let id: String
    let kind: String
    let from: String
    let to: String
    var nodes: [FactoryNodeCard]

    private static let order = ["needs_you": 0, "working": 1, "checking": 1, "failed": 2, "ready": 3, "new": 4, "blocked": 4, "done": 5]

    static func build(edges: [FactoryTeam.Edge], agents: [FactoryAgent], nodes: [FactoryNodeCard]) -> [ProjectFactoryLane] {
        let ids = Set(agents.map(\.id))
        var lanes: [ProjectFactoryLane] = []
        var seen = Set<String>()
        for edge in edges {
            let (from, to) = edge.kind == "checks" ? (edge.to, edge.from) : (edge.from, edge.to)
            let id = "\(from)->\(to)"
            guard ids.contains(from), ids.contains(to), from != to, seen.insert(id).inserted else { continue }
            lanes.append(ProjectFactoryLane(id: id, kind: edge.kind, from: from, to: to, nodes: []))
        }
        guard !lanes.isEmpty else { return lanes }

        let pool = nodes.isEmpty ? agents.compactMap(currentCard) : nodes
        var idOf: [String: String] = [:]
        for agent in agents {
            idOf[agent.address] = agent.id
            idOf[agent.id] = agent.id
        }
        var owner: [String: String] = [:]
        for node in pool {
            if let assignee = node.assignee, let id = idOf[assignee] { owner["\(node.dagId):\(node.nodeId)"] = id }
        }
        for node in pool {
            guard let assignee = node.assignee, let id = idOf[assignee] else { continue }
            let incoming = lanes.indices.filter { lanes[$0].to == id }
            guard let first = incoming.first else { continue }
            let sources = Set((node.blockedBy ?? []).compactMap { owner["\(node.dagId):\($0)"] })
            lanes[incoming.first { sources.contains(lanes[$0].from) } ?? first].nodes.append(node)
        }
        for index in lanes.indices {
            lanes[index].nodes.sort { (order[$0.status] ?? 9) < (order[$1.status] ?? 9) }
        }
        return lanes
    }

    /// A team with no campaign still shows its work.
    private static func currentCard(_ agent: FactoryAgent) -> FactoryNodeCard? {
        guard let node = agent.currentNode else { return nil }
        let status: String
        switch agent.state {
        case "needs_you": status = "needs_you"
        case "done": status = "done"
        case "failed": status = "failed"
        default: status = "working"
        }
        return FactoryNodeCard(dagId: node.dagId, nodeId: node.nodeId, title: node.title, status: status,
                               assignee: agent.address, bindingType: agent.binding?.type,
                               proof: agent.proof ?? FactoryProof(proven: 0, total: 0), blockedBy: [],
                               needsYou: agent.state == "needs_you", depth: 0, gate: nil)
    }
}

private struct ProjectTeamLanes: View {
    let agents: [FactoryAgent]
    let edges: [FactoryTeam.Edge]
    let nodes: [FactoryNodeCard]
    let onOpenAgent: (FactoryAgent) -> Void
    let onOpenNode: (FactoryNodeCard) -> Void

    @State private var openId: String?

    private var lanes: [ProjectFactoryLane] { ProjectFactoryLane.build(edges: edges, agents: agents, nodes: nodes) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                ForEach(agents) { agent in
                    Button { onOpenAgent(agent) } label: {
                        ProjectLaneAgentCard(agent: agent, nodes: nodes)
                    }
                    .buttonStyle(.plain)
                }
            }
            if lanes.isEmpty {
                ProjectCardNote("These bots don't hand work to each other yet. Lanes appear when the team has a delegates-to or checks link.")
            } else {
                ForEach(lanes) { lane in
                    ProjectLaneCard(lane: lane, agents: agents, openId: $openId, onOpenNode: onOpenNode)
                }
            }
        }
    }
}

/// HandoffLanes AgentCard, compact: face, name, binding, state, the slot.
private struct ProjectLaneAgentCard: View {
    let agent: FactoryAgent
    let nodes: [FactoryNodeCard]

    @ObservedObject private var store = BotModeStore.shared

    private var current: FactoryNodeCard? {
        guard let node = agent.currentNode else { return nil }
        return nodes.first { $0.dagId == node.dagId && $0.nodeId == node.nodeId }
    }

    private var queued: [FactoryNodeCard] {
        nodes.filter { $0.assignee == agent.address && ($0.status == "ready" || $0.status == "new") }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                ProjectBotFigure(botId: agent.id, size: 32, state: agent.state)
                VStack(alignment: .leading, spacing: 0) {
                    Text(store.bot(agent.id)?.botDisplayName ?? agent.name)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let binding = agent.binding {
                        Text(binding.label)
                            .font(.system(size: 11.5))
                            .foregroundColor(Color("TextTertiary"))
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
            }
            ProjectFactoryPill(label: ProjectFactoryWords.status(agent.state ?? "idle").projectSentenceCase, tone: .agent(agent.state))
            slot
                .font(.system(size: 12))
                .padding(.horizontal, 8)
                .frame(maxWidth: .infinity, minHeight: 30, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 8).fill(Color("FillNeutral")))
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
            .stroke(agent.state == "needs_you" ? Theme.statusWarning : Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
    }

    @ViewBuilder private var slot: some View {
        if let node = agent.currentNode {
            HStack(spacing: 4) {
                Text(node.title).fontWeight(.medium).foregroundColor(Color("TextPrimary")).lineLimit(1)
                if let current {
                    Spacer(minLength: 2)
                    Text(ProjectFactoryWords.status(current.status).projectSentenceCase).foregroundColor(Color("TextTertiary")).lineLimit(1)
                }
            }
        } else if let next = queued.first {
            HStack(spacing: 4) {
                Text("Up next").foregroundColor(Color("TextTertiary"))
                Text(next.title).fontWeight(.medium).foregroundColor(Color("TextPrimary")).lineLimit(1)
                if queued.count > 1 {
                    Text("+\(queued.count - 1)").foregroundColor(Color("TextTertiary"))
                }
            }
        } else {
            Text("Ready for a task").foregroundColor(Color("TextTertiary"))
        }
    }
}

/// One hand-off: "Scout → Raven", its count, the tasks moving along it.
private struct ProjectLaneCard: View {
    let lane: ProjectFactoryLane
    let agents: [FactoryAgent]
    @Binding var openId: String?
    let onOpenNode: (FactoryNodeCard) -> Void

    @ObservedObject private var store = BotModeStore.shared
    @State private var showAll = false

    private let maxItems = 4

    private func name(_ id: String) -> String {
        store.bot(id)?.botDisplayName ?? agents.first { $0.id == id }?.name ?? "?"
    }

    private var kindWord: String? {
        lane.kind == "checks" || lane.kind == "directs" ? lane.kind : nil
    }

    private var shown: [FactoryNodeCard] { showAll ? lane.nodes : Array(lane.nodes.prefix(maxItems)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            if lane.nodes.isEmpty {
                Text("Nothing handed over yet.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            } else {
                ForEach(shown) { node in
                    item(node)
                }
            }
            if lane.nodes.count > maxItems {
                Button(showAll ? "Show less" : "+\(lane.nodes.count - maxItems) more") { showAll.toggle() }
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 6)
                    .frame(minHeight: 44)
            }
        }
        .padding(6)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
    }

    private var header: some View {
        HStack(spacing: 6) {
            ProjectBotFigure(botId: lane.from, size: 18)
            Text(name(lane.from)).lineLimit(1)
            Text("→").foregroundColor(Color("TextTertiary")).accessibilityLabel("to")
            ProjectBotFigure(botId: lane.to, size: 18)
            Text(name(lane.to)).lineLimit(1)
            if let kindWord {
                Text("· \(kindWord)").foregroundColor(Color("TextTertiary")).lineLimit(1)
            }
            Spacer(minLength: 4)
            Text("\(lane.nodes.count)")
                .font(.system(size: 11).monospacedDigit())
                .padding(.horizontal, 6)
                .padding(.vertical, 1)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgPanel")))
        }
        .font(.system(size: 12))
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 6)
        .padding(.top, 4)
    }

    private func nodeWord(_ status: String) -> String {
        switch status {
        case "working": return "With \(name(lane.to))"
        case "checking": return "Checking"
        case "needs_you": return "Needs you"
        case "done": return "Done"
        case "failed": return "Failed"
        case "ready": return "Up next"
        case "blocked": return "Waiting on another step"
        default: return "Not started"
        }
    }

    private func item(_ node: FactoryNodeCard) -> some View {
        let key = "\(lane.id):\(node.id)"
        let isOpen = openId == key
        return VStack(alignment: .leading, spacing: 0) {
            Button {
                openId = isOpen ? nil : key
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(node.title)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                    Text((node.proof?.total ?? 0) > 0 ? "\(node.nodeId) · proof \(node.proof?.proven ?? 0)/\(node.proof?.total ?? 0)" : node.nodeId)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                    HStack(spacing: 6) {
                        let tone = ProjectFactoryTone.node(node.status)
                        if tone == .run {
                            ProgressView().controlSize(.mini)
                        } else {
                            Circle().fill(tone.color).frame(width: 7, height: 7)
                        }
                        Text(nodeWord(node.status))
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.top, 2)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(isOpen ? .isSelected : [])
            if isOpen {
                Divider()
                ProjectLaneNodeDetail(node: node, assignee: agents.first { $0.id == lane.to }, onOpenNode: onOpenNode)
                    .padding(10)
            }
        }
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .stroke(isOpen ? Color("BorderStrong") : Color("BorderSubtle"), lineWidth: 1))
    }
}

/// An opened task: its intent, files, proof, deliveries, and the way to its page.
private struct ProjectLaneNodeDetail: View {
    let node: FactoryNodeCard
    let assignee: FactoryAgent?
    let onOpenNode: (FactoryNodeCard) -> Void

    @State private var page: FactoryNodePage?
    @State private var isLoading = true
    @State private var failed = false
    @State private var takingOver = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if isLoading {
                Text("Loading the node…").font(.system(size: 11.5)).foregroundColor(Color("TextTertiary"))
            } else if failed {
                Text("Couldn't load this node's files.").font(.system(size: 11.5)).foregroundColor(Color("TextTertiary"))
            } else {
                if let intent = page?.spec?.intent, !intent.isEmpty {
                    Text(intent)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.bottom, 2)
                }
                let files = page?.files ?? []
                ForEach(files.prefix(4), id: \.path) { file in
                    fact("doc.text", Text(file.path).font(.system(size: 11.5, design: .monospaced)))
                }
                if files.count > 4 {
                    Text("+\(files.count - 4) files")
                        .font(.system(size: 11.5))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 8)
                }
                fact("checkmark.circle", Text("Proof \(node.proof?.proven ?? 0)/\(node.proof?.total ?? 0)").font(.system(size: 12)))
                let deliveries = page?.deliveries?.count ?? 0
                if deliveries > 0 {
                    fact("shippingbox", Text(deliveries == 1 ? "1 delivery" : "\(deliveries) deliveries").font(.system(size: 12)))
                }
            }
            HStack(spacing: 6) {
                Button { onOpenNode(node) } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "arrow.up.right.square").font(.system(size: 12))
                        Text("Open node").font(.system(size: 12))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 10)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                if assignee?.attachablePane != nil {
                    Button { takingOver = true } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "terminal").font(.system(size: 12))
                            Text("Take over").font(.system(size: 12))
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 10)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint("Open this bot's terminal pane and work in it")
                }
            }
        }
        .task(id: node.id) { await load() }
        .fullScreenCover(isPresented: $takingOver) {
            if let assignee { ProjectBotPaneScreen(agent: assignee) }
        }
    }

    private func fact(_ systemName: String, _ text: Text) -> some View {
        HStack(spacing: 6) {
            Image(systemName: systemName).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            text.foregroundColor(Color("TextPrimary")).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(RoundedRectangle(cornerRadius: 6).fill(Color("FillNeutral")))
    }

    private func load() async {
        defer { isLoading = false }
        do {
            page = try await BotProjectClient.shared.node(dagId: node.dagId, nodeId: node.nodeId)
            failed = false
        } catch is CancellationError {
        } catch {
            failed = true
        }
    }
}

private extension String {
    var projectSentenceCase: String { prefix(1).uppercased() + dropFirst() }
}

// MARK: - Reach icons

/// ReachIcons: how a bot can reach you, known channels only.
private struct ProjectReachIcons: View {
    let reach: [String]

    private static let channels: [String: (icon: String, label: String)] = [
        "telegram": ("paperplane", "Telegram"), "slack": ("number", "Slack"), "sms": ("message", "SMS"),
        "email": ("envelope", "Email"), "push": ("bell", "Push"), "phone": ("phone", "Phone"),
    ]

    private var known: [String] { Array(Set(reach)).filter { Self.channels[$0] != nil }.sorted() }

    var body: some View {
        if !known.isEmpty {
            HStack(spacing: 4) {
                ForEach(known, id: \.self) { channel in
                    Image(systemName: Self.channels[channel]?.icon ?? "circle")
                        .font(.system(size: 10.5))
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Reach: \(known.compactMap { Self.channels[$0]?.label }.joined(separator: ", "))")
        }
    }
}

// MARK: - Save run as template

/// SaveRunAsTemplate: names the run, then asks the engine to turn its DAG back
/// into a workspace template. A name already taken offers Replace; an engine
/// without the route says to update.
private struct ProjectSaveRunSheet: View {
    let flow: FactoryFlow

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var saving = false
    @State private var saved: BotProjectClient.SavedRunTemplate?
    @State private var errorFact: String?
    @State private var errorAction: String?
    @State private var taken = false

    init(flow: FactoryFlow) {
        self.flow = flow
        _name = State(initialValue: Self.templateId(fromTitle: flow.title))
    }

    /// templateIdFromTitle.
    private static func templateId(fromTitle title: String) -> String {
        var out = ""
        for character in title.lowercased() {
            if character.isASCII, character.isLetter || character.isNumber {
                out.append(character)
            } else if !out.isEmpty, !out.hasSuffix("-") {
                out.append("-")
            }
            if out.count >= 48 { break }
        }
        while out.hasSuffix("-") { out.removeLast() }
        return out.isEmpty ? "template" : out
    }

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        NavigationStack {
            Form {
                Section("Template name") {
                    TextField("Template name", text: $name)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .onChange(of: name) { _, value in
                            if value.count > 64 { name = String(value.prefix(64)) }
                            errorFact = nil
                            errorAction = nil
                            taken = false
                        }
                        .disabled(saved != nil)
                }
                Section {
                    Text("Saves this run's steps, order and approvals as a template you can run again. Bots stay as assigned in this run.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                }
                if let saved {
                    Section {
                        Text("Saved as \(saved.id) (\(saved.steps) steps).\(saved.fromRun.finished ? "" : " Some steps of this run hadn't finished, so look the template over before you use it.") Start it from Workflows.")
                            .font(.system(size: 13))
                    }
                }
                if let errorFact {
                    Section {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(errorFact).font(.system(size: 13)).foregroundColor(Theme.statusError)
                            if let errorAction {
                                Text(errorAction).font(.system(size: 12.5)).foregroundColor(Color("TextSecondary"))
                            }
                        }
                    }
                }
                if saved == nil {
                    Section {
                        Button(saving ? "Saving…" : "Save") { submit(force: false) }
                            .frame(minHeight: 44)
                            .disabled(saving || trimmed.isEmpty)
                        if taken {
                            Button("Replace it") { submit(force: true) }
                                .frame(minHeight: 44)
                                .disabled(saving)
                        }
                    }
                }
            }
            .navigationTitle("Save as template")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: saved == nil ? .cancellationAction : .confirmationAction) {
                    Button(saved == nil ? "Cancel" : "Done") { dismiss() }
                }
            }
        }
    }

    private func submit(force: Bool) {
        let id = trimmed
        let run = flow.dagId
        saving = true
        errorFact = nil
        errorAction = nil
        Task {
            defer { saving = false }
            do {
                saved = try await BotProjectClient.shared.saveRunAsTemplate(fromRun: run, id: id, force: force)
                taken = false
            } catch let refusal as BotProjectClient.FactoryRefusal {
                show(status: refusal.status, code: refusal.code, fact: refusal.fact, action: refusal.action)
            } catch APIError.httpError(let status, _) {
                show(status: status, code: nil, fact: "Couldn't save the template.", action: "Try again.")
            } catch {
                let failure = ProjectFactoryFailure(error)
                errorFact = failure.fact
                errorAction = failure.action
            }
        }
    }

    private func show(status: Int, code: String?, fact: String, action: String) {
        if status == 405 {
            errorFact = "This Factory engine is too old to save runs as templates."
            errorAction = "Update Allternit Desktop or Gizzi, then try again."
            return
        }
        taken = code == "refused" && fact.contains("already exists")
        errorFact = fact
        errorAction = taken ? "Pick another name, or replace the existing template." : action
    }
}

// MARK: - Take over

/// A Terminal bot's live pane (lib/factory/panes.ts): the same pane its wall
/// draws, so typing in either shows in both. Never reconnects on its own after
/// the pane goes or the stream fails; the screen says so and offers Reconnect.
@MainActor
private final class ProjectPaneMirror: ObservableObject {
    enum Status: Equatable {
        case connecting, live
        case ended(String)
    }

    @Published private(set) var status: Status = .connecting

    /// The terminal view's sink (screen, the pane's width); the latest screen
    /// waits here until it attaches.
    var onScreen: (@MainActor (String, Int) -> Void)? {
        didSet { if let latest { onScreen?(latest.text, latest.columns) } }
    }

    private let agentId: String
    private var latest: (text: String, columns: Int)?
    private var streamTask: Task<Void, Never>?
    private var inputQueue: [ProjectPaneInput] = []
    private var isSending = false

    init(agentId: String) { self.agentId = agentId }

    func start() {
        streamTask?.cancel()
        status = .connecting
        let id = agentId
        streamTask = Task { [weak self] in
            do {
                let bytes = try await BotProjectClient.shared.paneStream(agentId: id)
                // Read and decode off the main thread; only frames hop back.
                let reader = Task.detached { [weak self] () throws -> Bool in
                    var event = ""
                    for try await line in bytes.lines {
                        if line.hasPrefix("event:") {
                            event = line.dropFirst(6).trimmingCharacters(in: .whitespaces)
                        } else if line.hasPrefix("data:") {
                            let payload = Data(line.dropFirst(5).trimmingCharacters(in: .whitespaces).utf8)
                            guard let frame = try? JSONDecoder().decode(ProjectPaneFrame.self, from: payload) else { continue }
                            let name = event
                            await self?.apply(event: name, frame: frame)
                            if name == "gone" { return true }
                        }
                    }
                    return false
                }
                let gone = try await withTaskCancellationHandler {
                    try await reader.value
                } onCancel: {
                    reader.cancel()
                }
                guard !gone, !Task.isCancelled else { return }
                self?.status = .ended("The pane stream ended.")
            } catch {
                guard !Task.isCancelled else { return }
                self?.status = .ended(ProjectFactoryFailure(error).fact)
            }
        }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
    }

    private func apply(event: String, frame: ProjectPaneFrame) {
        if event == "gone" {
            status = .ended(frame.reason ?? "The pane closed.")
        } else if event == "screen", let ansi = frame.ansi {
            // Each event is the whole visible screen: reset, no line wrap (the
            // pane has its own width), no cursor (frames carry none), redraw.
            let lines = ansi.replacingOccurrences(of: "\r\n", with: "\n")
            let screen = "\u{1b}c\u{1b}[?7l\u{1b}[?25l" + lines.replacingOccurrences(of: "\n", with: "\r\n")
            let columns = Self.visibleColumns(lines)
            latest = (screen, columns)
            status = .live
            onScreen?(screen, columns)
        }
    }

    /// The widest line, not counting its colour sequences.
    private static func visibleColumns(_ text: String) -> Int {
        var widest = 0
        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            var width = 0
            var inEscape = false
            for scalar in line.unicodeScalars {
                if inEscape {
                    if (0x40...0x7e).contains(scalar.value), scalar != "[" { inEscape = false }
                } else if scalar == "\u{1b}" {
                    inEscape = true
                } else {
                    width += 1
                }
            }
            widest = max(widest, width)
        }
        return widest
    }

    /// Types into the pane, in order, one POST at a time.
    func type(_ data: String) {
        inputQueue.append(contentsOf: ProjectPaneInput.batches(from: data))
        guard !isSending else { return }
        isSending = true
        let id = agentId
        Task {
            defer { isSending = false }
            while !inputQueue.isEmpty {
                let batch = inputQueue.removeFirst()
                do {
                    try await BotProjectClient.shared.paneInput(agentId: id, text: batch.text, keys: batch.keys)
                } catch {
                    inputQueue.removeAll()
                    status = .ended("Couldn't type into the pane. \(FriendlyErrorMessage.from(error.localizedDescription))")
                    stop()
                }
            }
        }
    }
}

private struct ProjectPaneFrame: Decodable, Sendable {
    let ansi: String?
    let reason: String?
}

/// terminalDataToPaneInput: printable runs become `text`, control bytes and
/// escape sequences become the pane engine's named `keys`. Unknown escape
/// sequences are dropped rather than typed as garbage.
private struct ProjectPaneInput: Sendable {
    var text: String?
    var keys: [String]?

    private static let named: [String: String] = [
        "\r": "enter", "\n": "enter", "\t": "tab", "\u{7f}": "backspace", "\u{8}": "backspace", "\u{1b}": "esc",
        "\u{1b}[A": "up", "\u{1b}[B": "down", "\u{1b}[C": "right", "\u{1b}[D": "left",
        "\u{1b}OA": "up", "\u{1b}OB": "down", "\u{1b}OC": "right", "\u{1b}OD": "left", "\u{1b}[Z": "shift+tab",
    ]

    static func batches(from data: String) -> [ProjectPaneInput] {
        // Scalars, not Characters: "\r\n" is one Character but two keystrokes.
        let scalars = Array(data.unicodeScalars)
        var out: [ProjectPaneInput] = []
        var text = String.UnicodeScalarView()
        func string(_ slice: ArraySlice<UnicodeScalar>) -> String {
            var view = String.UnicodeScalarView()
            view.append(contentsOf: slice)
            return String(view)
        }
        func flush() {
            if !text.isEmpty { out.append(ProjectPaneInput(text: String(text), keys: nil)) }
            text = String.UnicodeScalarView()
        }
        func push(_ key: String) {
            flush()
            if let last = out.indices.last, out[last].text == nil {
                out[last].keys?.append(key)
            } else {
                out.append(ProjectPaneInput(text: nil, keys: [key]))
            }
        }
        var i = 0
        while i < scalars.count {
            let scalar = scalars[i]
            if scalar == "\u{1b}" {
                let three = string(scalars[i..<min(i + 3, scalars.count)])
                if three.unicodeScalars.count == 3, let key = named[three] {
                    push(key)
                    i += 3
                    continue
                }
                // CSI with parameters (ESC [ 1 ; 5 A): skip the whole sequence.
                if i + 1 < scalars.count, scalars[i + 1] == "[" {
                    var end = i + 2
                    while end < scalars.count, CharacterSet(charactersIn: "0123456789;").contains(scalars[end]) { end += 1 }
                    if end < scalars.count, CharacterSet.letters.union(CharacterSet(charactersIn: "~")).contains(scalars[end]) {
                        flush()
                        i = end + 1
                        continue
                    }
                }
                if i + 1 < scalars.count, scalars[i + 1].value >= 32, scalars[i + 1].value != 127 {
                    push("alt+\(String(scalars[i + 1]))")
                    i += 2
                    continue
                }
                push("esc")
                i += 1
                continue
            }
            if let key = named[String(scalar)] {
                push(key)
            } else if scalar.value < 32 {
                push("ctrl+\(String(UnicodeScalar(scalar.value + 96) ?? "?"))")
            } else {
                text.append(scalar)
            }
            i += 1
        }
        flush()
        return out
    }
}

/// The Code terminal's surface and key row, wired to a bot's pane.
private struct ProjectBotPaneScreen: View {
    let agent: FactoryAgent

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = BotModeStore.shared
    @StateObject private var mirror: ProjectPaneMirror
    @StateObject private var latch = TerminalCtrlLatch()

    init(agent: FactoryAgent) {
        self.agent = agent
        _mirror = StateObject(wrappedValue: ProjectPaneMirror(agentId: agent.id))
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if case .ended(let reason) = mirror.status {
                    HStack(spacing: 8) {
                        Text(reason)
                            .font(.system(size: 13))
                            .foregroundColor(Theme.statusError)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button("Reconnect") { mirror.start() }
                            .font(.system(size: 13))
                            .frame(minHeight: 44)
                    }
                    .padding(.horizontal, 8)
                    .background(Theme.statusError.opacity(0.12))
                }
                ZStack {
                    TerminalSurfaceView(
                        fontSize: 11,
                        attach: { view in
                            mirror.onScreen = { [weak view] text, columns in
                                guard let view else { return }
                                // The whole pane fits the phone's width: smaller type, not wrapped lines.
                                let fitted = columns > 0 ? (view.bounds.width - 4) / (CGFloat(columns) * 0.6) : 11
                                let size = min(11, max(6, fitted.rounded(.down)))
                                if view.font.pointSize != size {
                                    view.font = UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
                                }
                                view.feed(text: text)
                            }
                        },
                        onInput: { [weak mirror, weak latch] text in
                            mirror?.type(latch?.transform(text) ?? text)
                        },
                        onResize: { _, _ in }
                    )
                    .padding(8)
                    if mirror.status == .connecting {
                        ProgressView().tint(TerminalPalette.secondary)
                    }
                }
                TerminalAccessoryKeys(latch: latch) { data in mirror.type(data) }
            }
            .background(TerminalPalette.background)
            .navigationTitle(store.bot(agent.id)?.botDisplayName ?? agent.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
            }
            .task { mirror.start() }
            .onDisappear { mirror.stop() }
        }
    }
}
