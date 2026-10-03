import SwiftUI

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
struct VoiceModeView: View {
    /// The live session, owned by `VoiceHost` (so the quick bar and this
    /// full-screen view are the same conversation).
    @ObservedObject var host: VoiceHost
    @ObservedObject var viewModel: VoiceModeViewModel
    /// Called with the session length in seconds when the user ends voice
    /// mode — the host files the VoiceSummary feed card.
    let onEnd: (Int) -> Void

    @State private var isSettingsPresented = false
    /// Slow breathing pulse for the gradient while listening/speaking.
    @State private var pulse = false
    /// PTT/tap disambiguation on the mic button (tap = mute, hold = talk).
    @State private var pressStartedAt: Date? = nil

    init(host: VoiceHost, viewModel: VoiceModeViewModel, onEnd: @escaping (Int) -> Void) {
        self.host = host
        self.viewModel = viewModel
        self.onEnd = onEnd
    }

    var body: some View {
        ZStack {
            ambientBackground

            VStack(spacing: 0) {
                Spacer(minLength: 0)
                centerContent
                Spacer(minLength: 0)

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
        }
        .sheet(isPresented: $isSettingsPresented) {
            VoiceSettingsSheet()
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
                host.presentation = .bar
            }

            Spacer()

            micButton

            Spacer()

            controlButton(systemName: "xmark", label: "End voice chat") {
                if let durationSeconds = host.end() { onEnd(durationSeconds) }
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
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
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
