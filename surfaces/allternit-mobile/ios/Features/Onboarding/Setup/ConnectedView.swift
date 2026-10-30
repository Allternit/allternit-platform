import SwiftUI
import UserNotifications

/// The end of setup on the phone (allternit-ai `setup/ConnectedView.tsx`,
/// phone layout): a graph of everything ACTUALLY connected to this account
/// and phone, with the A:// core at the centre, the rest of the workspace as
/// chips, and a detail card for the tapped node. It opens on the live setup
/// checklist; "Continue" reveals the graph. "Open Allternit" plays the
/// branded hand-off (SetupHandoff), which finishes setup at ~3.3 s so the
/// workspace mounts under it. If the hand-off can't run, it finishes at once.
struct SetupConnectedView: View {
    let facts: ConnectedInventory.SessionFacts
    let themeLabel: String
    @ObservedObject var signals: SetupSignals
    var onBack: () -> Void
    /// Finishes setup; `fix` is a Settings section to open once the workspace is up.
    var onOpen: (_ fix: String?) -> Void

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var inventory: ConnectedInventory.Inventory?
    @State private var focus: String?
    @State private var checklistOpen = true
    @State private var handingOff = false
    @State private var graphFrame: CGRect = .zero
    @State private var firstName: String?
    @State private var shown = false

    // Phone canvas: 370×330, hub at (185, 150).
    private static let pw: CGFloat = 370
    private static let ph: CGFloat = 330
    private static let hub = CGPoint(x: 185, y: 150)

    private struct Placed: Identifiable { let node: ConnectedInventory.Node; let at: CGPoint; var id: String { node.id } }

    private var nodes: [ConnectedInventory.Node] {
        inventory.map { ConnectedInventory.buildGraph($0, facts) } ?? []
    }

    private func layout(_ nodes: [ConnectedInventory.Node]) -> [Placed] {
        let brains = nodes.filter { $0.group == .brains }
        var placed = brains.enumerated().map { i, n -> Placed in
            let a = (-90 + 360 / Double(brains.count) * Double(i)) * .pi / 180
            return Placed(node: n, at: CGPoint(x: (Self.hub.x + 108 * cos(a)).rounded(), y: (Self.hub.y + 92 * sin(a)).rounded()))
        }
        let comps = nodes.filter { $0.group == .computers }
        for (i, n) in comps.enumerated() {
            let x: CGFloat = comps.count <= 1 ? Self.hub.x : (48 + CGFloat(i) * (Self.pw - 96) / CGFloat(comps.count - 1)).rounded()
            placed.append(Placed(node: n, at: CGPoint(x: x, y: Self.hub.y + 156)))
        }
        return placed
    }

    var body: some View {
        let all = nodes
        let placed = layout(all)
        let focusId = focus ?? all.first(where: { $0.status == .new })?.id ?? all.first?.id
        let focused = all.first { $0.id == focusId }

        VStack(spacing: 0) {
            header
            graph(placed, focusId: focusId)
            if inventory != nil { groups(all) }
            bottomBar(focused)
        }
        .background(SetupPalette.bg.ignoresSafeArea())
        .scaleEffect(shown || reduceMotion ? 1 : 0.97)
        .opacity(shown || reduceMotion ? 1 : 0)
        .overlay {
            if checklistOpen && !handingOff {
                SetupChecklistCard(onFix: { section in
                    checklistOpen = false
                    open(fix: section, placed: placed)
                }, onContinue: { withAnimation(.easeOut(duration: 0.25)) { checklistOpen = false } })
                .transition(.opacity)
            }
        }
        .task {
            withAnimation(.timingCurve(0.16, 1, 0.3, 1, duration: 0.6)) { shown = true }
            await loadInventory()
        }
        .task {
            // The name on WELCOME: Settings → Profile's preferred name, else the account's first name.
            let preferred = await SetupAPI.preferredName()
            firstName = SetupHandoff.name(from: preferred) ?? SetupHandoff.name(from: AuthManager.shared.firstName)
        }
    }

    private func loadInventory() async {
        if !signals.runtimesSettled { await signals.loadRuntimes() }
        let reachable = signals.runtimes.contains(where: \.isOnline)
        inventory = await ConnectedInventory.load(runtimes: signals.runtimes, reachable: reachable)
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            AllternitOrbView(state: .idle, size: 22)
            Text("Everything connected").font(.system(size: 14, weight: .bold)).foregroundStyle(SetupPalette.ink)
            Spacer()
            Text("SETUP DONE").font(SetupPalette.mono(10.5, .semibold)).tracking(0.8).foregroundStyle(SetupPalette.mut)
        }
        .padding(.horizontal, 16)
        .frame(height: 52)
        .background(SetupPalette.cardGlass)
        .overlay(alignment: .bottom) { SetupPalette.line.frame(height: 1) }
        .opacity(handingOff ? 0 : 1)
        .animation(.easeOut(duration: 0.35), value: handingOff)
    }

    // MARK: Graph

    private func graph(_ placed: [Placed], focusId: String?) -> some View {
        ZStack {
            SetupBlockField()
            if inventory == nil {
                VStack(spacing: 10) {
                    AllternitOrbView(state: .searching, size: 48, label: "A:// is searching")
                    Text("Gathering what's connected…").font(.system(size: 13)).foregroundStyle(SetupPalette.sub)
                }
            } else {
                SetupFitBox(width: Self.pw, height: Self.ph) {
                    ZStack(alignment: .topLeading) {
                        ForEach(Array(placed.enumerated()), id: \.element.id) { i, p in
                            ConnectedEdge(from: Self.hub, to: p.at, on: p.node.status != .avail, delay: 0.1 + Double(i) * 0.03)
                        }
                        ForEach(Array(placed.filter { $0.node.status == .new || $0.node.status == .conn }.prefix(8).enumerated()), id: \.element.id) { i, p in
                            SetupPackets(from: Self.hub, to: p.at, size: 8, period: 2.2)
                                .opacity(handingOff ? 0 : 1)
                                .id("\(p.id)-\(i)")
                        }
                        ZStack {
                            SetupCoreRing(size: 64, minScale: 0.8, maxScale: 1.6, lineWidth: 1.5)
                            AllternitOrbView(state: .working, size: 64, label: "Allternit core")
                        }
                        .position(Self.hub)
                        .opacity(handingOff ? 0 : 1)
                        ForEach(Array(placed.enumerated()), id: \.element.id) { i, p in
                            nodeView(p, i: i, focused: p.id == focusId).position(p.at)
                        }
                    }
                    .frame(width: Self.pw, height: Self.ph, alignment: .topLeading)
                }
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { graphFrame = $0 }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // Small phones: the graph gives way first so the detail and Open stay on screen.
        .frame(minHeight: 180)
        .clipped()
        .animation(.easeOut(duration: 0.35), value: handingOff)
    }

    private func nodeView(_ p: Placed, i: Int, focused: Bool) -> some View {
        let n = p.node
        let lit = n.status != .avail
        return Button { focus = n.id } label: {
            HStack(spacing: 6) {
                Circle()
                    .fill(n.status == .avail ? SetupPalette.lineDash : n.status == .inc ? SetupPalette.ink : SetupPalette.ok)
                    .frame(width: 7, height: 7)
                Text(n.short).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            }
            .foregroundStyle(lit ? SetupPalette.ink : SetupPalette.sub)
            .padding(.horizontal, n.brain ? 9 : 10)
            .frame(minWidth: n.brain ? 84 : nil)
            .frame(height: n.brain ? 32 : 30)
            .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 10))
            .overlay {
                if lit {
                    RoundedRectangle(cornerRadius: 10).stroke(n.status == .inc ? SetupPalette.line : SetupPalette.ink, lineWidth: 1)
                } else {
                    RoundedRectangle(cornerRadius: 10).stroke(SetupPalette.lineDash, style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
                }
            }
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(SetupPalette.coral.opacity(focused ? 0.35 : n.status == .new ? 0.28 : 0), lineWidth: 3)
                    .padding(-2)
            )
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(n.title), \(n.statusText)")
        .accessibilityAddTraits(focused ? .isSelected : [])
        .modifier(SetupPopIn(delay: 0.2 + Double(i) * 0.04))
        .opacity(handingOff ? 0 : 1)
        .scaleEffect(handingOff ? 0.6 : 1)
        .animation(.easeIn(duration: 0.14), value: handingOff)
    }

    // MARK: Groups + detail

    private func groups(_ all: [ConnectedInventory.Node]) -> some View {
        let items = [ConnectedInventory.NodeGroup.bots, .channels, .workspace].flatMap { g in all.filter { $0.group == g } }
        return SetupFlowLayout(spacing: 6) {
            ForEach(items) { n in
                Button { focus = n.id } label: {
                    HStack(spacing: 5) {
                        Text(n.short).lineLimit(1)
                        if n.status != .avail { Text("✓").fontWeight(.bold).foregroundStyle(SetupPalette.ink) }
                    }
                    .font(.system(size: 11.5))
                    .foregroundStyle(SetupPalette.sub)
                    .padding(.horizontal, 9)
                    .frame(height: 28)
                    .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        if n.status == .avail {
                            RoundedRectangle(cornerRadius: 8).stroke(SetupPalette.lineDash, style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
                        } else {
                            RoundedRectangle(cornerRadius: 8).stroke(SetupPalette.line)
                        }
                    }
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(n.title), \(n.statusText)")
            }
        }
        .padding(.horizontal, 14)
        .opacity(handingOff ? 0 : 1)
        .animation(.easeOut(duration: 0.35), value: handingOff)
    }

    private func bottomBar(_ focused: ConnectedInventory.Node?) -> some View {
        VStack(spacing: 8) {
            if let n = focused {
                VStack(alignment: .leading, spacing: 4) {
                    Text(n.group.rawValue.uppercased()).font(.system(size: 10.5)).tracking(0.8).foregroundStyle(SetupPalette.mut)
                    Text(n.title).font(.system(size: 16, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                    Text(n.desc).font(.system(size: 12.5)).foregroundStyle(SetupPalette.sub).lineLimit(4).fixedSize(horizontal: false, vertical: true)
                    Text(n.statusText)
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(n.status == .avail ? SetupPalette.sub : SetupPalette.okFg)
                        .padding(.horizontal, 9)
                        .frame(height: 22)
                        .background(n.status == .avail ? SetupPalette.fill : SetupPalette.okBg, in: Capsule())
                        .padding(.top, 6)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 14)
                .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 14))
                .overlay(RoundedRectangle(cornerRadius: 14).stroke(SetupPalette.line))
                .id(n.id)
                .transition(.opacity.combined(with: .scale(scale: 0.94)))
                .accessibilityElement(children: .combine)
            }
            HStack(spacing: 8) {
                Button("Back", action: onBack).buttonStyle(SetupGhostButtonStyle()).disabled(handingOff)
                Spacer()
                Button {
                    open(fix: nil, placed: layout(nodes))
                } label: {
                    HStack(spacing: 8) {
                        Text(handingOff ? "Opening…" : "Open Allternit")
                        if !handingOff { Image(systemName: "arrow.right").font(.system(size: 13, weight: .semibold)) }
                    }
                }
                .buttonStyle(SetupPrimaryButtonStyle())
                .fixedSize()
                .disabled(handingOff)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .opacity(handingOff ? 0 : 1)
        .animation(.easeOut(duration: 0.2), value: focus)
        .animation(.easeOut(duration: 0.35), value: handingOff)
    }

    // MARK: Hand-off

    private func open(fix: String?, placed: [Placed]) {
        guard !handingOff else { return }
        // Connected nodes (set up now or already) collapse into the mark's
        // blocks from where they sit on screen; the rest fly in from afar.
        let scale = graphFrame.width > 0 ? min(graphFrame.width / Self.pw, graphFrame.height / Self.ph, 1.15) : 0
        let origin = CGPoint(x: graphFrame.midX - Self.pw * scale / 2, y: graphFrame.midY - Self.ph * scale / 2)
        let live = placed.filter { $0.node.status != .avail }
        let sources = (live.isEmpty ? placed : live).prefix(23).map {
            CGPoint(x: origin.x + $0.at.x * scale, y: origin.y + $0.at.y * scale)
        }
        let started = scale > 0 && SetupHandoff.start(
            sources: Array(sources), name: firstName, dark: colorScheme == .dark, reduced: reduceMotion,
            finish: { onOpen(fix) })
        if started { handingOff = true } else { onOpen(fix) }
    }
}

/// An edge from the hub: dashed for available, solid ink for connected (grows in).
private struct ConnectedEdge: View {
    var from: CGPoint
    var to: CGPoint
    var on: Bool
    var delay: Double
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = false

    var body: some View {
        Path { p in p.move(to: from); p.addLine(to: to) }
            .stroke(on ? SetupPalette.ink : SetupPalette.edge,
                    style: StrokeStyle(lineWidth: on ? 2 : 1, dash: on ? [10, 4] : [4, 5]))
            .opacity(shown || reduceMotion ? (on ? 0.9 : 1) : 0)
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeOut(duration: 0.6).delay(delay)) { shown = true }
            }
            .allowsHitTesting(false)
    }
}

/// Wrapping row of chips (the web's flex-wrap).
struct SetupFlowLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, row: CGFloat = 0, maxX: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width { x = 0; y += row + spacing; row = 0 }
            x += size.width + spacing
            maxX = max(maxX, x - spacing)
            row = max(row, size.height)
        }
        return CGSize(width: proposal.width ?? maxX, height: y + row)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, row: CGFloat = 0
        for s in subviews {
            let size = s.sizeThatFits(.unspecified)
            if x > bounds.minX && x + size.width > bounds.maxX { x = bounds.minX; y += row + spacing; row = 0 }
            s.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            row = max(row, size.height)
        }
    }
}

// MARK: - Setup checklist

/// The live-checked setup checklist (the same rows as Settings › Setup).
/// A row is checked only when its live check passed. A row that needs you
/// has one button that opens the Settings page that fixes it once setup is
/// finished; "Check again" re-runs every check.
struct SetupChecklistCard: View {
    var onFix: (String) -> Void
    var onContinue: () -> Void

    private enum Phase: Equatable { case loading, ready, error(String) }
    @State private var phase: Phase = .loading
    @State private var rows: [SetupCheckRow] = []
    @State private var run = 0

    var body: some View {
        ZStack {
            Color.black.opacity(0.38).ignoresSafeArea()
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    let ready = rows.filter { $0.status == .ready }.count
                    let blocking = rows.filter { $0.required && $0.status != .ready }.count
                    HStack(spacing: 10) {
                        ZStack {
                            Circle().stroke(SetupPalette.fill2, lineWidth: 3)
                            Circle().trim(from: 0, to: rows.isEmpty ? 0 : CGFloat(ready) / CGFloat(rows.count))
                                .stroke(SetupPalette.ok, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                                .rotationEffect(.degrees(-90))
                                .animation(.easeOut(duration: 0.5), value: ready)
                        }
                        .frame(width: 26, height: 26)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(!rows.isEmpty && blocking == 0 ? "Everything you need is ready" : "Check your setup")
                                .font(.system(size: 15, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                            Text("\(ready) of \(rows.count) ready").font(.system(size: 12)).foregroundStyle(SetupPalette.sub)
                        }
                    }
                    if phase == .loading && rows.isEmpty {
                        Text("Running checks…").font(.system(size: 13)).foregroundStyle(SetupPalette.sub).padding(.vertical, 8)
                    }
                    if case .error(let message) = phase {
                        Text("Allternit could not run the checks: \(message)").font(.system(size: 13)).foregroundStyle(SetupPalette.errFg)
                    }
                    ForEach(rows) { row in rowView(row) }
                    HStack(spacing: 8) {
                        Spacer()
                        Button(phase == .loading ? "Checking…" : "Check again") { Task { await recheck() } }
                            .buttonStyle(SetupGhostButtonStyle())
                            .disabled(phase == .loading)
                        Button("Continue", action: onContinue)
                            .buttonStyle(SetupPrimaryButtonStyle())
                            .fixedSize()
                    }
                }
                .padding(14)
                .background(SetupPalette.card, in: RoundedRectangle(cornerRadius: 16))
                .overlay(RoundedRectangle(cornerRadius: 16).stroke(SetupPalette.line))
                .padding(16)
                .frame(maxWidth: 520)
                .frame(maxWidth: .infinity)
            }
            .scrollBounceBehavior(.basedOnSize)
            .defaultScrollAnchor(.center)
        }
        .accessibilityAddTraits(.isModal)
        .task { await recheck() }
    }

    private func rowView(_ row: SetupCheckRow) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: row.status == .ready ? "checkmark.circle.fill" : "circle")
                .font(.system(size: 18))
                .foregroundStyle(row.status == .ready ? SetupPalette.ok : SetupPalette.lineDash)
            VStack(alignment: .leading, spacing: 3) {
                Text(row.title + (row.required ? "" : " (optional)"))
                    .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(SetupPalette.ink)
                Text("\(row.status.label). \(row.detail)")
                    .font(.system(size: 12.5)).foregroundStyle(SetupPalette.sub)
                    .fixedSize(horizontal: false, vertical: true)
                if row.status != .ready, let label = row.fixLabel, let section = row.fixSection {
                    Button(label) { onFix(section) }
                        .buttonStyle(SetupInlineButtonStyle())
                        .padding(.top, 4)
                }
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }

    private func recheck() async {
        run += 1
        let mine = run
        phase = .loading
        let device = await Self.notificationsRow()
        do {
            let server = try await SetupAPI.setupChecks()
            guard mine == run else { return }
            rows = SetupAPI.sortChecks(server + [device])
            phase = .ready
        } catch {
            guard mine == run else { return }
            // On a failed request the device row still shows, with the error beside it.
            rows = [device]
            phase = .error(error.localizedDescription.isEmpty ? "The checks could not be run." : error.localizedDescription)
        }
    }

    /// Notification permission on this phone (only the device can answer).
    static func notificationsRow() async -> SetupCheckRow {
        let status: SetupCheckStatus
        let detail: String
        switch await PushNotificationManager.shared.authorizationStatus() {
        case .authorized, .provisional, .ephemeral:
            status = .ready; detail = "Allternit can notify you on this phone."
        case .denied:
            status = .needsSetup; detail = "Notifications are blocked for Allternit. Allow them in iOS Settings."
        default:
            status = .notChecked; detail = "Allternit has not asked to notify you on this phone yet."
        }
        return SetupCheckRow(id: "notifications", title: "Notifications", required: false, status: status, detail: detail,
                             fixLabel: "Open notifications", fixSection: "notifications")
    }
}
