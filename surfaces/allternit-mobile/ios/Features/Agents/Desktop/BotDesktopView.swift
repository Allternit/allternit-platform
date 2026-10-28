import SwiftUI
import UIKit

// -----------------------------------------------------------------------------
// Fullscreen native VNC viewer for a bot's desktop sandbox, presented from
// the Agent detail Desktop card ("View desktop"). Renders the RFB
// framebuffer via `BotDesktopSession`/`RFBClient`
// (Features/Agents/Desktop/RFBClient.swift) — the native counterpart of the
// web's noVNC canvas (surfaces/ai.allternit.com/src/views/bots/
// BotDesktopView.tsx), talking to `GET /ws/bots/:id/desktop/vnc`
// (cmd/allternit-api/src/bot_desktop_stream.rs).
//
// TOUCH MAPPING (documented choice — touch has no hover):
//   tap            = left click (press+release at point)
//   two-finger tap = right click
//   long-press     = right click (single-hand fallback)
//   one-finger drag, zoomed to fit   = pointer move with left button held
//   one-finger drag, pinch-zoomed in = pan the viewport
//   pinch          = zoom (1…8× the fit scale; returning to 1× resets pan)
// Keyboard: a hidden first-responder view (toolbar keyboard button, visible
// while controlling) captures software + hardware keyboards → X11 keysyms.
// All input is gated on `human_controls` by BotDesktopSession — the server
// forwards input regardless of control_state, so the gate is client-side.
// -----------------------------------------------------------------------------

struct BotDesktopView: View {
    let agent: AgentRecord

    @StateObject private var session: BotDesktopSession
    @StateObject private var desktopStore = BotDesktopStore.shared
    @Environment(\.dismiss) private var dismiss

    /// Committed pinch zoom on top of the aspect-fit scale (1 = fit).
    @State private var userScale: CGFloat = 1
    /// In-progress pinch factor (reset by the gesture each time).
    @GestureState private var magnifyDelta: CGFloat = 1
    @State private var pan: CGSize = .zero
    /// Pan value when the current drag began (drags report relative
    /// translations, panning needs the accumulated value).
    @State private var panAtDragStart: CGSize? = nil
    @State private var isPointerDragging = false
    @State private var isKeyboardVisible = false

    /// `status` is the Desktop-card snapshot the button was tapped with; it
    /// always carries a wsURL there (the card gates on it). Live control
    /// state keeps flowing in from BotDesktopStore via onChange below.
    init(agent: AgentRecord, status: BotDesktopStatus) {
        self.agent = agent
        _session = StateObject(wrappedValue: BotDesktopSession(
            wsPath: status.wsURL ?? "",
            controlState: status.controlState
        ))
    }

    var body: some View {
        NavigationStack {
            GeometryReader { geometry in
                ZStack {
                    Color.black
                    framebufferLayer(in: geometry.size)
                    DesktopInteractionOverlay(
                        isKeyboardVisible: isKeyboardVisible && session.allowsInput,
                        onTap: { point in handleTap(at: point, buttonMask: 1, in: geometry.size) },
                        onRightClick: { point in handleTap(at: point, buttonMask: 4, in: geometry.size) },
                        onKeysym: { keysym, down in session.sendKey(keysym: keysym, down: down) }
                    )
                    bottomOverlay
                    stateOverlay
                }
                .contentShape(Rectangle())
                .gesture(magnificationGesture)
                .gesture(dragGesture(in: geometry.size))
            }
            .background(Color.black.ignoresSafeArea())
            .navigationTitle(session.desktopName.isEmpty ? "\(agent.name) desktop" : session.desktopName)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbarContent }
            .onChange(of: desktopStore.entry(for: agent.id)?.status?.controlState) { _, newValue in
                if let newValue {
                    session.controlState = newValue
                }
            }
        }
        .task { session.start() }
        .onDisappear { session.stop() }
    }

    // MARK: - Canvas

    @ViewBuilder
    private func framebufferLayer(in size: CGSize) -> some View {
        if let frame = session.frame {
            let rect = displayedRect(in: size)
            Image(decorative: frame, scale: 1)
                .resizable()
                .frame(width: rect.width, height: rect.height)
                .position(x: rect.midX, y: rect.midY)
        }
    }

    /// Aspect-fit rect of the framebuffer in the canvas, with user zoom and
    /// pan applied.
    private func displayedRect(in size: CGSize) -> CGRect {
        let framebuffer = session.framebufferSize
        guard framebuffer.width > 0, framebuffer.height > 0 else { return .zero }
        let fitScale = min(size.width / framebuffer.width, size.height / framebuffer.height)
        let scale = fitScale * userScale * magnifyDelta
        let width = framebuffer.width * scale
        let height = framebuffer.height * scale
        return CGRect(
            x: (size.width - width) / 2 + pan.width,
            y: (size.height - height) / 2 + pan.height,
            width: width,
            height: height
        )
    }

    /// Canvas point → framebuffer pixel; nil outside the image (letterbox
    /// taps are ignored rather than clamped into a stray click).
    private func framebufferPoint(for viewPoint: CGPoint, in size: CGSize) -> CGPoint? {
        let rect = displayedRect(in: size)
        let framebuffer = session.framebufferSize
        guard rect.width > 0, rect.height > 0,
              framebuffer.width > 0, framebuffer.height > 0 else { return nil }
        let x = (viewPoint.x - rect.minX) / rect.width * framebuffer.width
        let y = (viewPoint.y - rect.minY) / rect.height * framebuffer.height
        guard x >= 0, y >= 0, x < framebuffer.width, y < framebuffer.height else { return nil }
        return CGPoint(x: x, y: y)
    }

    private func handleTap(at point: CGPoint, buttonMask: UInt8, in size: CGSize) {
        guard let point = framebufferPoint(for: point, in: size) else { return }
        session.tap(x: Int(point.x), y: Int(point.y), buttonMask: buttonMask)
    }

    private var magnificationGesture: some Gesture {
        MagnifyGesture()
            .updating($magnifyDelta) { value, state, _ in
                state = value.magnification
            }
            .onEnded { value in
                userScale = min(8, max(1, userScale * value.magnification))
                if userScale <= 1.02 {
                    // Back at fit: recenter (pan only exists for zoomed-in).
                    userScale = 1
                    pan = .zero
                }
            }
    }

    /// One finger: pan when zoomed in, pointer drag (left button held) when
    /// at fit — see the file header for the mapping rationale.
    private func dragGesture(in size: CGSize) -> some Gesture {
        DragGesture()
            .onChanged { value in
                if userScale * magnifyDelta > 1.02 {
                    if panAtDragStart == nil { panAtDragStart = pan }
                    if let base = panAtDragStart {
                        pan = CGSize(
                            width: base.width + value.translation.width,
                            height: base.height + value.translation.height
                        )
                    }
                } else if session.allowsInput {
                    if !isPointerDragging {
                        isPointerDragging = true
                        if let start = framebufferPoint(for: value.startLocation, in: size) {
                            session.sendPointer(x: Int(start.x), y: Int(start.y), buttonMask: 1)
                        }
                    }
                    if let point = framebufferPoint(for: value.location, in: size) {
                        session.sendPointer(x: Int(point.x), y: Int(point.y), buttonMask: 1)
                    }
                }
            }
            .onEnded { value in
                if isPointerDragging,
                   let point = framebufferPoint(for: value.location, in: size) {
                    session.sendPointer(x: Int(point.x), y: Int(point.y), buttonMask: 0)
                }
                isPointerDragging = false
                panAtDragStart = nil
            }
    }

    // MARK: - Overlays

    /// Bottom-center: "Hand back" while controlling (parity with the web
    /// header button, BotDesktopView.tsx:267-279), otherwise the view-only
    /// banner.
    @ViewBuilder
    private var bottomOverlay: some View {
        if session.state == .live {
            VStack {
                Spacer()
                if session.controlState == .humanControls {
                    Button(action: {
                        Task { await desktopStore.handBack(botId: agent.id) }
                    }) {
                        Label("Hand back to bot", systemImage: "hand.raised")
                            .font(.subheadline)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 8)
                            .background(.ultraThinMaterial)
                            .clipShape(Capsule())
                    }
                } else {
                    Label(viewOnlyLabel, systemImage: "eye")
                        .font(.caption)
                        .foregroundColor(.white)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(.ultraThinMaterial)
                        .clipShape(Capsule())
                }
            }
            .padding(.bottom, 20)
            // Passive UI — taps fall through to the canvas outside the
            // capsule itself.
            .allowsHitTesting(session.controlState == .humanControls)
        }
    }

    private var viewOnlyLabel: String {
        session.controlState == .humanObserving
            ? "You're observing — view only"
            : "Bot is driving — view only"
    }

    /// Blocks the canvas until frames flow (and after a failure, owns the
    /// retry path).
    @ViewBuilder
    private var stateOverlay: some View {
        switch session.state {
        case .connecting:
            connectionOverlay {
                ProgressView()
                Text("Connecting to desktop…")
            }
        case .reconnecting(let attempt):
            connectionOverlay {
                ProgressView()
                Text("Connection lost — reconnecting (\(attempt)/\(BotDesktopSession.maxReconnectAttempts))…")
            }
        case .failed(let message):
            connectionOverlay {
                FriendlyStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't show the desktop",
                    message: message,
                    actionTitle: "Retry",
                    action: { session.retry() }
                )
            }
        case .live:
            if session.frame == nil {
                connectionOverlay {
                    ProgressView()
                    Text("Waiting for the first frame…")
                }
            }
        }
    }

    private func connectionOverlay<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        VStack(spacing: 10) { content() }
            .foregroundColor(.white)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(32)
            .background(Color.black.opacity(0.9))
    }

    // MARK: - Toolbar

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button("Close") { dismiss() }
        }
        ToolbarItem(placement: .principal) {
            Text(connectionStateLabel)
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            if session.allowsInput {
                Button(action: { isKeyboardVisible.toggle() }) {
                    Image(systemName: isKeyboardVisible ? "keyboard.chevron.compact.down" : "keyboard")
                }
                .accessibilityLabel(isKeyboardVisible ? "Hide keyboard" : "Show keyboard")
            }
            Text(session.controlState.label)
                .font(.caption)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(controlBadgeColor.opacity(0.2))
                .foregroundColor(controlBadgeColor)
                .clipShape(Capsule())
        }
    }

    private var connectionStateLabel: String {
        switch session.state {
        case .connecting: return "Connecting…"
        case .live: return "Live"
        case .reconnecting(let attempt): return "Reconnecting (\(attempt)/\(BotDesktopSession.maxReconnectAttempts))…"
        case .failed: return "Connection failed"
        }
    }

    private var controlBadgeColor: Color {
        session.controlState == .humanControls ? Theme.statusSuccess : Theme.statusInfo
    }
}

// MARK: - Touch + keyboard capture overlay

/// Transparent canvas-covering view that owns the tap recognizers (single /
/// two-finger / long-press — SwiftUI has no two-finger tap) and acts as the
/// keyboard first responder (UIKeyInput for software keyboards, UIPress for
/// hardware). Points are reported in view coordinates; the parent converts
/// to framebuffer pixels.
struct DesktopInteractionOverlay: UIViewRepresentable {
    var isKeyboardVisible: Bool
    var onTap: (CGPoint) -> Void
    var onRightClick: (CGPoint) -> Void
    var onKeysym: (_ keysym: UInt32, _ down: Bool) -> Void

    func makeUIView(context: Context) -> DesktopKeyboardCaptureView {
        let view = DesktopKeyboardCaptureView()
        update(view)
        return view
    }

    func updateUIView(_ view: DesktopKeyboardCaptureView, context: Context) {
        update(view)
        if isKeyboardVisible, !view.isFirstResponder {
            view.becomeFirstResponder()
        } else if !isKeyboardVisible, view.isFirstResponder {
            view.resignFirstResponder()
        }
    }

    private func update(_ view: DesktopKeyboardCaptureView) {
        view.onTap = onTap
        view.onRightClick = onRightClick
        view.onKeysym = onKeysym
    }
}

final class DesktopKeyboardCaptureView: UIView, UIKeyInput {
    var onTap: ((CGPoint) -> Void)?
    var onRightClick: ((CGPoint) -> Void)?
    var onKeysym: ((UInt32, Bool) -> Void)?

    override var canBecomeFirstResponder: Bool { true }

    /// Always true so the software keyboard keeps delivering
    /// `deleteBackward` even though no text is stored here.
    var hasText: Bool { true }

    // UITextInputTraits — plain passthrough typing, no autocorrect/assist
    // rewriting what the remote desktop receives.
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var keyboardType: UIKeyboardType = .default

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .clear
        isOpaque = false

        let singleTap = UITapGestureRecognizer(target: self, action: #selector(handleSingleTap(_:)))
        let twoFingerTap = UITapGestureRecognizer(target: self, action: #selector(handleTwoFingerTap(_:)))
        twoFingerTap.numberOfTouchesRequired = 2
        singleTap.require(toFail: twoFingerTap)
        let longPress = UILongPressGestureRecognizer(target: self, action: #selector(handleLongPress(_:)))
        longPress.minimumPressDuration = 0.5
        addGestureRecognizer(singleTap)
        addGestureRecognizer(twoFingerTap)
        addGestureRecognizer(longPress)
    }

    required init?(coder: NSCoder) {
        fatalError("DesktopKeyboardCaptureView is created in code only")
    }

    @objc private func handleSingleTap(_ recognizer: UITapGestureRecognizer) {
        onTap?(recognizer.location(in: self))
    }

    @objc private func handleTwoFingerTap(_ recognizer: UITapGestureRecognizer) {
        onRightClick?(recognizer.location(in: self))
    }

    @objc private func handleLongPress(_ recognizer: UILongPressGestureRecognizer) {
        guard recognizer.state == .began else { return }
        onRightClick?(recognizer.location(in: self))
    }

    // MARK: - UIKeyInput (software keyboard + hardware printable keys)

    func insertText(_ text: String) {
        for scalar in text.unicodeScalars {
            let keysym: UInt32?
            switch scalar {
            case "\n", "\r": keysym = RFBKeysym.return
            case "\t": keysym = RFBKeysym.tab
            default: keysym = RFBKeysym.forScalar(scalar)
            }
            if let keysym {
                onKeysym?(keysym, true)
                onKeysym?(keysym, false)
            }
        }
    }

    func deleteBackward() {
        onKeysym?(RFBKeysym.backSpace, true)
        onKeysym?(RFBKeysym.backSpace, false)
    }

    // MARK: - Hardware keyboard (non-printable keys)

    /// Printable keys arrive via `insertText`; here only the structural keys
    /// (arrows, Escape, Delete-forward, modifiers, …) that never produce
    /// text are intercepted — everything else goes to super so the text
    /// pipeline still sees it.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        handlePresses(presses, down: true, event: event) { super.pressesBegan($0, with: $1) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        handlePresses(presses, down: false, event: event) { super.pressesEnded($0, with: $1) }
    }

    private func handlePresses(_ presses: Set<UIPress>, down: Bool, event: UIPressesEvent?,
                               fallback: (Set<UIPress>, UIPressesEvent?) -> Void) {
        var unhandled = Set<UIPress>()
        for press in presses {
            if let keyCode = press.key?.keyCode,
               let keysym = Self.specialKeysym(for: keyCode) {
                onKeysym?(keysym, down)
            } else {
                unhandled.insert(press)
            }
        }
        if !unhandled.isEmpty {
            fallback(unhandled, event)
        }
    }

    private static func specialKeysym(for keyCode: UIKeyboardHIDUsage) -> UInt32? {
        switch keyCode {
        case .keyboardLeftArrow: return RFBKeysym.left
        case .keyboardUpArrow: return RFBKeysym.up
        case .keyboardRightArrow: return RFBKeysym.right
        case .keyboardDownArrow: return RFBKeysym.down
        case .keyboardReturnOrEnter: return RFBKeysym.return
        case .keyboardTab: return RFBKeysym.tab
        case .keyboardEscape: return RFBKeysym.escape
        case .keyboardDeleteOrBackspace: return RFBKeysym.backSpace
        case .keyboardDeleteForward: return RFBKeysym.delete
        case .keyboardLeftShift: return RFBKeysym.shiftLeft
        case .keyboardRightShift: return RFBKeysym.shiftRight
        case .keyboardLeftControl: return RFBKeysym.controlLeft
        case .keyboardRightControl: return RFBKeysym.controlRight
        case .keyboardLeftAlt: return RFBKeysym.altLeft
        // ⌘ → Super — the X11 desktop has no Command key.
        case .keyboardLeftGUI: return RFBKeysym.superLeft
        case .keyboardRightGUI: return RFBKeysym.superRight
        default: return nil
        }
    }
}
