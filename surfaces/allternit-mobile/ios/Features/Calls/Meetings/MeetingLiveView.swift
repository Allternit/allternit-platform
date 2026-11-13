import SwiftUI

/// The live meeting page (Eoj's pick on 11-13: the live voice view look).
/// Gizzi's mascot on the voice stage with the state word under it, then the
/// Transcript / Artifacts / Approvals panel, and the Stop listening / Speak
/// replies / Leave controls. State chip: Joining / Waiting to be let in /
/// Live · elapsed / Not let in / Recap ready.
struct MeetingLiveView: View {
    @StateObject private var live: MeetingLiveSession
    @State private var tab: Tab = .transcript
    @State private var confirmLeave = false
    @State private var safariURL: IdentifiableURL?
    @State private var showRecap = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    enum Tab: String, CaseIterable, Identifiable {
        case transcript = "Transcript", artifacts = "Artifacts", approvals = "Approvals"
        var id: String { rawValue }
    }

    init(sessionId: String, meeting: Meeting?) {
        _live = StateObject(wrappedValue: MeetingLiveSession(sessionId: sessionId, meeting: meeting))
    }

    var body: some View {
        Group {
            switch live.phase {
            case .loading where live.session == nil:
                VStack(spacing: 14) {
                    SetupGizzi(size: 88, emotion: .curious)
                    ProgressView()
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .failed(let message) where live.session == nil:
                FriendlyStateView(style: .offline, icon: "wifi.slash", title: "Couldn't open the meeting",
                                  message: message, actionTitle: "Retry") { Task { await live.loadAll() } }
                    .frame(maxHeight: .infinity)
            case .paidPlan:
                ScrollView { MeetingsPaidPlanCard().padding(16) }
            case .signedOut:
                FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to use Meetings",
                                  message: "Your session ended. Sign in again to see this meeting.")
                    .frame(maxHeight: .infinity)
            default:
                page
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle(live.meeting?.displayTitle ?? "Meeting")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if live.session != nil {
                    MeetingStateChip(status: live.status, session: live.session, meeting: live.meeting)
                        .accessibilityIdentifier("meetingStateChip")
                }
            }
        }
        .onAppear { live.start() }
        .onDisappear { live.stop() }
        .confirmationDialog("Gizzi leaves the call", isPresented: $confirmLeave, titleVisibility: .visible) {
            Button("Leave", role: .destructive) { Task { await live.leave() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The transcript so far is kept and the recap is written.")
        }
        .alert("Meetings", isPresented: Binding(
            get: { live.actionError != nil }, set: { if !$0 { live.actionError = nil } }
        )) {
            Button("OK", role: .cancel) { live.actionError = nil }
        } message: { Text(live.actionError ?? "") }
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
        .navigationDestination(isPresented: $showRecap) {
            if let meeting = live.meeting { MeetingRecapView(meeting: meeting, session: live.session) }
        }
        #if DEBUG
        .task {
            if launchArgumentEnabled("open-meeting-approvals") { tab = .approvals }
            if launchArgumentEnabled("open-meeting-artifacts") { tab = .artifacts }
        }
        #endif
    }

    // MARK: - Page

    private var page: some View {
        VStack(spacing: 0) {
            stage
                .frame(maxWidth: .infinity)
                .padding(.top, 6)
            Picker("Panel", selection: $tab) {
                ForEach(Tab.allCases) { t in
                    Text(t == .approvals && !live.approvals.isEmpty ? "Approvals · \(live.approvals.count)" : t.rawValue).tag(t)
                }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.top, 10)
            panel
                .frame(maxHeight: .infinity)
            controls
                .padding(.horizontal, 24)
                .padding(.top, 8)
                .padding(.bottom, 12)
        }
    }

    private var stage: some View {
        VStack(spacing: 8) {
            VoiceSpeakerView(phase: speakerPhase, muted: live.status == .paused, style: .ring,
                             palette: .spectrum, botColor: nil, size: 96, still: live.isEnded) {
                SetupGizzi(size: 96, emotion: live.status == .live ? .pleased : (live.isEnded ? .focused : .curious))
            }
            .frame(height: 96 * VoiceSpeakerView<SetupGizzi>.scale * 0.92)
            .accessibilityHidden(true)
            Text(stageWord.uppercased())
                .font(.system(size: 12, weight: .semibold))
                .tracking(1.6)
                .foregroundColor(Color("TextSecondary"))
                .accessibilityLabel(stageWord)
            Text(stageDetail)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .lineLimit(2)
                .padding(.horizontal, 32)
        }
    }

    private var speakerPhase: VoiceSpeakerView<SetupGizzi>.Phase {
        switch live.status {
        case .launching, .joining, .waitingAdmit, .scheduled: return .connecting
        case .live: return live.lastReply != nil && live.session?.speakReplies == true ? .speaking : .listening
        default: return .idle
        }
    }

    private var stageWord: String {
        switch live.status {
        case .live: return (live.session?.listening ?? true) ? "Listening" : "Paused"
        case .paused: return "Paused"
        case .launching, .joining, .scheduled, .off: return "Joining"
        case .waitingAdmit: return live.meeting?.platform == "zoom" ? "Waiting for you to join Zoom" : "Waiting to be let in"
        case .notAdmitted: return "Not let in"
        case .ended: return live.session?.summary != nil ? "Recap ready" : "Left the call"
        }
    }

    private var stageDetail: String {
        let platform = live.meeting?.platformName ?? "the call"
        switch live.status {
        case .live:
            if let reply = live.lastReply { return "Gizzi: \(reply)" }
            return "Say “Gizzi, …” in \(platform) to ask for something."
        case .paused: return "Gizzi is in the call but not listening."
        case .waitingAdmit where live.meeting?.platform == "zoom":
            return "Zoom lets Gizzi in once you're in the call. Join it from Zoom."
        case .waitingAdmit: return "Let “Gizzi (Allternit)” in from \(platform)."
        case .launching, .joining, .scheduled, .off: return "Gizzi is joining \(platform)."
        case .notAdmitted: return MeetingFormat.endReason(live.session?.endReason ?? "not_admitted")
        case .ended:
            return live.session?.summary != nil ? "Summary, decisions and action items are ready." : MeetingFormat.endReason(live.session?.endReason)
        }
    }

    // MARK: - Panel

    @ViewBuilder
    private var panel: some View {
        switch tab {
        case .transcript: transcriptList
        case .artifacts: artifactsList
        case .approvals: approvalsList
        }
    }

    private var transcriptList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if live.transcript.isEmpty {
                        panelEmpty(live.status == .live ? "The transcript shows here as people talk." : "No transcript yet.")
                    }
                    ForEach(live.transcript) { segment in
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(segment.speaker ?? "Someone")
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                Text(Self.offset(segment.startMs))
                                    .font(.system(size: 11).monospacedDigit())
                                    .foregroundColor(Color("TextTertiary"))
                            }
                            Text(segment.text)
                                .font(.system(size: 15))
                                .foregroundColor(Color("TextSecondary"))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .id(segment.seq)
                        .accessibilityElement(children: .combine)
                    }
                }
                .padding(16)
            }
            .onChange(of: live.transcript.last?.seq) { _, last in
                guard let last else { return }
                if reduceMotion { proxy.scrollTo(last, anchor: .bottom) } else {
                    withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(last, anchor: .bottom) }
                }
            }
        }
    }

    private var artifactsList: some View {
        ScrollView {
            VStack(spacing: 10) {
                if live.artifacts.isEmpty {
                    panelEmpty("Docs, slides and canvases Gizzi makes in the call show here.")
                }
                ForEach(live.artifacts) { artifact in
                    MeetingArtifactRow(artifact: artifact) {
                        if let url = URL(string: artifact.url) { safariURL = IdentifiableURL(url: url) }
                    }
                }
            }
            .padding(16)
        }
    }

    private var approvalsList: some View {
        ScrollView {
            VStack(spacing: 10) {
                if live.approvals.isEmpty {
                    panelEmpty("When someone asks Gizzi to use one of your tools, you approve it here. Names in a call aren't verified.")
                }
                ForEach(live.approvals) { approval in
                    VStack(alignment: .leading, spacing: 10) {
                        Text(approval.argsSummary)
                            .font(.system(size: 15))
                            .foregroundColor(Color("TextPrimary"))
                            .fixedSize(horizontal: false, vertical: true)
                        Text([approval.askedBy.map { "Asked by \($0)" }, approval.tool].compactMap { $0 }.joined(separator: " · "))
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                        HStack(spacing: 8) {
                            Button("Allow") { Task { await live.decide(approval, approve: true) } }
                                .buttonStyle(SettingsPrimaryButtonStyle())
                            Button("Deny") { Task { await live.decide(approval, approve: false) } }
                                .buttonStyle(SettingsQuietButtonStyle())
                        }
                    }
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
                }
            }
            .padding(16)
        }
    }

    private func panelEmpty(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 14))
            .foregroundColor(Color("TextTertiary"))
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity)
            .padding(.top, 24)
    }

    private static func offset(_ ms: Int) -> String {
        let s = ms / 1000
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    // MARK: - Controls

    @ViewBuilder
    private var controls: some View {
        if live.isEnded {
            if live.session?.summary != nil {
                Button("See the recap") { showRecap = true }
                    .buttonStyle(SettingsPrimaryButtonStyle())
            }
        } else {
            HStack {
                controlButton(
                    symbol: live.status == .paused ? "mic.fill" : "mic.slash",
                    caption: live.status == .paused ? "Resume" : "Stop listening",
                    active: live.status == .paused,
                    busy: live.busyAction == "stop_listening" || live.busyAction == "resume",
                    enabled: live.status == .live || live.status == .paused
                ) { Task { await live.toggleListening() } }
                Spacer()
                controlButton(
                    symbol: live.session?.speakReplies == true ? "speaker.wave.2.fill" : "speaker.slash",
                    caption: "Speak replies",
                    active: live.session?.speakReplies == true,
                    busy: live.busyAction == "speak",
                    enabled: live.session != nil
                ) { Task { await live.setSpeakReplies(!(live.session?.speakReplies ?? false)) } }
                .accessibilityValue(live.session?.speakReplies == true ? "On" : "Off")
                Spacer()
                controlButton(symbol: "phone.down.fill", caption: "Leave", destructive: true,
                              busy: live.busyAction == "leave", enabled: true) { confirmLeave = true }
            }
            .frame(maxWidth: 320)
            .frame(maxWidth: .infinity)
        }
    }

    private func controlButton(symbol: String, caption: String, active: Bool = false, destructive: Bool = false,
                               busy: Bool, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                ZStack {
                    Circle()
                        .fill(destructive ? Theme.statusError : (active ? Color("TextPrimary") : Color("FillNeutral")))
                        .frame(width: 56, height: 56)
                    if busy {
                        ProgressView().tint(destructive || active ? Color("BgPrimary") : Color("TextPrimary"))
                    } else {
                        Image(systemName: symbol)
                            .font(.system(size: 20, weight: .medium))
                            .foregroundColor(destructive ? .white : (active ? Color("BgPrimary") : Color("TextPrimary")))
                    }
                }
                Text(caption)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(minWidth: 84)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled || busy)
        .opacity(enabled ? 1 : 0.45)
        .accessibilityLabel(caption)
    }
}

struct MeetingArtifactRow: View {
    let artifact: MeetingArtifact
    let onOpen: () -> Void

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 18))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 36, height: 36)
                    .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Color("BgPanel")))
                VStack(alignment: .leading, spacing: 2) {
                    Text(artifact.title)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(kindLabel + ((artifact.version ?? 1) > 1 ? " · version \(artifact.version ?? 1)" : ""))
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                }
                Spacer(minLength: 0)
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(12)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens the \(kindLabel.lowercased())")
    }

    private var icon: String {
        switch artifact.kind {
        case "slides": return "rectangle.on.rectangle"
        case "canvas": return "scribble.variable"
        default: return "doc.text"
        }
    }

    private var kindLabel: String {
        switch artifact.kind {
        case "slides": return "Slides"
        case "canvas": return "Canvas"
        default: return "Doc"
        }
    }
}
