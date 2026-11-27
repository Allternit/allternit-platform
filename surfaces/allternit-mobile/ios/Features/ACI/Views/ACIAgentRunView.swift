import SwiftUI

/// Port of the web viewport's `KIND_COLOR` table
/// (surfaces/ai.allternit.com/src/capsules/browser/ACIComputerUseView.tsx:53-61).
/// Lookup is on the lowercased action kind; unknown kinds — including the
/// ActionContract types without a dedicated color (select/wait/assert/…) —
/// fall back to the teal default, exactly like the web.
enum ACIActionPalette {
    static func color(for kind: String?) -> Color {
        switch kind?.lowercased() {
        case "type": return Color(hex: "#A855F7")
        case "scroll": return Color(hex: "#FBBF24")
        case "read": return Color(hex: "#3B82F6")
        case "navigate": return Color(hex: "#22C55E")
        case "extract": return Color(hex: "#F97316")
        // "click" and the default share the same teal on the web.
        default: return Color(hex: "#63FCF1")
        }
    }
}

/// One line of the run's action log.
struct ACITraceLine: Identifiable, Sendable {
    let id: Int
    let text: String
    /// Lowercase action kind for color-coding; nil for plain trace messages.
    let kind: String?
}

/// State holder for `ACIAgentRunView` — owns the run lifecycle
/// (start → stream → stop/approve) against `ACIAgentClient`, mirroring the
/// web store's runGoal reducer (browserAgent.store.ts:546-660).
@MainActor
final class ACIAgentRunViewModel: ObservableObject {
    @Published private(set) var status: ACIRunStatus = .idle
    @Published private(set) var lastMessage: String? = nil
    @Published private(set) var adapterId: String? = nil
    @Published private(set) var stepIndex: Int? = nil
    @Published private(set) var totalSteps: Int? = nil
    @Published private(set) var currentAction: ACIEvent.ACIAction? = nil
    @Published private(set) var screenshot: UIImage? = nil
    @Published private(set) var traceLines: [ACITraceLine] = []
    /// Set when the run POST or the stream fails; drives the retry affordance.
    @Published private(set) var errorMessage: String? = nil
    /// The bot's latest pointer step (computer toolset events).
    @Published private(set) var agentAction: ACIAgentAction? = nil
    /// Set when the run is waiting on the owner; drives ACIRunGateSheet.
    @Published var gate: ACIRunGate? = nil

    let goal: String
    /// The computer whose toolset events are followed; nil = the active computer.
    private(set) var computerId: String?
    /// A run that already exists (push / deep link): follow it, do not start one.
    private(set) var attachRunId: String?
    /// True while this person holds control of the computer ("You have control").
    @Published private(set) var controlHeld = false
    @Published private(set) var controlBusy = false
    @Published private(set) var controlError: String? = nil
    /// Set once the run has its own display (Linux): frames come from `screenshot?run_id=`.
    private var displayRunId: String?
    private var frameTask: Task<Void, Never>?

    private let client: ACIAgentClient
    private var runId: String? = nil
    private var streamTask: Task<Void, Never>? = nil
    private var eventsTask: Task<Void, Never>? = nil
    private var resolvedComputerId: String? = nil
    private var traceCounter = 0
    private var hasStarted = false

    init(goal: String, computerId: String? = nil, attachRunId: String? = nil,
         client: ACIAgentClient = ACIAgentClient()) {
        self.goal = goal
        self.computerId = computerId
        self.attachRunId = attachRunId
        self.client = client
        if let attachRunId {
            runId = attachRunId
            status = .running
        }
    }

    /// Name on the badge and in the working line.
    var botName: String { agentAction?.actor?.name ?? "Your bot" }
    /// Step for the working line: the event's own step, else the state frame's.
    var currentStep: Int? { agentAction?.step ?? stepIndex }

    var requiresApproval: Bool { status == .waitingApproval }

    /// Anything before a terminal state — idle here means "connecting".
    var isActive: Bool {
        switch status {
        case .idle, .running, .waitingApproval, .unknown: return true
        case .blocked, .done, .error: return false
        }
    }

    // MARK: - Lifecycle

    /// POSTs the run and consumes the SSE stream until `done`, connection
    /// end, or cancellation. Idempotent — the view's `onAppear` can fire more
    /// than once.
    func start() {
        guard !hasStarted else { return }
        hasStarted = true

        startEvents()
        streamTask = Task { [weak self] in
            guard let self else { return }
            do {
                let sessionId: String
                if let attached = self.attachRunId {
                    sessionId = attached
                } else {
                    let run = try await self.client.startRun(goal: self.goal)
                    self.runId = run.sessionId
                    if self.adapterId == nil { self.adapterId = run.adapterId }
                    sessionId = run.sessionId
                }

                for try await event in self.client.stream(runId: sessionId) {
                    self.apply(event)
                }

                // Connection ended without a terminal frame — the web treats
                // a dropped EventSource on a Running agent as Done
                // (browserAgent.store.ts:652-655).
                if self.isActive { self.status = .done }
            } catch is CancellationError {
                // Intentional cancel (stop / leaving the view) stays silent.
            } catch let error as URLError where error.code == .cancelled {
                // URLSession surfaces task cancellation as URLError.cancelled.
            } catch {
                // A followed run may not be on the ACI stream (a browser run
                // from the extension): its computer events still drive the view.
                if self.attachRunId != nil { return }
                self.errorMessage = error.localizedDescription
                self.status = .error
            }
        }
    }

    // MARK: - Agent browser events

    /// Follows `/computers/:id/toolset/events` for the cursor and the gates;
    /// reconnects after a drop until the view goes away.
    private func startEvents() {
        eventsTask?.cancel()
        eventsTask = Task { [weak self] in
            guard let self else { return }
            let computer: String?
            if let id = self.computerId { computer = id }
            else { computer = await RuntimeRelayStore.shared.resolveActiveRuntimeId() }
            guard let computer else { return }
            self.resolvedComputerId = computer
            while !Task.isCancelled {
                do {
                    for try await event in self.client.computerEvents(computerId: computer, runtimeId: self.computerId) {
                        self.apply(computerEvent: event)
                    }
                } catch is CancellationError {
                    return
                } catch {
                    // Dropped or not reachable: try again shortly.
                }
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    /// Ignores events of another run when this view follows a known one.
    private func isThisRun(_ id: String?) -> Bool {
        guard let id, let runId else { return true }
        return id == runId
    }

    func apply(computerEvent event: ACIComputerEvent) {
        switch event {
        case .action(let action):
            guard isThisRun(action.runId) else { return }
            agentAction = action
            if status == .idle { status = .running }
        case .confirmationRequired(let request):
            guard isThisRun(request.runId) else { return }
            gate = .approval(request)
        case .humanRequested(let request):
            guard isThisRun(request.runId) else { return }
            gate = .needsYou(request)
        case .humanResumed(let id):
            guard isThisRun(id) else { return }
            if case .needsYou = gate { gate = nil }
        case .blockDetected(let block):
            guard isThisRun(block.runId) else { return }
            gate = .blocked(block)
        case .blockResolved(let id, _, _):
            guard isThisRun(id) else { return }
            if case .blocked = gate { gate = nil }
        case .agentDisplay(let id):
            guard isThisRun(id) else { return }
            displayRunId = id ?? runId
            startFrames()
        }
    }

    // MARK: Gate answers

    func approveGate(scope: String?) {
        guard case .approval(let request) = gate else { return }
        gate = nil
        let client = self.client, runtime = computerId
        Task { [client] in try? await client.approveHandoff(approvalId: request.approvalId, scope: scope, runtimeId: runtime) }
    }

    func denyGate() {
        guard case .approval(let request) = gate else { return }
        gate = nil
        let client = self.client, runtime = computerId
        Task { [client] in try? await client.denyHandoff(approvalId: request.approvalId, runtimeId: runtime) }
    }

    /// Sends the typed secret to the masked box. The value is passed straight
    /// to the request and never stored or logged here; the caller clears its field.
    func submitSecret(_ text: String, saveAs: String?) {
        guard case .needsYou = gate, !text.isEmpty else { return }
        gate = nil
        guard let computer = resolvedComputerId ?? computerId else { return }
        let client = self.client, runtime = computerId
        Task { [client] in
            try? await client.submitSecret(computerId: computer, text: text, saveAs: saveAs, runtimeId: runtime)
        }
    }

    func continueGate() {
        let current = gate
        gate = nil
        guard let computer = resolvedComputerId ?? computerId else { return }
        let client = self.client, runtime = computerId
        Task { [client] in
            switch current {
            case .blocked:
                try? await client.resolveBlock(computerId: computer, action: "continue", runtimeId: runtime)
            default:
                try? await client.humanDone(computerId: computer, runtimeId: runtime)
            }
        }
    }

    func stopFromGate() {
        let current = gate
        gate = nil
        if case .blocked = current, let computer = resolvedComputerId ?? computerId {
            let client = self.client, runtime = computerId
            Task { [client] in try? await client.resolveBlock(computerId: computer, action: "stop", runtimeId: runtime) }
        }
        if case .needsYou(let request) = current, request.secret, let computer = resolvedComputerId ?? computerId {
            let client = self.client, runtime = computerId
            Task { [client] in try? await client.cancelSecret(computerId: computer, runtimeId: runtime) }
        }
        stop()
    }

    /// "Re-run on <target>": a fresh run there with `rerun_of` set. The block
    /// is answered with `rerun` so this computer's run ends.
    func rerun(on target: ACIRerunTarget) {
        guard case .blocked(let block) = gate else { return }
        gate = nil
        let client = self.client, runtime = computerId
        let goal = self.goal, previous = block.runId ?? runId
        let computer = resolvedComputerId ?? computerId
        Task { [weak self, client] in
            if let computer {
                try? await client.resolveBlock(computerId: computer, action: "rerun",
                                               rerunTarget: target.computerId, runtimeId: runtime)
            }
            do {
                let newRun = try await client.startComputerRun(computerId: target.computerId, runtimeId: target.runtimeId ?? target.computerId,
                                                            task: goal, rerunOf: previous)
                await MainActor.run {
                    self?.lastMessage = "Re-running on \(target.name)"
                    if let newRun { self?.open(runId: newRun, on: target.runtimeId ?? target.computerId) }
                }
            } catch ACIRunError.computerUnavailable {
                await MainActor.run { self?.lastMessage = "Your Mac isn't reachable" }
            } catch {
                await MainActor.run { self?.lastMessage = "Couldn't start the re-run on \(target.name)" }
            }
        }
    }

    /// Points this view at another run (the re-run) and follows it.
    private func open(runId newRun: String, on computer: String) {
        streamTask?.cancel(); streamTask = nil
        eventsTask?.cancel(); eventsTask = nil
        frameTask?.cancel(); frameTask = nil
        if controlHeld, let old = resolvedComputerId ?? computerId {
            let client = self.client, runtime = computerId
            Task { [client] in try? await client.releaseControl(computerId: old, runtimeId: runtime) }
        }
        controlHeld = false
        displayRunId = nil
        computerId = computer
        resolvedComputerId = computer
        attachRunId = newRun
        runId = newRun
        gate = nil
        agentAction = nil
        screenshot = nil
        traceLines = []
        errorMessage = nil
        status = .running
        hasStarted = false
        start()
    }

    // MARK: Take over

    /// Take over: the bot's calls are refused while this person holds control.
    func takeOver() {
        guard !controlHeld, !controlBusy, let computer = resolvedComputerId ?? computerId else { return }
        controlBusy = true
        controlError = nil
        let client = self.client, runtime = computerId
        Task { [weak self, client] in
            do {
                try await client.takeControl(computerId: computer, runtimeId: runtime)
                await MainActor.run { self?.controlHeld = true; self?.controlBusy = false; self?.startFrames() }
            } catch {
                await MainActor.run { self?.controlError = "Couldn't take control."; self?.controlBusy = false }
            }
        }
    }

    func handBack() {
        guard controlHeld, let computer = resolvedComputerId ?? computerId else { return }
        controlHeld = false
        let client = self.client, runtime = computerId
        Task { [client] in try? await client.releaseControl(computerId: computer, runtimeId: runtime) }
    }

    /// A tap on the live view, as a fraction (0...1) of the screen.
    func tap(fx: Double, fy: Double) { sendMouse("click", fx: fx, fy: fy) }

    /// A swipe on the live view: "up" scrolls the page down, like a finger.
    func scroll(fx: Double, fy: Double, up: Bool) {
        sendMouse("scroll", fx: fx, fy: fy, button: up ? "down" : "up", amount: 3)
    }

    func type(_ text: String) {
        guard controlHeld, !text.isEmpty, let computer = resolvedComputerId ?? computerId else { return }
        let client = self.client, runtime = computerId
        Task { [client] in try? await client.keyboard(computerId: computer, text: text, runtimeId: runtime) }
    }

    func press(key: String) {
        guard controlHeld, let computer = resolvedComputerId ?? computerId else { return }
        let client = self.client, runtime = computerId
        Task { [client] in try? await client.keyboard(computerId: computer, key: key, runtimeId: runtime) }
    }

    private func sendMouse(_ action: String, fx: Double, fy: Double, button: String? = nil, amount: Int? = nil) {
        guard controlHeld, let computer = resolvedComputerId ?? computerId else { return }
        // The API takes screen pixels: the bot's screen size, else the frame's.
        let w = agentAction?.screenW ?? Double(screenshot.map { $0.size.width * $0.scale } ?? 0)
        let h = agentAction?.screenH ?? Double(screenshot.map { $0.size.height * $0.scale } ?? 0)
        guard w > 1, h > 1 else { return }
        let x = Int((min(max(fx, 0), 1) * w).rounded()), y = Int((min(max(fy, 0), 1) * h).rounded())
        let client = self.client, runtime = computerId
        Task { [client] in
            try? await client.mouse(computerId: computer, action: action, x: x, y: y,
                                    button: button, amount: amount, runtimeId: runtime)
        }
    }

    /// Polls the computer's screen (or the run's own display on Linux) while the
    /// live view needs frames the run stream does not carry.
    private func startFrames() {
        frameTask?.cancel()
        frameTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                if let computer = self.resolvedComputerId ?? self.computerId,
                   let data = try? await self.client.screenshot(computerId: computer, runId: self.displayRunId,
                                                                runtimeId: self.computerId),
                   let image = UIImage(data: data) {
                    self.screenshot = image
                }
                try? await Task.sleep(for: .milliseconds(self.controlHeld ? 700 : 1500))
            }
        }
    }

    /// Stop button: cancels the local consumer and stops the run server-side.
    func stop() {
        streamTask?.cancel()
        streamTask = nil
        eventsTask?.cancel()
        eventsTask = nil
        frameTask?.cancel()
        frameTask = nil
        handBack()
        guard isActive else { return }
        status = .done
        lastMessage = "Stopped"
        guard let runId else { return }
        // Fire-and-forget like the web (`.catch(() => {})`). The client is
        // captured strongly so the POST still fires if the view (and this
        // view model) is torn down right after Stop/leave.
        let client = self.client
        Task { [client] in try? await client.stop(runId: runId) }
    }

    /// Leaving the run view abandons the run — the web closes its
    /// EventSource on navigation; we additionally stop the server run so the
    /// agent doesn't keep driving a browser nobody is watching.
    func leave() {
        // A run followed from a push or link belongs to someone else's
        // session: closing the sheet only stops watching.
        guard isActive, attachRunId == nil else {
            streamTask?.cancel()
            streamTask = nil
            eventsTask?.cancel()
            eventsTask = nil
            frameTask?.cancel()
            frameTask = nil
            handBack()
            return
        }
        stop()
    }

    /// Approve/Deny buttons for a `WaitingApproval` run. Optimistically back
    /// to Running; the next state frame corrects us if the server disagrees.
    func approve(deny: Bool) {
        guard let runId, requiresApproval else { return }
        status = .running
        currentAction = nil
        let client = self.client
        Task { [client] in try? await client.approve(runId: runId, deny: deny) }
    }

    /// Error-state retry affordance: fresh run for the same goal.
    func retry() {
        streamTask?.cancel()
        streamTask = nil
        eventsTask?.cancel()
        eventsTask = nil
        runId = attachRunId
        errorMessage = nil
        status = .idle
        hasStarted = false
        start()
    }

    // MARK: - Frame handling (browserAgent.store.ts:578-650)

    private func apply(_ event: ACIEvent) {
        switch event {
        case .state(let frame):
            if frame.status != nil { status = ACIRunStatus(frame.status) }
            if let message = frame.lastMessage { lastMessage = message }
            if let adapter = frame.adapterId { adapterId = adapter }
            if let step = frame.stepIndex { stepIndex = step }
            if let total = frame.totalSteps { totalSteps = total }
            // Present-but-null clears the action; absent leaves it.
            if frame.hasCurrentAction {
                currentAction = frame.currentAction
                if let action = frame.currentAction {
                    appendTrace(action.label ?? action.selector ?? action.type ?? "Action",
                                kind: action.type)
                }
            }

        case .screenshot(let base64):
            // Undecodable frames just leave the previous image up.
            if let data = Data(base64Encoded: base64), let image = UIImage(data: data) {
                screenshot = image
            }

        case .trace(let frame):
            if let message = frame.message {
                lastMessage = message
                appendTrace(message, kind: nil)
            }
            if let adapter = frame.adapterId { adapterId = adapter }

        case .done:
            if isActive { status = .done }

        case .ignored:
            break
        }
    }

    private func appendTrace(_ text: String, kind: String?) {
        guard !text.isEmpty, traceLines.last?.text != text else { return }
        traceCounter += 1
        traceLines.append(ACITraceLine(id: traceCounter, text: text, kind: kind))
        if traceLines.count > 100 {
            traceLines.removeFirst(traceLines.count - 100)
        }
    }
}

/// The Kimi Computer-style live ACI viewport, ported from
/// ACIComputerUseView.tsx: top status strip, aspect-fit live screenshot with
/// color-coded element highlights, scanline overlay, action trace log, and
/// Stop / Approve / Deny controls.
struct ACIAgentRunView: View {
    let goal: String
    let onExit: () -> Void

    @StateObject private var viewModel: ACIAgentRunViewModel

    private let theme = ModeTheme(mode: .browser)

    /// `computerId` + `runId` attach to a run that is already going (a push
    /// or `/computers/<id>?run=<run_id>` link); a goal starts a new one.
    init(goal: String, computerId: String? = nil, runId: String? = nil, onExit: @escaping () -> Void) {
        self.goal = goal
        self.onExit = onExit
        _viewModel = StateObject(wrappedValue: ACIAgentRunViewModel(goal: goal, computerId: computerId, attachRunId: runId))
    }

    var body: some View {
        VStack(spacing: 0) {
            topStrip
            viewport
            workingLine
            traceLog
            controlBar
        }
        .background(Color("BgPrimary"))
        .onAppear { viewModel.start() }
        .onDisappear { viewModel.leave() }
        .sheet(item: $viewModel.gate) { gate in
            ACIRunGateSheet(
                gate: gate,
                botName: viewModel.botName,
                onApprove: { viewModel.approveGate(scope: $0) },
                onDeny: { viewModel.denyGate() },
                onContinue: { viewModel.continueGate() },
                onStop: { viewModel.stopFromGate() },
                onRerun: { viewModel.rerun(on: $0) },
                onSecret: { viewModel.submitSecret($0, saveAs: $1) }
            )
        }
    }

    // MARK: - Top strip (ACIComputerUseView.tsx TopStrip)

    private var statusDotColor: Color {
        switch viewModel.status {
        case .running: return Theme.statusSuccess
        case .waitingApproval: return Theme.statusWarning
        case .done: return Theme.statusInfo
        case .blocked, .error: return Color(hex: "#EF4444")
        case .idle, .unknown: return Color("TextSecondary")
        }
    }

    private var isPulsing: Bool {
        viewModel.status == .running || viewModel.status == .waitingApproval
    }

    private var statusMessage: String {
        if let message = viewModel.lastMessage, !message.isEmpty { return message }
        if !goal.isEmpty { return goal }
        return viewModel.status == .done ? "Task complete" : "Waiting…"
    }

    /// Web adapter label: drop the first dotted segment, falling back to the
    /// full id (`adapterId.split('.').slice(1).join('.') || adapterId`).
    private var adapterLabel: String? {
        guard let adapterId = viewModel.adapterId else { return nil }
        let tail = adapterId.split(separator: ".").dropFirst().joined(separator: ".")
        return tail.isEmpty ? adapterId : tail
    }

    private var topStrip: some View {
        HStack(spacing: 8) {
            Button(action: onExit) {
                Label("Close", systemImage: "chevron.left")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusSM)
                            .stroke(Color("BorderSubtle"), lineWidth: 1)
                    )
            }
            .buttonStyle(.plain)

            Circle()
                .fill(statusDotColor)
                .frame(width: 7, height: 7)
                .shadow(color: isPulsing ? statusDotColor.opacity(0.67) : .clear, radius: 3)
                .phaseAnimator([false, true]) { content, phase in
                    content.opacity(isPulsing ? (phase ? 0.45 : 1) : 1)
                } animation: { _ in
                    .easeInOut(duration: 0.9)
                }

            Text("COMPUTER USE")
                .font(.system(size: 10, weight: .bold, design: .monospaced))
                .tracking(1.2)
                .foregroundColor(Theme.borderWarmStrong)

            stripDivider

            Text(statusMessage)
                .font(.system(size: 11, weight: viewModel.requiresApproval ? .semibold : .regular))
                .foregroundColor(viewModel.requiresApproval
                                 ? Color(hex: "#FDE68A")
                                 : Color("TextSecondary"))
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let step = viewModel.stepIndex,
               let total = viewModel.totalSteps, total > 1 {
                stripDivider
                Text("\(step)/\(total)")
                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                    .foregroundColor(Theme.borderWarmStrong)
            }

            if let adapterLabel {
                stripDivider
                Text(adapterLabel)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundColor(Color("TextSecondary").opacity(0.6))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 34)
        .background(Theme.glassBgThick)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Theme.borderWarmSubtle).frame(height: 1)
        }
    }

    private var stripDivider: some View {
        Rectangle()
            .fill(Color("BorderSubtle"))
            .frame(width: 1, height: 12)
    }

    // MARK: - Viewport (screenshot + overlays)

    private var viewport: some View {
        GeometryReader { geometry in
            ZStack {
                if let errorMessage = viewModel.errorMessage {
                    errorState(errorMessage)
                } else if let screenshot = viewModel.screenshot {
                    screenshotContent(screenshot, in: geometry.size)
                } else if let action = viewModel.agentAction {
                    screenPanel(for: action, in: geometry.size)
                } else if isConnecting {
                    connectingState
                } else {
                    noVideoState
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay { scanlines }
        }
        .clipped()
    }

    /// Aspect-fit math mirroring the web's img metrics: fit the screenshot
    /// into the container, center it, and scale element rects from
    /// screenshot pixel space into view space with the same factor.
    private func screenshotContent(_ image: UIImage, in size: CGSize) -> some View {
        let natural = image.size
        let scale = min(size.width / max(natural.width, 1),
                        size.height / max(natural.height, 1))
        let displayW = natural.width * scale
        let displayH = natural.height * scale
        let offsetX = (size.width - displayW) / 2
        let offsetY = (size.height - displayH) / 2

        return ZStack(alignment: .topLeading) {
            Image(uiImage: image)
                .resizable()
                .frame(width: displayW, height: displayH)
                .clipShape(RoundedRectangle(cornerRadius: 3))
                .overlay(
                    RoundedRectangle(cornerRadius: 3)
                        .stroke(Theme.borderWarmSubtle, lineWidth: 1)
                )
                .offset(x: offsetX, y: offsetY)

            if viewModel.controlHeld {
                takeOverLayer(frame: CGRect(x: offsetX, y: offsetY, width: displayW, height: displayH))
            }

            if let box = viewModel.currentAction?.boundingBox {
                highlightBox(box, scale: scale, offsetX: offsetX, offsetY: offsetY)
            }

            // The cursor sits in screen_w x screen_h space; scale it onto the
            // image as drawn (the image's own pixels may differ from the screen's).
            if let action = viewModel.agentAction {
                agentCursor(action, frame: CGRect(x: offsetX, y: offsetY, width: displayW, height: displayH))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// While you hold control: a tap clicks there, a swipe scrolls. `frame` is
    /// where the screen is drawn; positions go to the API as screen fractions.
    private func takeOverLayer(frame: CGRect) -> some View {
        Color.clear
            .frame(width: frame.width, height: frame.height)
            .contentShape(Rectangle())
            .overlay(Rectangle().stroke(PolishTokens.ink.opacity(0.5), lineWidth: 2))
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .local).onEnded { value in
                    let fx = value.location.x / max(frame.width, 1)
                    let fy = value.location.y / max(frame.height, 1)
                    if abs(value.translation.height) > 14 {
                        viewModel.scroll(fx: value.startLocation.x / max(frame.width, 1),
                                         fy: value.startLocation.y / max(frame.height, 1),
                                         up: value.translation.height < 0)
                    } else {
                        viewModel.tap(fx: fx, fy: fy)
                    }
                }
            )
            .offset(x: frame.minX, y: frame.minY)
            .accessibilityLabel("Live screen. Tap to click, swipe to scroll.")
    }

    /// No video frame yet: a neutral screen-shaped panel so the cursor still
    /// shows where the bot is working.
    private func screenPanel(for action: ACIAgentAction, in size: CGSize) -> some View {
        let scale = min(size.width / action.screenW, size.height / action.screenH)
        let w = action.screenW * scale, h = action.screenH * scale
        let frame = CGRect(x: (size.width - w) / 2, y: (size.height - h) / 2, width: w, height: h)
        return ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(PolishTokens.neutralFill)
                .frame(width: w, height: h)
                .offset(x: frame.minX, y: frame.minY)
            Text("No live video yet")
                .font(.system(size: 10, design: .monospaced))
                .foregroundColor(Color("TextSecondary").opacity(0.6))
                .frame(width: w, height: h)
                .offset(x: frame.minX, y: frame.minY)
            agentCursor(action, frame: frame)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// The bot's cursor with a badge (name, step). `frame` is where the
    /// screen is drawn in the viewport.
    private func agentCursor(_ action: ACIAgentAction, frame: CGRect) -> some View {
        let color = action.actor?.color.map { Color(hex: $0) } ?? theme.accent
        let px = frame.minX + CGFloat(action.x / action.screenW) * frame.width
        let py = frame.minY + CGFloat(action.y / action.screenH) * frame.height
        let step = action.step.map { " · step \($0)" } ?? ""
        return ZStack(alignment: .topLeading) {
            Image(systemName: "cursorarrow")
                .font(.system(size: 20, weight: .bold))
                .foregroundColor(color)
                .shadow(color: .white, radius: 1)
                .shadow(color: color.opacity(0.4), radius: 4)
            Text((action.actor?.name ?? "Bot") + step)
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(.white)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(color))
                .fixedSize()
                .offset(x: 14, y: 20)
        }
        .offset(x: px, y: py)
        .animation(.easeOut(duration: 0.25), value: action)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    // MARK: - Working line

    /// "<bot> is working · step N" with Take over and Stop.
    @ViewBuilder
    private var workingLine: some View {
        if viewModel.isActive, viewModel.agentAction != nil || viewModel.attachRunId != nil {
            VStack(spacing: 4) {
                HStack(spacing: 10) {
                    Text("\(viewModel.botName) is working" + (viewModel.currentStep.map { " · step \($0)" } ?? ""))
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(PolishTokens.ink)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if viewModel.controlHeld {
                        Button("Hand back") { viewModel.handBack() }
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                            .background(Capsule().fill(PolishTokens.ink))
                    } else {
                        Button("Take over") { viewModel.takeOver() }
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(PolishTokens.ink)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                            .background(Capsule().fill(PolishTokens.neutralFill))
                            .disabled(viewModel.controlBusy)
                    }
                    Button("Stop") { viewModel.stop() }
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color(hex: "#C0392B"))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(Capsule().fill(PolishTokens.neutralFill))
                }
                if viewModel.controlHeld {
                    Text("You have control · Hand back")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(PolishTokens.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    TakeOverKeys(onType: { viewModel.type($0) }, onKey: { viewModel.press(key: $0) })
                } else if let error = viewModel.controlError {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundColor(Color(hex: "#C0392B"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color.white)
            .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
        }
    }

    /// Element highlight for the current action (ElementHighlight,
    /// ACIComputerUseView.tsx:151-207): 2px kind-colored border, 7% fill,
    /// glow, and a `KIND · label` chip above the box.
    private func highlightBox(_ box: CGRect, scale: CGFloat, offsetX: CGFloat, offsetY: CGFloat) -> some View {
        let kind = viewModel.currentAction?.type
        let color = ACIActionPalette.color(for: kind)
        let label = viewModel.currentAction?.label ?? viewModel.currentAction?.selector
        let x = offsetX + box.minX * scale
        let y = offsetY + box.minY * scale
        let width = max(box.width * scale, 1)
        let height = max(box.height * scale, 1)

        return ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 4)
                .stroke(color.opacity(0.85), lineWidth: 2)
                .background(RoundedRectangle(cornerRadius: 4).fill(color.opacity(0.07)))
                .shadow(color: color.opacity(0.25), radius: 6)
                .frame(width: width, height: height)
                .offset(x: x, y: y)
                .animation(.easeOut(duration: 0.18), value: box)

            if let label, !label.isEmpty {
                Text((kind.map { "\($0.uppercased()) · " } ?? "") + label)
                    .font(.system(size: 9, weight: .bold, design: .monospaced))
                    .tracking(0.6)
                    .foregroundColor(color)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Theme.glassBgThick)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                    .overlay(
                        RoundedRectangle(cornerRadius: 4)
                            .stroke(color.opacity(0.4), lineWidth: 1)
                    )
                    .fixedSize()
                    .offset(x: x, y: max(y - 20, 0))
                    .animation(.easeOut(duration: 0.18), value: box)
            }
        }
        .allowsHitTesting(false)
    }

    /// Scanline texture overlay (ACIComputerUseView.tsx:372-375) — a 1px
    /// 6%-black line every 4px, purely decorative.
    private var scanlines: some View {
        Canvas { context, size in
            var y: CGFloat = 3
            while y < size.height {
                context.fill(
                    Path(CGRect(x: 0, y: y, width: size.width, height: 1)),
                    with: .color(.black.opacity(0.06))
                )
                y += 4
            }
        }
        .allowsHitTesting(false)
    }

    /// CONNECTING is only honest before the run's first feedback: no error,
    /// no screenshot, no trace lines, and no status reported yet. This
    /// backend (ACU) never streams screenshot frames, so gating on the
    /// screenshot alone pinned the viewport here for the entire run.
    private var isConnecting: Bool {
        viewModel.errorMessage == nil
            && viewModel.screenshot == nil
            && viewModel.traceLines.isEmpty
            && viewModel.status == .idle
    }

    private var connectingState: some View {
        VStack(spacing: 16) {
            VStack(spacing: 16) {
                ThinkingOrb(state: .connecting, size: .avatar)
                Text("CONNECTING")
                    .font(.system(size: 10, design: .monospaced))
                    .tracking(3)
                    .foregroundColor(Color("TextSecondary"))
            }
            .opacity(0.45)

            closeButton
        }
    }

    /// Shown in place of the screenshot once the run has produced feedback
    /// or finished: ACU streams state/trace frames only (no video), so the
    /// viewport surfaces the run's status here instead of a bare spinner.
    private var noVideoState: some View {
        VStack(spacing: 8) {
            Image(systemName: "video.slash")
                .font(.system(size: 20))
                .foregroundColor(Color("TextSecondary"))
            Text(noVideoLabel)
                .font(.system(size: 10, design: .monospaced))
                .tracking(3)
                .foregroundColor(Color("TextSecondary"))
            Text("This backend streams status and actions only — no screen video.")
                .font(.system(size: 10, design: .monospaced))
                .foregroundColor(Color("TextSecondary").opacity(0.7))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
        }
        .opacity(0.45)
    }

    private var noVideoLabel: String {
        switch viewModel.status {
        case .done: return "TASK COMPLETE"
        case .blocked: return "RUN BLOCKED"
        case .error: return "RUN ERROR"
        default: return "NO LIVE VIDEO"
        }
    }

    /// Secondary exit affordance for the connecting/terminal states — the
    /// top strip's labeled Close button is the primary exit in every state.
    private var closeButton: some View {
        Button("Close", action: onExit)
            .font(.system(size: 12, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 16)
            .padding(.vertical, 6)
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusSM)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
    }

    private func errorState(_ message: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "bolt.trianglebadge.exclamationmark")
                .font(.system(size: 24))
                .foregroundColor(Color("TextSecondary"))
            Text(message)
                .font(.system(size: 11, design: .monospaced))
                .foregroundColor(Color(hex: "#EF4444"))
                .multilineTextAlignment(.center)
            HStack(spacing: 10) {
                Button("Retry") { viewModel.retry() }
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(theme.accent)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 6)
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusSM)
                            .stroke(theme.accentGlow, lineWidth: 1)
                    )
                closeButton
            }
        }
        .padding(24)
    }

    // MARK: - Trace log

    private var traceLog: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(viewModel.traceLines) { line in
                        Text(line.text)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundColor(line.kind != nil
                                             ? ACIActionPalette.color(for: line.kind)
                                             : Color("TextSecondary"))
                            .lineLimit(1)
                            .id(line.id)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onChange(of: viewModel.traceLines.count) { _, _ in
                if let last = viewModel.traceLines.last {
                    proxy.scrollTo(last.id, anchor: .bottom)
                }
            }
        }
        .frame(height: 84)
        .background(Color("BgSecondary"))
        .overlay(alignment: .top) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
    }

    // MARK: - Controls

    private var controlBar: some View {
        HStack(spacing: 12) {
            if viewModel.requiresApproval {
                Button(action: { viewModel.approve(deny: true) }) {
                    Text("Deny")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 7)
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusSM)
                                .stroke(Color("BorderSubtle"), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)

                Button(action: { viewModel.approve(deny: false) }) {
                    Text("Approve")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("BgPrimary"))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 7)
                        .background(Theme.statusWarning)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                }
                .buttonStyle(.plain)
            }

            Spacer()

            if viewModel.isActive {
                Button(action: { viewModel.stop() }) {
                    Label("Stop", systemImage: "stop.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color(hex: "#EF4444"))
                        .padding(.horizontal, 14)
                        .padding(.vertical, 7)
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusSM)
                                .stroke(Color(hex: "#EF4444").opacity(0.4), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(Theme.glassBgThick)
        .overlay(alignment: .top) {
            Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
        }
    }
}

/// Text box and Return for the person who took over: typed text goes to the
/// computer's keyboard route.
private struct TakeOverKeys: View {
    let onType: (String) -> Void
    let onKey: (String) -> Void
    @State private var text = ""

    var body: some View {
        HStack(spacing: 8) {
            TextField("Type to the computer", text: $text)
                .font(.system(size: 13))
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .padding(8)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(PolishTokens.neutralFill))
                .onSubmit(send)
            Button("Send", action: send)
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(PolishTokens.ink)
            Button("Return") { onKey("Return") }
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(PolishTokens.ink)
        }
    }

    private func send() {
        guard !text.isEmpty else { return }
        onType(text)
        text = ""
    }
}
