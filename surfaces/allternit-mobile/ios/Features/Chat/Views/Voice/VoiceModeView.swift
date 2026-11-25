import SwiftUI
import UIKit

/// Full voice mode (Phase 7b, Claude iOS parity): a full-screen takeover
/// with an ambient gradient tinted by turn state (blue family = listening,
/// warm/orange family = speaking), the assistant reply in serif, the live
/// STT transcript in italics at the bottom, and bottom controls (settings
/// gear, mic, X to end).
///
/// Turns run through VoiceModeViewModel; the conversation itself goes
/// through the normal chat stream, so the text thread stays in sync. On end
/// (X) the caller files a "Voice chat ended · Ns" card into the feed and
/// dismisses the cover.
///
/// Orb travel (SCOPE-B1 clip 3): the cover is presented and dismissed with no
/// system slide and a clear background (`VoiceHost.present`). The stage then
/// moves its own orb: in from the quick bar's orb (window coordinates, so it
/// crosses the presentation boundary that `matchedGeometryEffect` cannot) while
/// the stage fades in round it; back down onto the bar on minimize; and a melt
/// to a dot on end. With no bar on screen, or Reduce Motion, it crossfades.
struct VoiceModeView: View {
    /// The live session, owned by `VoiceHost` (so the quick bar and this
    /// full-screen view are the same conversation).
    @ObservedObject var host: VoiceHost
    @ObservedObject var viewModel: VoiceModeViewModel
    /// Called with the session length in seconds when the user ends voice
    /// mode — the host files the VoiceSummary feed card.
    let onEnd: (Int) -> Void
    /// Bot conversations: the bot's avatar is the speaker, in its live mode look.
    let bot: AgentRecord?

    @ObservedObject private var looks = SpeakerLookStore.shared
    @ObservedObject private var botColors = BotColorStore.shared
    @State private var isSettingsPresented = false
    /// Slow breathing pulse for the gradient while listening/speaking.
    @State private var pulse = false
    /// PTT/tap disambiguation on the mic button (tap = mute, hold = talk).
    @State private var pressStartedAt: Date? = nil
    @ReducedMotion private var reduceMotion
    /// The stage is up. False while the orb travels in from the bar or back to it.
    @State private var revealed = false
    /// End (X): the orb melts out before the call ends.
    @State private var ending = false
    /// Where the stage orb rests, in window coordinates.
    @State private var restFrame: CGRect = .zero

    /// Bar ↔ stage travel, ~0.4 s in and ~0.35 s back (ui-motion: large shared element).
    private static let travelIn: Animation = .spring(response: 0.42, dampingFraction: 0.86)
    private static let travelOut: Animation = .spring(response: 0.35, dampingFraction: 0.92)

    init(host: VoiceHost, viewModel: VoiceModeViewModel, bot: AgentRecord? = nil, onEnd: @escaping (Int) -> Void) {
        self.host = host
        self.viewModel = viewModel
        self.bot = bot
        self.onEnd = onEnd
    }

    var body: some View {
        Group {
            if let bot { botStage(bot) } else { plainStage }
        }
        // Controls act only once the stage is up, and not while it leaves.
        .allowsHitTesting(revealed && !ending)
        .sheet(isPresented: $isSettingsPresented) {
            VoiceSettingsSheet(bot: bot)
        }
        .onAppear {
            pulse = true
            #if DEBUG
            // `-open-voice-settings` (DEBUG only): opens the voice settings
            // sheet straight away for screenshot verification.
            if launchArgumentEnabled("open-voice-settings") {
                isSettingsPresented = true
            }
            #endif
        }
    }

    // MARK: - Bot stage (live mode)

    private var speakerPhase: VoiceSpeakerView<BotAvatarView>.Phase {
        if viewModel.isConnecting { return .connecting }
        switch viewModel.state {
        case .idle: return .idle
        case .listening: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        }
    }

    /// The plain stage's phase: `Phase` is nested in the generic view, so each face type has its own.
    private var markPhase: VoiceSpeakerView<VoiceMarkFace>.Phase {
        if viewModel.isConnecting { return .connecting }
        switch viewModel.state {
        case .idle: return .idle
        case .listening: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        }
    }

    private func botStage(_ bot: AgentRecord) -> some View {
        let look = looks.look(for: bot.id)
        let _ = looks.botRevision
        let color = botColors.hex(for: bot).map { Color(hex: $0) }
        let youTalking = viewModel.state == .listening && !viewModel.liveTranscript.isEmpty
        let captionText = youTalking ? viewModel.liveTranscript
            : (viewModel.state == .speaking || viewModel.state == .thinking) ? viewModel.replyText : ""
        let quiet: String? = viewModel.isConnecting ? "Connecting"
            : viewModel.state == .thinking && viewModel.replyText.isEmpty ? "\(bot.botDisplayName) is thinking" : nil
        return ZStack {
            Color("BgPrimary").ignoresSafeArea()
                .opacity(stageOpacity)
            VStack(spacing: 0) {
                Text("Talking with \(bot.botDisplayName)")
                    .font(.subheadline.weight(.medium))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 12)
                    .opacity(stageOpacity)
                Spacer(minLength: 0)
                travelling(style: look.style) {
                    VoiceSpeakerView(phase: speakerPhase, muted: viewModel.isMuted, style: look.style, palette: look.palette, botColor: color, size: VoiceHost.stageFaceSize) {
                        BotAvatarView(bot: bot, size: VoiceHost.stageFaceSize, live: true)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture { viewModel.interrupt() }
                Group {
                    Text(statusLabel.uppercased())
                        .font(.system(size: 12, weight: .semibold))
                        .tracking(1.6)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.top, 8)
                        .accessibilityLabel(statusLabel)
                    if let engine = viewModel.engineLabel {
                        Text(engine).font(.caption2).foregroundColor(Color("TextSecondary")).padding(.top, 6)
                    }
                    if let statusLine = viewModel.statusLine {
                        Text(statusLine)
                            .font(.caption)
                            .foregroundColor(Theme.statusWarning)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 32)
                            .padding(.top, 6)
                    }
                }
                .opacity(stageOpacity)
                Spacer(minLength: 0)
                Group {
                    VoiceCaptionsView(you: youTalking, name: bot.botDisplayName, color: color, text: captionText, quiet: quiet, size: looks.captionSize) {
                        BotAvatarView(bot: bot, size: 18)
                    }
                    .padding(.horizontal, 14)
                    .padding(.bottom, 14)
                    .onTapGesture { viewModel.interrupt() }
                    controlsRow
                        .padding(.bottom, 20)
                }
                .opacity(stageOpacity)
            }
        }
    }

    // MARK: - Plain chat stage

    /// The Allternit thinking mark as the speaker's face, in the default live
    /// mode look and colour (Eoj 2026-11-20: the Settings pick shows in every
    /// voice chat; with no bot, "Bot's colour" keeps the spectrum), then the
    /// reply or the state in serif under it.
    private var plainStage: some View {
        ZStack {
            ambientBackground
                .opacity(stageOpacity)

            VStack(spacing: 0) {
                Spacer(minLength: 0)
                travelling(style: looks.style) {
                    VoiceSpeakerView(phase: markPhase, muted: viewModel.isMuted, style: looks.style, palette: looks.palette, botColor: nil, size: VoiceHost.stageFaceSize) {
                        VoiceMarkFace(state: viewModel.state, size: VoiceHost.stageFaceSize)
                    }
                }
                .contentShape(Rectangle())
                .onTapGesture { viewModel.interrupt() }
                .padding(.bottom, 8)
                Group {
                    centerContent
                }
                .opacity(stageOpacity)
                Spacer(minLength: 0)

                Group {

                // Live STT transcript (Claude parity: italics, bottom).
                Text(viewModel.liveTranscript.isEmpty ? " " : viewModel.liveTranscript)
                    .font(.body.italic())
                    .foregroundColor(Color("TextPrimary").opacity(0.85))
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                    .padding(.horizontal, 32)
                    .padding(.bottom, 6)
                    .animation(.easeOut(duration: 0.15), value: viewModel.liveTranscript)

                if let engine = viewModel.engineLabel {
                    Text(engine)
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.bottom, 4)
                }

                // In-place error/status line — no alerts, no crashes.
                if let statusLine = viewModel.statusLine {
                    Text(statusLine)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 32)
                        .padding(.bottom, 6)
                }

                controlsRow
                    .padding(.bottom, 20)
                }
                .opacity(stageOpacity)
            }
        }
    }

    // MARK: - Orb travel

    /// Everything but the orb: up with the stage, down while it travels or melts.
    private var stageOpacity: Double { revealed && !ending ? 1 : 0 }

    /// The quick bar's orb to travel from and back to; nil = crossfade (a new
    /// call opened straight to full voice grows its orb in place instead).
    private var barTarget: CGRect? {
        reduceMotion || host.growPending || host.barOrbFrame == .zero ? nil : host.barOrbFrame
    }

    /// The stage orb, moved by the reveal: at the bar's orb (scaled to its
    /// face) before the stage is up, at rest once it is, a dot on end. The
    /// look inside keeps following the voice level the whole way.
    private func travelling<Orb: View>(style: SpeakerStyle, @ViewBuilder _ orb: () -> Orb) -> some View {
        let target = barTarget
        let measured = restFrame != .zero
        let atBar = !revealed && !ending && target != nil
        let s = VoiceHost.barFaceSize / VoiceHost.stageFaceSize
        // The Ring look seats the face below its centre; line face up with face.
        let faceDy = VoiceSpeakerView<VoiceMarkFace>.faceDy(style: style, size: VoiceHost.stageFaceSize, compact: false)
        var offset = CGSize.zero
        if atBar, let target, measured {
            offset = CGSize(width: target.midX - restFrame.midX, height: target.midY - restFrame.midY - faceDy * s)
        }
        let scale: CGFloat = ending ? (reduceMotion ? 1 : 0.06)
            : atBar ? s
            : revealed || reduceMotion ? 1 : 0.92
        let opacity: Double = ending ? 0 : atBar ? (measured ? 1 : 0) : revealed ? 1 : 0
        return ZStack {
            orb()
                .modifier(VoiceOrbGrow(host: host))
                .scaleEffect(scale)
                .offset(offset)
                .opacity(opacity)
        }
        // Measured outside the moving layer, so it reads the resting frame.
        .background(GeometryReader { proxy in
            Color.clear
                .onAppear {
                    restFrame = proxy.frame(in: .global)
                    reveal()
                }
                .onChange(of: proxy.frame(in: .global)) { _, frame in restFrame = frame }
        })
    }

    /// Brings the stage up one frame after the orb is drawn at the bar.
    private func reveal() {
        guard !revealed, !ending else { return }
        DispatchQueue.main.async {
            let animation: Animation = barTarget != nil ? Self.travelIn
                : reduceMotion ? Motion.fade : .easeOut(duration: 0.22)
            withAnimation(animation) { revealed = true } completion: {
                // The cover arrived without a system transition: point VoiceOver at it.
                UIAccessibility.post(notification: .screenChanged, argument: nil)
            }
        }
    }

    /// Minimize: the orb travels back onto the bar, then the cover goes.
    private func minimize() {
        guard revealed, !ending else { return }
        let animation: Animation = barTarget != nil ? Self.travelOut
            : reduceMotion ? Motion.fade : .easeOut(duration: 0.2)
        withAnimation(animation) { revealed = false } completion: {
            host.present(.bar)
            UIAccessibility.post(notification: .screenChanged, argument: nil)
        }
    }

    /// End (X): the orb melts to a dot and the stage fades, then the call ends.
    private func endCall() {
        guard !ending else { return }
        withAnimation(reduceMotion ? Motion.fade : .easeIn(duration: 0.26)) { ending = true } completion: {
            var transaction = Transaction()
            transaction.disablesAnimations = true
            withTransaction(transaction) {
                if let durationSeconds = host.end() { onEnd(durationSeconds) }
            }
            UIAccessibility.post(notification: .screenChanged, argument: nil)
        }
    }

    // MARK: - Ambient background

    /// Gradient tint per turn state — animated on every state change.
    private var stateTint: (primary: Color, secondary: Color) {
        switch viewModel.state {
        case .idle:
            return (Theme.bgTertiary, Color("BgSecondary"))
        case .listening:
            // Blue family (Claude's listening tint).
            return (Theme.statusInfo, Color(hex: "#1E3A8A"))
        case .thinking:
            return (Color(hex: "#7C8DB0"), Color(hex: "#2E3A52"))
        case .speaking:
            // Warm/orange family (Claude's speaking tint).
            return (Color("AccentPrimary"), Color(hex: "#7C3A1E"))
        }
    }

    private var ambientBackground: some View {
        let tint = stateTint
        let animating = viewModel.state == .listening || viewModel.state == .speaking
        return ZStack {
            Color("BgPrimary").edgesIgnoringSafeArea(.all)

            Circle()
                .fill(tint.primary.opacity(animating ? (pulse ? 0.5 : 0.3) : 0.25))
                .frame(width: 420, height: 420)
                .blur(radius: 120)
                .offset(x: -60, y: -180)

            Circle()
                .fill(tint.secondary.opacity(animating ? (pulse ? 0.45 : 0.25) : 0.2))
                .frame(width: 460, height: 460)
                .blur(radius: 130)
                .offset(x: 70, y: 220)
        }
        .edgesIgnoringSafeArea(.all)
        .animation(.easeInOut(duration: 0.8), value: viewModel.state)
        .animation(.easeInOut(duration: 2.2).repeatForever(autoreverses: true), value: pulse)
    }

    // MARK: - Center content

    @ViewBuilder
    private var centerContent: some View {
        if !viewModel.replyText.isEmpty {
            // Assistant reply — serif (Claude voice-mode parity). A tap
            // while thinking/speaking interrupts (stop TTS / abort stream);
            // simultaneous so the reply stays scrollable.
            ScrollView {
                Text(viewModel.replyText)
                    .font(.system(size: 22, weight: .medium, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 28)
                    .frame(maxWidth: .infinity)
            }
            .scrollIndicators(.hidden)
            .padding(.horizontal, 8)
            .contentShape(Rectangle())
            .simultaneousGesture(
                TapGesture().onEnded { viewModel.interrupt() }
            )
        } else {
            Text(statusLabel)
                .font(.system(size: 22, weight: .medium, design: .serif))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
                .contentShape(Rectangle())
                .onTapGesture { viewModel.interrupt() }
        }

        if viewModel.canInterrupt {
            // Barge-in affordance — visible only while a tap would act.
            HStack(spacing: 5) {
                Image(systemName: "hand.tap")
                    .font(.system(size: 10, weight: .semibold))
                Text("Tap to interrupt")
                    .font(.caption)
            }
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
            .padding(.bottom, 10)
            .transition(.opacity)
        }
    }

    private var statusLabel: String {
        switch viewModel.state {
        case .idle:
            if viewModel.isConnecting {
                if let progress = viewModel.modelProgress {
                    return "Downloading voice model \(Int(progress * 100))%"
                }
                return "Connecting…"
            }
            if viewModel.isMuted { return "Muted" }
            return viewModel.isPushToTalk ? "Hold to talk" : "Tap to talk"
        case .listening:
            return "Listening…"
        case .thinking:
            return "Thinking…"
        case .speaking:
            return "Speaking…"
        }
    }

    // MARK: - Bottom controls

    private var controlsRow: some View {
        HStack {
            controlButton(systemName: "gearshape", label: "Voice settings") {
                isSettingsPresented = true
            }

            controlButton(systemName: "arrow.down.right.and.arrow.up.left", label: "Minimize to voice bar") {
                minimize()
            }

            Spacer()

            micButton

            Spacer()

            controlButton(systemName: "xmark", label: "End voice chat") {
                endCall()
            }
        }
        .padding(.horizontal, 36)
    }

    /// Center mic control. Hands-free: tap toggles mute. Push-to-talk: hold
    /// to record, release to send; a short tap still toggles mute.
    private var micButton: some View {
        let listening = viewModel.state == .listening
        return ZStack {
            Circle()
                .fill(viewModel.isMuted ? Color("BgSecondary") : Color.white)
                .frame(width: 72, height: 72)
                .shadow(color: stateTint.primary.opacity(listening ? 0.6 : 0.2), radius: listening ? 16 : 6)

            Image(systemName: viewModel.isMuted ? "mic.slash.fill" : "mic.fill")
                .font(.system(size: 26, weight: .medium))
                .foregroundColor(viewModel.isMuted ? Color("TextSecondary") : .black)
                .symbolEffect(.pulse, isActive: listening && !viewModel.isMuted)
        }
        .contentShape(Circle())
        .gesture(
            // A plain Button would fire on release after a PTT hold and
            // double-trigger — a zero-distance drag with a duration check
            // tells tap (mute) from hold (talk) instead.
            DragGesture(minimumDistance: 0)
                .onChanged { _ in
                    guard pressStartedAt == nil else { return }
                    pressStartedAt = Date()
                    if viewModel.isPushToTalk { viewModel.pressBegan() }
                }
                .onEnded { _ in
                    let wasTap = pressStartedAt.map { Date().timeIntervalSince($0) < 0.25 } ?? true
                    pressStartedAt = nil
                    if viewModel.isPushToTalk, !wasTap {
                        viewModel.pressEnded()
                    } else {
                        viewModel.toggleMute()
                    }
                }
        )
        .accessibilityLabel(viewModel.isMuted ? "Unmute microphone" : "Mute microphone")
    }

    private func controlButton(systemName: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 44, height: 44)
                .background(.ultraThinMaterial, in: Circle())
                .overlay(
                    Circle().strokeBorder(Color.white.opacity(0.18), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

/// A plain chat's speaker face: the Allternit thinking mark (the web's brand
/// orb) at `size`. ThinkingOrb has two tuned sizes, so it draws at `.avatar`
/// and scales. Used by full voice mode and the quick bar.
struct VoiceMarkFace: View {
    let state: VoiceModeViewModel.TurnState
    let size: CGFloat

    var body: some View {
        ThinkingOrb(state: orbState, size: .avatar, paused: state == .idle)
            .scaleEffect(size / ThinkingOrbSize.avatar.rawValue)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }

    private var orbState: ThinkingOrbState {
        switch state {
        case .idle, .listening: return .listening
        case .thinking: return .breathing
        case .speaking: return .composing
        }
    }
}
