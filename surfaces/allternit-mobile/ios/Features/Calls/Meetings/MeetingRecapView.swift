import SwiftUI

/// A past call's recap: summary, decisions, action items, what Gizzi made,
/// and the transcript on demand. A call that ended without a summary shows
/// why (CONTRACT §3: "Recap ready" only when `summary` is set).
struct MeetingRecapView: View {
    let meeting: Meeting
    @State private var session: MeetingSession?
    @State private var artifacts: [MeetingArtifact] = []
    @State private var transcript: [TranscriptSegment] = []
    @State private var showTranscript = false
    @State private var loading = true
    @State private var error: String?
    @State private var paidPlan = false
    @State private var safariURL: IdentifiableURL?

    init(meeting: Meeting, session: MeetingSession? = nil) {
        self.meeting = meeting
        _session = State(initialValue: session ?? MeetingsStore.shared.session(for: meeting))
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                if paidPlan {
                    MeetingsPaidPlanCard().padding(.top, 16)
                } else if loading && session == nil {
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                } else if let error, session == nil {
                    FriendlyStateView(style: .offline, icon: "wifi.slash", title: "Couldn't load the recap",
                                      message: error, actionTitle: "Retry") { Task { await load() } }
                        .padding(.top, 24)
                } else if let session {
                    recap(session)
                } else {
                    SettingsDescription(text: "Gizzi wasn't in this call, so there's no recap.")
                        .padding(.top, 16)
                }
            }
            .padding(EdgeInsets(top: 8, leading: 16, bottom: 32, trailing: 16))
        }
        .refreshable { await load() }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Recap")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $safariURL) { item in SafariView(url: item.url) }
        .task { await load() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(meeting.displayTitle)
                .font(.system(size: 22, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            HStack(spacing: 6) {
                MeetingPlatformDot(platform: meeting.platform)
                Text(subtitle)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
            }
            if let session { MeetingStateChip(status: session.status, session: session).padding(.top, 4) }
        }
    }

    private var subtitle: String {
        var parts = [meeting.platformName]
        if let s = session?.liveDate, let e = session?.endedDate { parts.append(MeetingFormat.minutes(from: s, to: e)) }
        if let start = meeting.startDate ?? session?.liveDate {
            parts.append(start.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day().hour().minute()))
        }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func recap(_ session: MeetingSession) -> some View {
        if let summary = session.summary {
            SettingsSectionHeading(text: "Summary")
            Text(summary)
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)

            if let decisions = session.decisions, !decisions.isEmpty {
                SettingsSectionHeading(text: "Decisions")
                PhoneSettingsCard {
                    ForEach(Array(decisions.enumerated()), id: \.offset) { i, d in
                        if i > 0 { PhoneCardDivider() }
                        bullet(symbol: "checkmark.seal", text: d, detail: nil)
                    }
                }
            }
            if let items = session.actionItems, !items.isEmpty {
                SettingsSectionHeading(text: "Action items")
                PhoneSettingsCard {
                    ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                        if i > 0 { PhoneCardDivider() }
                        bullet(symbol: "circle", text: item.text,
                               detail: [item.owner, item.due.flatMap(Self.dueLabel)].compactMap { $0 }.joined(separator: " · "))
                    }
                }
            }
        } else {
            SettingsSectionHeading(text: session.status == .ended ? "No recap" : "In progress")
            SettingsDescription(text: session.status.isActive
                                ? "Gizzi is still in this call. The recap is written when it ends."
                                : MeetingFormat.recapLine(meeting: meeting, session: session) == "Writing the recap…"
                                    ? "Gizzi is writing the recap. Pull down to check again."
                                    : MeetingFormat.endReason(session.endReason))
        }

        if !artifacts.isEmpty {
            SettingsSectionHeading(text: "Made in the call")
            VStack(spacing: 8) {
                ForEach(artifacts) { a in
                    MeetingArtifactRow(artifact: a) {
                        if let url = URL(string: a.url) { safariURL = IdentifiableURL(url: url) }
                    }
                }
            }
        }

        SettingsSectionHeading(text: "Transcript")
        if showTranscript {
            if transcript.isEmpty {
                SettingsDescription(text: "No transcript was kept for this call.")
            }
            LazyVStack(alignment: .leading, spacing: 10) {
                ForEach(transcript) { s in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(s.speaker ?? "Someone").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        Text(s.text).font(.system(size: 15)).foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .textSelection(.enabled)
        } else {
            Button("Show the transcript") {
                showTranscript = true
                Task { transcript = (try? await MeetingsClient.shared.transcript(session.id, limit: 2000).segments) ?? [] }
            }
            .buttonStyle(SettingsQuietButtonStyle())
        }
    }

    private func bullet(symbol: String, text: String, detail: String?) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 20)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(text)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                if let detail, !detail.isEmpty {
                    Text(detail).font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .accessibilityElement(children: .combine)
    }

    private static func dueLabel(_ due: String) -> String? {
        if let date = MeetingDates.parse(due) { return "Due \(date.formatted(.dateTime.month(.abbreviated).day()))" }
        return due.isEmpty ? nil : "Due \(due)"
    }

    private func load() async {
        loading = true
        defer { loading = false }
        guard let sid = session?.id ?? meeting.sessionId else { return }
        do {
            let detail = try await MeetingsClient.shared.session(sid)
            session = detail.session
            MeetingsStore.shared.didUpdate(session: detail.session, meeting: detail.meeting)
            artifacts = (try? await MeetingsClient.shared.artifacts(sid)) ?? artifacts
            error = nil
        } catch MeetingsError.paidPlanRequired {
            paidPlan = true
        } catch {
            self.error = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
