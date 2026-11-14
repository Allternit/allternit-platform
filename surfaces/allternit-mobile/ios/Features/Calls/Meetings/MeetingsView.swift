import SwiftUI
import UIKit

/// Where the Meetings sheet navigates.
enum MeetingRoute: Hashable {
    case live(sessionId: String, meeting: Meeting)
    case recap(Meeting)
}

/// Meetings, the iPhone shape of the web's Home > Meetings (Eoj's pick on
/// 11-13: the agenda list). The live call sits on top, each upcoming call
/// has a Gizzi switch (`PATCH auto_join`), past calls open their recap, and
/// "Paste a call link" sends Gizzi to any Meet / Zoom / Teams call. Opened
/// from the drawer's Meetings button (no rail tab), a notification, or the
/// Live Activity (`allternit://meetings/<id>`).
struct MeetingsView: View {
    /// Open this meeting's live page or recap once loaded (deep link).
    var focusMeetingId: String? = nil

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = MeetingsStore.shared
    @State private var path = NavigationPath()
    @State private var isPastePresented = false
    @State private var sendTarget: Meeting?
    @State private var focusHandled = false

    var body: some View {
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    content
                }
                .padding(EdgeInsets(top: 4, leading: 16, bottom: 32, trailing: 16))
            }
            .refreshable { await store.load(quiet: true) }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Meetings")
            .navigationBarTitleDisplayMode(.inline)
            // The sheet's root has ✕ only; pushed pages get a bare ‹.
            .navigationBarBackButtonHidden(true)
            .toolbarRole(.editor)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    MeetingRoundButton(systemImage: "xmark", label: "Close meetings") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    if store.phase == .loaded {
                        MeetingRoundButton(systemImage: "link.badge.plus", label: "Paste a call link") { isPastePresented = true }
                            .accessibilityIdentifier("meetingsPasteLink")
                    }
                }
            }
            .navigationDestination(for: MeetingRoute.self) { route in
                switch route {
                case .live(let sid, let meeting): MeetingLiveView(sessionId: sid, meeting: meeting)
                case .recap(let meeting): MeetingRecapView(meeting: meeting)
                }
            }
            .sheet(isPresented: $isPastePresented) {
                MeetingPasteLinkSheet { meeting, session in
                    if let session { path.append(MeetingRoute.live(sessionId: session.id, meeting: meeting)) }
                }
            }
            .confirmationDialog(sendTarget?.displayTitle ?? "", isPresented: Binding(
                get: { sendTarget != nil }, set: { if !$0 { sendTarget = nil } }
            ), titleVisibility: .visible, presenting: sendTarget) { meeting in
                Button("Send Gizzi now") {
                    Task {
                        if let session = await store.sendNow(meeting) {
                            path.append(MeetingRoute.live(sessionId: session.id, meeting: meeting))
                        }
                    }
                }
                if let url = URL(string: meeting.joinUrl) {
                    Button("Open in \(meeting.platformName)") { UIApplication.shared.open(url) }
                }
                Button("Cancel", role: .cancel) {}
            } message: { meeting in
                Text("Gizzi joins as “Gizzi (Allternit)” and posts a notice in the call chat.")
            }
            .alert("Meetings", isPresented: Binding(
                get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } }
            )) {
                if store.actionError == MeetingsError.zoomNotConnected.localizedDescription {
                    Button("Connect Zoom") { store.actionError = nil; Task { await store.connectZoom() } }
                }
                Button("OK", role: .cancel) { store.actionError = nil }
            } message: {
                Text(store.actionError ?? "")
            }
            .task {
                await store.load(quiet: store.phase == .loaded)
                await openFocus()
                #if DEBUG
                openDebugRoute()
                #endif
            }
        }
    }

    // MARK: - States

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .idle:
            MeetingsSkeleton()
        case .loading where store.live.isEmpty && store.upcoming.isEmpty && store.past.isEmpty:
            MeetingsSkeleton()
        case .signedOut:
            FriendlyStateView(style: .empty, icon: "person.crop.circle",
                              title: "Sign in to use Meetings",
                              message: "Gizzi joins your calls, takes notes and makes what you ask for, once you're signed in.")
                .padding(.top, 40)
        case .paidPlan:
            MeetingsPaidPlanCard().padding(.top, 12)
        case .forbidden(let message):
            FriendlyStateView(style: .error, icon: "building.2", title: "Meetings is off for your organization", message: message)
                .padding(.top, 40)
        case .failed(let message) where store.live.isEmpty && store.upcoming.isEmpty && store.past.isEmpty:
            FriendlyStateView(style: .offline, icon: "wifi.slash", title: "Couldn't load meetings",
                              message: message, actionTitle: "Retry") { Task { await store.load() } }
                .padding(.top, 40)
        default:
            agenda
        }
    }

    @ViewBuilder
    private var agenda: some View {
        if store.connectionsLoaded && store.connections.isEmpty || store.pickRule {
            MeetingsCalendarConnectCard()
                .padding(.top, 8)
        }

        if let runner = store.runner {
            MeetingRunnerNote(runner: runner)
                .padding(.top, 12)
        }

        ForEach(store.live) { meeting in
            MeetingLiveCard(meeting: meeting, session: store.session(for: meeting)) {
                if let sid = store.session(for: meeting)?.id ?? meeting.sessionId {
                    path.append(MeetingRoute.live(sessionId: sid, meeting: meeting))
                }
            }
            .padding(.top, 12)
        }

        if store.upcoming.isEmpty && store.live.isEmpty {
            MeetingsEmptyUpcoming(hasCalendar: !store.connections.isEmpty) { isPastePresented = true }
                .padding(.top, 12)
        }

        ForEach(dayGroups, id: \.label) { group in
            PhoneSettingsLabel(text: group.label)
            PhoneSettingsCard {
                ForEach(Array(group.meetings.enumerated()), id: \.element.id) { index, meeting in
                    if index > 0 { PhoneCardDivider() }
                    MeetingAgendaRow(
                        meeting: meeting,
                        isPending: store.pendingSwitches.contains(meeting.id),
                        onToggle: { on in Task { await store.setGizzi(meeting, on: on) } },
                        onTap: { sendTarget = meeting }
                    )
                }
            }
        }

        if !store.past.isEmpty {
            PhoneSettingsLabel(text: "Past")
            PhoneSettingsCard {
                ForEach(Array(store.past.prefix(30).enumerated()), id: \.element.id) { index, meeting in
                    if index > 0 { PhoneCardDivider() }
                    MeetingPastRow(meeting: meeting, session: store.session(for: meeting)) {
                        path.append(MeetingRoute.recap(meeting))
                    }
                }
            }
        }
    }

    private struct DayGroup { let label: String; let meetings: [Meeting] }

    private var dayGroups: [DayGroup] {
        var order: [String] = []
        var groups: [String: [Meeting]] = [:]
        for m in store.upcoming {
            let label = MeetingFormat.dayLabel(m.startDate)
            if groups[label] == nil { order.append(label) }
            groups[label, default: []].append(m)
        }
        return order.map { DayGroup(label: $0, meetings: groups[$0] ?? []) }
    }

    // MARK: - Focus (deep link)

    private func openFocus() async {
        guard let id = focusMeetingId, !focusHandled else { return }
        focusHandled = true
        let known = (store.live + store.upcoming + store.past).first { $0.id == id }
        var found = known
        if found == nil { found = try? await MeetingsClient.shared.meeting(id) }
        guard let meeting = found else { return }
        if meeting.status.isActive, let sid = store.session(for: meeting)?.id ?? meeting.sessionId {
            path.append(MeetingRoute.live(sessionId: sid, meeting: meeting))
        } else if meeting.status == .ended || meeting.status == .notAdmitted {
            path.append(MeetingRoute.recap(meeting))
        }
    }

    #if DEBUG
    /// `-open-meeting-live` / `-open-meeting-recap` / `-open-meetings-paste`
    /// (DEBUG): jump to a screen for screenshots.
    private func openDebugRoute() {
        if launchArgumentEnabled("open-meeting-live"), let m = store.live.first,
           let sid = store.session(for: m)?.id ?? m.sessionId {
            path.append(MeetingRoute.live(sessionId: sid, meeting: m))
        } else if launchArgumentEnabled("open-meeting-recap"), let m = store.past.first {
            path.append(MeetingRoute.recap(m))
        } else if launchArgumentEnabled("open-meetings-paste") {
            isPastePresented = true
        }
    }
    #endif
}

// MARK: - Rows

/// The live call on top: mascot, title, chip with elapsed time, Open.
/// One line above the agenda: which computer Gizzi joins from. When none is
/// online it also offers a cloud computer.
struct MeetingRunnerNote: View {
    let runner: MeetingRunner

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(runner.text)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if runner.isNone {
                Button(MeetingRunner.noneAction) {
                    DeepLinkRouter.shared.enqueue(.computers)
                }
                .buttonStyle(.plain)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 4)
        .accessibilityIdentifier("meetingsRunnerNote")
    }
}

struct MeetingLiveCard: View {
    let meeting: Meeting
    let session: MeetingSession?
    let onOpen: () -> Void

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 14) {
                SetupGizzi(size: 44, emotion: (session?.status ?? meeting.status) == .live ? .pleased : .curious)
                VStack(alignment: .leading, spacing: 4) {
                    Text(meeting.displayTitle)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    MeetingStateChip(status: session?.status ?? meeting.status, session: session, meeting: meeting)
                    HStack(spacing: 6) {
                        MeetingPlatformMark(platform: meeting.platform)
                        Text(detail)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                Text("Open")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("BgPrimary"))
                    .padding(.horizontal, 14)
                    .frame(minHeight: 34)
                    .background(Capsule().fill(Color("TextPrimary")))
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityHint("Opens the live meeting")
        .accessibilityIdentifier("meetingLiveCard")
    }

    private var detail: String {
        let status = session?.status ?? meeting.status
        switch status {
        case .live: return ((session?.listening ?? true) ? "Gizzi is listening" : "Gizzi stopped listening") + " · \(meeting.platformName)"
        case .paused: return "Gizzi stopped listening · \(meeting.platformName)"
        case .waitingAdmit where meeting.platform == "zoom": return "Join the Zoom call so Gizzi can come in"
        case .waitingAdmit: return "Let “Gizzi (Allternit)” in from \(meeting.platformName)"
        default: return "Gizzi is joining \(meeting.platformName)"
        }
    }
}

/// An upcoming call: time, title, platform and people, the Gizzi switch.
struct MeetingAgendaRow: View {
    let meeting: Meeting
    let isPending: Bool
    let onToggle: @Sendable (Bool) -> Void
    let onTap: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Button(action: onTap) {
                HStack(spacing: 12) {
                    Text(MeetingFormat.time(meeting.startDate))
                        .font(.system(size: 14).monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 66, alignment: .leading)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(meeting.displayTitle)
                            .font(.system(size: 16, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        HStack(spacing: 6) {
                            MeetingPlatformMark(platform: meeting.platform)
                            Text(subtitle)
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextSecondary"))
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 4)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint("Send Gizzi now or open the call")

            Toggle("Gizzi joins \(meeting.displayTitle)", isOn: Binding(get: { meeting.gizziOn }, set: onToggle))
                .labelsHidden()
                .tint(Color("BrandAccent"))
                .disabled(isPending)
                .accessibilityIdentifier("meetingSwitch-\(meeting.id)")
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 60)
    }

    private var subtitle: String {
        var parts = [meeting.platformName]
        if let people = meeting.peopleLabel, !people.isEmpty { parts.append(people) }
        if !meeting.gizziOn, meeting.isOneOnOne == true, meeting.autoJoin == nil { parts.append("Skipped: 1:1") }
        return parts.joined(separator: " · ")
    }
}

/// A past call: when, title, recap line, Recap.
struct MeetingPastRow: View {
    let meeting: Meeting
    let session: MeetingSession?
    let onOpen: () -> Void

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 12) {
                Text(MeetingFormat.shortDay(meeting.startDate ?? session?.endedDate))
                    .font(.system(size: 13).monospacedDigit())
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 66, alignment: .leading)
                VStack(alignment: .leading, spacing: 3) {
                    Text(meeting.displayTitle)
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(MeetingFormat.recapLine(meeting: meeting, session: session))
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                Text("Recap")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 30)
                    .background(Capsule().fill(Color("FillNeutral")))
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 60)
            .contentShape(Rectangle())
        }
        .buttonStyle(PhoneRowButtonStyle())
        .accessibilityElement(children: .combine)
        .accessibilityHint("Opens the recap")
    }
}

/// No calls ahead: what to do next.
private struct MeetingsEmptyUpcoming: View {
    let hasCalendar: Bool
    let onPaste: () -> Void

    var body: some View {
        VStack(spacing: 10) {
            SetupGizzi(size: 56, emotion: .curious)
            Text("No calls coming up")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Text(hasCalendar
                 ? "Calls with a Meet, Zoom or Teams link on your calendar show here."
                 : "Connect a calendar above, or paste a call link to send Gizzi now.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
            Button("Paste a call link", action: onPaste)
                .buttonStyle(SettingsQuietButtonStyle())
                .padding(.top, 4)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 24)
        .padding(.horizontal, 16)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
    }
}

/// Loading: three placeholder rows in the agenda's shape.
private struct MeetingsSkeleton: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")).frame(height: 86)
            RoundedRectangle(cornerRadius: 4).fill(Color("FillNeutral")).frame(width: 60, height: 12).padding(.top, 14)
            RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")).frame(height: 180)
        }
        .padding(.top, 12)
        .opacity(dim ? 0.55 : 1)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { dim = true }
        }
        .accessibilityElement()
        .accessibilityLabel("Loading meetings")
    }
}

/// Free plan (402): what Meetings is, and the plan link where allowed.
struct MeetingsPaidPlanCard: View {
    @ObservedObject private var storefront = StorefrontGate.shared
    @State private var safariURL: IdentifiableURL?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                SetupGizzi(size: 44, emotion: .focused)
                Text("Gizzi in your calls")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
            Text("Meetings is part of the Plus, Super and Ultra plans. Gizzi joins your Meet, Zoom and Teams calls, writes the transcript, answers in the chat and sends a recap.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
            if storefront.allowsPurchaseLinks {
                Button("Choose a plan") { safariURL = IdentifiableURL(url: StorefrontGate.upgradeURL) }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .padding(.top, 4)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
    }
}

// MARK: - Shared pieces

/// Joining / Waiting to be let in / Live · 12 min / Not let in / Recap ready.
struct MeetingStateChip: View {
    let status: GizziStatus
    var session: MeetingSession? = nil
    /// Fallback for `live_at` and the platform when no session is loaded.
    var meeting: Meeting? = nil

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(tint).frame(width: 7, height: 7)
            if status == .live, let since = session?.liveDate ?? meeting?.liveDate {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    Text("Live · \(MeetingFormat.minutes(from: since, to: context.date))")
                }
            } else {
                Text(MeetingFormat.stateLabel(status, session: session, platform: meeting?.platform))
            }
        }
        .font(.system(size: 12, weight: .semibold))
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .background(Capsule().fill(tint.opacity(0.16)))
        .accessibilityElement(children: .combine)
    }

    private var tint: Color { MeetingFormat.tint(status, session: session) }
}

/// Small real brand mark placed inline before the platform name. The visible text carries the meaning.
struct MeetingPlatformMark: View {
    let platform: String
    var size: CGFloat = 14

    var body: some View {
        if let name = asset {
            Image(name)
                .resizable()
                .scaledToFit()
                .frame(width: size, height: size)
                .clipShape(RoundedRectangle(cornerRadius: 3, style: .continuous))
                .accessibilityHidden(true)
        }
    }

    private var asset: String? {
        switch platform {
        case "meet": return "applogo-googlemeet"
        case "zoom": return "applogo-zoom"
        case "teams": return "applogo-microsoftteams"
        default: return nil
        }
    }
}

/// `.phone-round-btn`: 40pt filled circle.
struct MeetingRoundButton: View {
    let systemImage: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 17, weight: .regular))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 40, height: 40)
                .background(Circle().fill(Color("FillNeutral")))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

/// The drawer footer's Meetings button (next to Phones), shown only with the
/// `meetings` flag. A green dot while Gizzi is in a call.
struct MeetingsDrawerButton: View {
    @ObservedObject private var store = MeetingsStore.shared
    let action: () -> Void

    var body: some View {
        Group {
            if store.isEnabled { button }
        }
        .task { await store.refreshIfStale() }
    }

    private var button: some View {
        Group {
            Button {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                action()
            } label: {
                Image(systemName: "video")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(Color("FillNeutral")))
                    .overlay(alignment: .topTrailing) {
                        if store.activeMeeting != nil {
                            Circle().fill(Theme.statusSuccess)
                                .frame(width: 10, height: 10)
                                .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                        }
                    }
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(store.activeMeeting != nil ? "Meetings, Gizzi is in a call" : "Meetings")
            .accessibilityIdentifier("drawerMeetings")
        }
    }
}

enum MeetingFormat {
    static func stateLabel(_ status: GizziStatus, session: MeetingSession?, platform: String? = nil) -> String {
        switch status {
        case .waitingAdmit where (session?.platform ?? platform) == "zoom":
            // CONTRACT (Zoom change): the OBF join waits for the account owner.
            return "Waiting for you to join Zoom"
        case .scheduled: return "Gizzi joins"
        case .off: return "Off"
        case .launching, .joining: return "Joining"
        case .waitingAdmit: return "Waiting to be let in"
        case .live: return "Live"
        case .paused: return "Paused"
        case .notAdmitted: return "Not let in"
        case .ended: return session?.summary != nil ? "Recap ready" : "Ended"
        }
    }

    static func tint(_ status: GizziStatus, session: MeetingSession?) -> Color {
        switch status {
        case .live: return Theme.statusSuccess
        case .launching, .joining, .waitingAdmit: return Theme.statusWarning
        case .notAdmitted: return Theme.statusError
        case .ended where session?.summary != nil: return Color("BrandAccent")
        default: return Color("TextTertiary")
        }
    }

    static func endReason(_ reason: String?) -> String {
        switch reason {
        case "left_by_user": return "You asked Gizzi to leave."
        case "removed": return "Gizzi was removed from the call."
        case "not_admitted": return "Nobody let Gizzi in."
        case "empty_room": return "Everyone left, so Gizzi did too."
        case "max_duration": return "Gizzi left after 8 hours."
        case "other_notetaker": return "Another notetaker joined, so Gizzi left."
        case "join_failed": return "Gizzi couldn't join the call."
        case "guest_blocked": return "The call doesn't let guests like Gizzi in."
        case "no_capacity": return "Gizzi was in too many calls to join."
        case "no_runtime": return "Gizzi could not join. None of your computers was online. Open Allternit on your computer or start your cloud computer, then send Gizzi again."
        case "error": return "Something went wrong in the call."
        default: return "Gizzi left the call."
        }
    }

    static func recapLine(meeting: Meeting, session: MeetingSession?) -> String {
        if meeting.status == .notAdmitted { return "Not let in" }
        guard let session else { return meeting.status == .ended ? "Ended" : "\(meeting.platformName)" }
        if session.summary != nil {
            var parts = ["Recap ready"]
            let d = session.decisions?.count ?? 0, a = session.actionItems?.count ?? 0
            if d > 0 { parts.append("\(d) decision\(d == 1 ? "" : "s")") }
            if a > 0 { parts.append("\(a) action item\(a == 1 ? "" : "s")") }
            return parts.joined(separator: " · ")
        }
        if session.status == .ended, ["left_by_user", "empty_room", "removed", "max_duration"].contains(session.endReason ?? "") {
            return "Writing the recap…"
        }
        return endReason(session.endReason)
    }

    static func minutes(from start: Date, to end: Date) -> String {
        let m = max(0, Int(end.timeIntervalSince(start) / 60))
        return m < 60 ? "\(m) min" : "\(m / 60) h \(m % 60) min"
    }

    static func time(_ date: Date?) -> String {
        guard let date else { return "Now" }
        return date.formatted(date: .omitted, time: .shortened)
    }

    static func dayLabel(_ date: Date?) -> String {
        guard let date else { return "Links you added" }
        let cal = Calendar.current
        if cal.isDateInToday(date) { return "Today" }
        if cal.isDateInTomorrow(date) { return "Tomorrow" }
        return date.formatted(.dateTime.weekday(.wide).month(.abbreviated).day())
    }

    static func shortDay(_ date: Date?) -> String {
        guard let date else { return "" }
        let cal = Calendar.current
        if cal.isDateInToday(date) { return date.formatted(date: .omitted, time: .shortened) }
        if cal.isDateInYesterday(date) { return "Yesterday" }
        return date.formatted(.dateTime.month(.abbreviated).day())
    }

    /// CONTRACT §7, client side for an instant hint; the server decides.
    static func isCallLink(_ text: String) -> Bool {
        let s = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let patterns = [
            #"^https://meet\.google\.com/([a-z]{3}-[a-z]{4}-[a-z]{3}|lookup/[^\s]+)"#,
            #"^https://([a-z0-9-]+\.)?zoom\.(us|com)/(j|my|wc)/"#,
            #"^https://teams\.(microsoft|live)\.com/(l/meetup-join/|meet/)"#,
        ]
        return patterns.contains { s.range(of: $0, options: .regularExpression) != nil }
    }
}

// MARK: - Paste a call link

struct MeetingPasteLinkSheet: View {
    /// Called with the saved meeting and, when sent now, its session.
    let onDone: (Meeting, MeetingSession?) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = MeetingsStore.shared
    @State private var link = ""
    @State private var busy: Bool? = nil
    @State private var error: String?
    @FocusState private var focused: Bool

    private var valid: Bool { MeetingFormat.isCallLink(link) }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 14) {
                Text("Paste a Google Meet, Zoom or Microsoft Teams link. Gizzi joins as “Gizzi (Allternit)” and posts a notice in the call chat.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    TextField("https://meet.google.com/abc-defg-hij", text: $link)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .focused($focused)
                        .font(.system(size: 15))
                        .padding(.horizontal, 12)
                        .frame(minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                        .accessibilityIdentifier("meetingLinkField")
                    PasteButton(payloadType: String.self) { strings in
                        if let first = strings.first { link = first }
                    }
                    .labelStyle(.iconOnly)
                    .buttonBorderShape(.capsule)
                    .tint(Color("TextPrimary"))
                }
                if !link.isEmpty && !valid {
                    Text("That isn't a Meet, Zoom or Teams link.")
                        .font(.system(size: 13))
                        .foregroundColor(Theme.statusError)
                }
                if error == nil, let runner = store.runner, runner.isNone {
                    MeetingRunnerNote(runner: runner)
                }
                if let error {
                    SettingsErrorBanner(message: error)
                    if error == MeetingsError.zoomNotConnected.localizedDescription {
                        Button("Connect Zoom") { Task { await store.connectZoom(); self.error = nil } }
                            .buttonStyle(SettingsQuietButtonStyle())
                    }
                }
                HStack(spacing: 10) {
                    Button { submit(joinNow: true) } label: {
                        HStack(spacing: 6) {
                            if busy == true { ProgressView().tint(Color("OnAccent")) }
                            Text("Send Gizzi now")
                        }
                    }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .disabled(!valid || busy != nil)
                    Button { submit(joinNow: false) } label: {
                        HStack(spacing: 6) {
                            if busy == false { ProgressView() }
                            Text("Save for later")
                        }
                    }
                    .buttonStyle(SettingsQuietButtonStyle())
                    .disabled(!valid || busy != nil)
                }
                Spacer(minLength: 0)
            }
            .padding(16)
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Paste a call link")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Cancel") { dismiss() } }
            }
            .onAppear {
                focused = true
                #if DEBUG
                if launchArgumentEnabled("open-meetings-paste") { link = "https://meet.google.com/abc-defg-hij" }
                #endif
            }
        }
        .presentationDetents([.medium])
    }

    private func submit(joinNow: Bool) {
        busy = joinNow
        error = nil
        Task {
            defer { busy = nil }
            do {
                let (meeting, session) = try await store.addLink(link, joinNow: joinNow)
                dismiss()
                onDone(meeting, session)
            } catch {
                if case .paidPlanRequired? = error as? MeetingsError { store.handle(error); dismiss(); return }
                if case .noRuntime? = error as? MeetingsError { self.error = MeetingRunner.noneText; return }
                self.error = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}
