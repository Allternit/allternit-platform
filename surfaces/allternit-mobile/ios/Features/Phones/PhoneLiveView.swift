import SwiftUI

/// Live view of an Android phone that shares its screen with the Allternit
/// Android app (PhoneViewport.tsx on the web): watch, then "Allow taps" to tap,
/// long press, swipe, type and press Back / Home / Recents. Opened from the
/// phone's row in ACI › Computers; an iPhone row never opens it.
struct PhoneLiveView: View {
    @StateObject private var session: PhoneLiveSession
    @Environment(\.dismiss) private var dismiss
    @State private var typed = ""
    @State private var touching = false
    @State private var lastMove = Date.distantPast
    /// Where a tap just landed on the shown screen (view coordinates). Bot taps would set this
    /// too; their x/y do not reach the app yet (see PhoneTapMarker).
    @State private var tapMark: PhoneTapMark?

    init(phone: ACIComputersClient.CloudPhone) {
        _session = StateObject(wrappedValue: PhoneLiveSession(phone: phone))
    }

    private var name: String { session.phone.name.isEmpty ? "your phone" : session.phone.name }
    private var tapsEnabled: Bool { session.hasControl && session.phone.canTap == true }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                accessBar
                ZStack {
                    Color.black
                    AnnexBDisplayLayer { session.attach($0) }
                        .opacity(session.state == .live ? 1 : 0)
                    if session.state == .live && tapsEnabled { touchSurface }
                    PhoneTapMarker(mark: tapMark)
                    overlay
                }
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                if tapsEnabled { controls }
            }
            .background(Color("BgPanel").ignoresSafeArea())
            .navigationTitle(name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel("Close")
                }
            }
        }
        .task { session.start() }
        .onDisappear { session.stop() }
    }

    // MARK: Pieces

    @ViewBuilder private var accessBar: some View {
        if session.state == .live && !session.hasControl {
            HStack(spacing: 8) {
                Image(systemName: "eye")
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
                if session.phone.canTap == true {
                    Text("Watching only. Allow taps to tap and type on \(name).")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ACIPillButton(title: session.controlBusy ? "Allowing…" : "Allow taps") {
                        Task { await session.requestControl() }
                    }
                    .disabled(session.controlBusy)
                } else {
                    Text("Watching only. \(PhoneLiveCopy.tapsOff(name))")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .accessibilityElement(children: .contain)
        }
        if let notice = session.notice {
            Text(notice)
                .font(.system(size: 13))
                .foregroundColor(Theme.statusError)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.bottom, 4)
        }
    }

    @ViewBuilder private var overlay: some View {
        switch session.state {
        case .live:
            EmptyView()
        case .connecting:
            message("Connecting…", retry: false)
        case .waiting:
            message(PhoneLiveCopy.waiting(name), retry: false)
        case .ended(let reason):
            message(PhoneLiveCopy.ended(reason: reason, name: name), retry: true)
        case .error(let text):
            message(text, retry: true)
        case .relayOffline:
            message(PhoneLiveCopy.relayOffline, retry: true)
        case .signedOut:
            message(PhoneLiveCopy.signedOut, retry: false)
        }
    }

    private func message(_ text: String, retry: Bool) -> some View {
        VStack(spacing: 12) {
            if !retry && session.state != .signedOut { ProgressView().tint(.white) }
            Text(text)
                .font(.system(size: 14))
                .multilineTextAlignment(.center)
                .foregroundColor(.white)
            if retry {
                ACIPillButton(title: "Retry") { session.start() }
            }
        }
        .padding(24)
        .accessibilityElement(children: .combine)
    }

    /// Tap, long press and swipe are one drag: down on touch, moves while it
    /// travels, up on release, with the time held in between.
    private var touchSurface: some View {
        GeometryReader { geo in
            Color.clear
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            guard let p = PhoneLiveGeometry.videoPoint(value.location, view: geo.size, video: session.videoSize, clamp: touching) else { return }
                            if !touching {
                                touching = true
                                lastMove = Date()
                                session.send([.touch(.down, x: Int(p.x), y: Int(p.y))])
                            } else if Date().timeIntervalSince(lastMove) >= 0.03 {
                                lastMove = Date()
                                session.send([.touch(.move, x: Int(p.x), y: Int(p.y))])
                            }
                        }
                        .onEnded { value in
                            guard touching else { return }
                            touching = false
                            tapMark = PhoneTapMark(point: value.location)
                            if let p = PhoneLiveGeometry.videoPoint(value.location, view: geo.size, video: session.videoSize, clamp: true) {
                                session.send([.touch(.up, x: Int(p.x), y: Int(p.y))])
                            }
                        }
                )
        }
        .accessibilityLabel("\(name) screen. Tap, hold or swipe to control it.")
    }

    private var controls: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                TextField("Type on \(name)", text: $typed)
                    .textFieldStyle(.roundedBorder)
                    .submitLabel(.send)
                    .onSubmit(sendText)
                ACIPillButton(title: "Send") { sendText() }
                    .disabled(typed.isEmpty)
            }
            HStack(spacing: 8) {
                navButton("Back", "chevron.backward", .back)
                navButton("Home", "circle", .home)
                navButton("Recents", "square.on.square", .appSwitch)
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
    }

    private func navButton(_ title: String, _ symbol: String, _ event: PhoneInput) -> some View {
        Button { session.send([event]) } label: {
            Label(title, systemImage: symbol)
                .font(.system(size: 14, weight: .medium))
                .frame(maxWidth: .infinity, minHeight: 44)
                .foregroundColor(Color("TextPrimary"))
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(title) on \(name)")
    }

    private func sendText() {
        guard !typed.isEmpty else { return }
        session.send([.text(typed)])
        typed = ""
    }
}

/// A tap's place on the live screen, in the screen view's coordinates. `id` makes each tap new.
struct PhoneTapMark: Equatable {
    let id = UUID()
    let point: CGPoint
}

/// A ring where a tap landed: grows in and fades in about 0.5 s; under Reduce Motion a still ring
/// that fades. Only taps made in this view feed it today. The bot's tap x/y live in phone_runs
/// Step.args on the runtime but the trail the app reads (RunStep) carries only n, label, ok, map, so
/// bot taps need that to reach the app first.
struct PhoneTapMarker: View {
    let mark: PhoneTapMark?
    @ReducedMotion private var reduceMotion
    @State private var shownId: UUID?
    @State private var grown = false

    var body: some View {
        ZStack {
            if let mark, shownId == mark.id {
                Circle()
                    .strokeBorder(Color.white, lineWidth: 2)
                    .background(Circle().fill(Color.white.opacity(0.18)))
                    .frame(width: 36, height: 36)
                    .scaleEffect(reduceMotion ? 1 : (grown ? 1.35 : 0.5))
                    .opacity(grown ? 0 : 1)
                    .position(mark.point)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onChange(of: mark) { _, new in
            guard let new else { return }
            shownId = new.id
            grown = false
            withAnimation(.easeOut(duration: reduceMotion ? 0.3 : 0.5)) { grown = true }
        }
    }
}
