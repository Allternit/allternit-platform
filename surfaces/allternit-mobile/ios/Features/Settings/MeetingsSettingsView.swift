import SwiftUI

/// Settings › Meetings: which calls Gizzi joins (default all except 1:1s),
/// spoken replies by default, the recap email, and the connected calendars.
/// `GET /me/meeting-settings` works on every plan; writes on a free plan
/// answer 402 and the page shows the plan card.
struct MeetingsSettingsView: View {
    @ObservedObject private var store = MeetingsStore.shared
    @State private var confirmRemove: CalendarConnection?

    var body: some View {
        PhoneSettingsPage(title: "Meetings") {
            if store.phase == .paidPlan {
                MeetingsPaidPlanCard().padding(.top, 12)
            }

            SettingsSectionHeading(text: "Which calls Gizzi joins")
            SettingsDescription(text: "From your connected calendars. You can still switch Gizzi on or off for any one call. Auto-join needs one of your computers online at the start time.")
            VStack(spacing: 8) {
                ForEach(AutoJoinRule.allCases) { rule in
                    SettingsRadioCard(title: rule.title, detail: rule.detail,
                                      isSelected: (store.settings?.autoJoin ?? AutoJoinRule.allExcept1on1.rawValue) == rule.rawValue) {
                        Task { await store.updateSettings(autoJoin: rule.rawValue) }
                    }
                    .accessibilityIdentifier("meetingsRule-\(rule.rawValue)")
                }
            }
            .disabled(store.settings == nil)

            SettingsSectionHeading(text: "In the call")
            SettingsCardBox {
                SettingsSwitchRow(
                    title: "Speak replies",
                    detail: "Gizzi answers out loud. Off: replies go to the meeting chat.",
                    isOn: Binding(
                        get: { store.settings?.defaultReplyMode == "voice" },
                        set: { on in Task { await store.updateSettings(defaultReplyMode: on ? "voice" : "chat") } }
                    )
                )
                SettingsSwitchRow(
                    title: "Email me the recap",
                    detail: "Summary, decisions and action items after each call.",
                    isOn: Binding(
                        get: { store.settings?.recapEmail ?? true },
                        set: { on in Task { await store.updateSettings(recapEmail: on) } }
                    )
                )
            }
            .disabled(store.settings == nil)

            SettingsSectionHeading(text: "Calendars")
            if !store.connections.isEmpty {
                PhoneSettingsCard {
                    ForEach(Array(store.connections.enumerated()), id: \.element.id) { index, connection in
                        if index > 0 { PhoneCardDivider() }
                        PhoneSettingsRowContent(
                            title: connection.providerName,
                            systemImage: connection.provider == "google" ? "calendar" : "link",
                            value: connection.detail
                        ) {
                            Button {
                                confirmRemove = connection
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .font(.system(size: 18))
                                    .foregroundColor(Color("TextTertiary"))
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Disconnect \(connection.providerName)")
                        }
                    }
                }
                .padding(.bottom, 12)
            }
            MeetingsCalendarConnectCard()

            SettingsSectionHeading(text: "Zoom")
            SettingsDescription(text: "Zoom lets Gizzi into calls through the account of someone in the call. Connect yours to send Gizzi to Zoom calls.")
            PhoneSettingsCard {
                PhoneSettingsRowContent(title: "Zoom", systemImage: "video.badge.checkmark",
                                        value: store.zoomConnected == true ? "Connected" : nil) {
                    if store.zoomConnected == true {
                        Button("Disconnect") { Task { await store.disconnectZoom() } }
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        Button("Connect Zoom") { Task { await store.connectZoom() } }
                            .buttonStyle(SettingsQuietButtonStyle())
                            .accessibilityIdentifier("meetingsConnectZoom")
                    }
                }
            }

            if let usage = store.usage {
                SettingsDescription(text: "\(usage.usedMinutes) of \(usage.includedMinutes) meeting minutes used this month.")
                    .padding(.top, 16)
            }
        }
        .confirmationDialog("Disconnect this calendar?", isPresented: Binding(
            get: { confirmRemove != nil }, set: { if !$0 { confirmRemove = nil } }
        ), titleVisibility: .visible, presenting: confirmRemove) { connection in
            Button("Disconnect", role: .destructive) { Task { await store.removeConnection(connection) } }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("Gizzi stops joining calls from it. Calls already on your list stay.")
        }
        .alert("Meetings", isPresented: Binding(
            get: { store.actionError != nil }, set: { if !$0 { store.actionError = nil } }
        )) {
            Button("OK", role: .cancel) { store.actionError = nil }
        } message: { Text(store.actionError ?? "") }
        .task { await store.load(quiet: store.phase == .loaded) }
    }
}
