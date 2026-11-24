import SwiftUI

/// The one calendar step, the same in Meetings, Settings › Meetings and
/// Connectors: "Connect Google Calendar" or "Paste a calendar link" (Outlook,
/// iCloud, any .ics). After the first calendar connects, the auto-join rule
/// is picked inline with "All calls except 1:1s" preselected.
struct MeetingsCalendarConnectCard: View {
    /// Connectors shows a shorter version.
    var compact = false

    @ObservedObject private var store = MeetingsStore.shared
    @State private var isLinkSheetPresented = false
    @State private var connecting = false

    var body: some View {
        if store.pickRule {
            MeetingsAutoJoinPicker()
        } else {
            VStack(alignment: .leading, spacing: 12) {
                HStack(alignment: .top, spacing: 12) {
                    Image(systemName: "calendar.badge.plus")
                        .font(.system(size: 20))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 40, height: 40)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("BgPanel")))
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(store.connections.isEmpty ? "Connect your calendar" : "Add another calendar")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(!store.connections.isEmpty
                             ? "Calls from every connected calendar follow the same rule."
                             : compact
                                ? "Gizzi joins your Meet, Zoom and Teams calls by itself."
                                : "Gizzi joins your Meet, Zoom and Teams calls by itself. You pick which calls next.")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                // Side by side when they fit, stacked on narrow phones.
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 8) { googleButton; linkButton }
                    VStack(alignment: .leading, spacing: 8) { googleButton; linkButton }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
            .sheet(isPresented: $isLinkSheetPresented) { MeetingsCalendarLinkSheet() }
        }
    }
}

extension MeetingsCalendarConnectCard {
    fileprivate var googleButton: some View {
        Button {
            connecting = true
            Task { await store.connectGoogle(); connecting = false }
        } label: {
            HStack(spacing: 6) {
                if connecting { ProgressView().tint(Color("OnAccent")) }
                Text("Connect Google Calendar").fixedSize()
            }
        }
        .buttonStyle(SettingsPrimaryButtonStyle())
        .disabled(connecting)
        .accessibilityIdentifier("meetingsConnectGoogle")
    }

    fileprivate var linkButton: some View {
        Button { isLinkSheetPresented = true } label: { Text("Paste a calendar link").fixedSize() }
            .buttonStyle(SettingsQuietButtonStyle())
            .accessibilityIdentifier("meetingsPasteCalendar")
    }
}

/// "Which calls should Gizzi join?" — the three rules, inline.
struct MeetingsAutoJoinPicker: View {
    @ObservedObject private var store = MeetingsStore.shared
    @State private var choice: AutoJoinRule = .allExcept1on1
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                Text("Calendar connected")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
            }
            Text("Which calls should Gizzi join?")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            ForEach(AutoJoinRule.allCases) { rule in
                SettingsRadioCard(title: rule.title, detail: rule.detail, isSelected: choice == rule) { choice = rule }
            }
            Button {
                saving = true
                Task {
                    await store.updateSettings(autoJoin: choice.rawValue)
                    store.pickRule = false
                    saving = false
                    await store.load(quiet: true)
                }
            } label: {
                HStack(spacing: 6) {
                    if saving { ProgressView().tint(Color("OnAccent")) }
                    Text("Done")
                }
            }
            .buttonStyle(SettingsPrimaryButtonStyle())
            .disabled(saving)
            .padding(.top, 2)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
    }
}

/// Paste a private calendar feed (ICS) link. The server fetches it once to
/// check it parses before saving.
struct MeetingsCalendarLinkSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = MeetingsStore.shared
    @State private var link = ""
    @State private var saving = false
    @State private var error: String?

    private var looksValid: Bool {
        let s = link.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return s.hasPrefix("https://") || s.hasPrefix("webcal://")
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    TextField("https://…/calendar.ics", text: $link)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .font(.system(size: 15))
                        .padding(.horizontal, 12)
                        .frame(minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                    if let error { SettingsErrorBanner(message: error) }
                    help(title: "Outlook", text: "Outlook on the web › Settings › Calendar › Shared calendars › Publish a calendar. Copy the ICS link.")
                    help(title: "iCloud", text: "Calendar app › tap ⓘ next to a calendar › Public Calendar. Share the link.")
                    help(title: "Others", text: "Any private calendar address that ends in .ics works. The link stays encrypted on our servers.")
                    Button {
                        save()
                    } label: {
                        HStack(spacing: 6) {
                            if saving { ProgressView().tint(Color("OnAccent")) }
                            Text("Add calendar")
                        }
                    }
                    .buttonStyle(SettingsPrimaryButtonStyle())
                    .disabled(!looksValid || saving)
                }
                .padding(16)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Paste a calendar link")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarLeading) { Button("Cancel") { dismiss() } } }
        }
        .presentationDetents([.large])
    }

    private func help(title: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(text).font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func save() {
        saving = true
        error = nil
        var value = link.trimmingCharacters(in: .whitespacesAndNewlines)
        if value.lowercased().hasPrefix("webcal://") { value = "https://" + value.dropFirst("webcal://".count) }
        Task {
            defer { saving = false }
            do {
                try await store.addCalendarLink(value)
                dismiss()
            } catch {
                self.error = (error as? MeetingsError)?.localizedDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}

/// Connectors' calendar prompt: shown with the `meetings` flag when no
/// calendar is connected yet.
struct MeetingsConnectorsPrompt: View {
    @ObservedObject private var store = MeetingsStore.shared

    var body: some View {
        Group {
            if store.isEnabled && ((store.connectionsLoaded && store.connections.isEmpty) || store.pickRule) {
                MeetingsCalendarConnectCard(compact: true)
                    .padding(.horizontal, 20)
                    .padding(.top, 12)
            }
        }
        .task {
            await store.refreshFlag()
            if store.isEnabled && !store.connectionsLoaded { await store.loadConnections() }
        }
    }
}
