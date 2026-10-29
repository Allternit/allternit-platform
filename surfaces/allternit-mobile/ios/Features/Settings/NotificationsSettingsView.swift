import SwiftUI
import UIKit
@preconcurrency import UserNotifications

// MARK: - Notifications

/// Settings → Account → Notifications, the web's NotificationsPanel on the
/// phone: what this iPhone notifies you about (Response completions, Approval
/// requests, Incoming calls), Delivery (iOS permission, Sound), then the
/// account-wide "Phone and browser notifications" (`/api/v1/notifications/
/// preferences`, one switch per event, from the cloud in its order), "Text me
/// critical alerts" (`/api/v1/alerts/sms`) and Quiet hours (`/api/v1/
/// notifications/quiet-hours`). Every switch gates a real source: the device
/// ones are read by NotificationService and InappCallStore.
struct NotificationsSettingsView: View {
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var status: UNAuthorizationStatus? = nil
    @State private var tested = false

    @AppStorage(NotificationPrefs.responseKey) private var response = true
    @AppStorage(NotificationPrefs.approvalKey) private var approvals = true
    @AppStorage(NotificationPrefs.soundKey) private var sound = true
    /// Per device: off = incoming Allternit calls vibrate only (InappCallStore).
    @AppStorage(InappCallStore.ringSoundKey) private var ringSound = true

    var body: some View {
        PhoneSettingsPage(title: "Notifications") {
            SettingsSectionHeading(text: "Notifications")
            SettingsDescription(text: "Choose what gets your attention while you work somewhere else. The switches down to Delivery apply to this iPhone only. Phone notifications and quiet hours, at the end, apply to your whole account.")

            SettingsSwitchRow(title: "Response completions",
                              detail: "When a reply finishes and Allternit isn’t the app you’re in.",
                              isOn: $response)

            SettingsSectionHeading(text: "Needs you")
                .padding(.top, 16)
            SettingsSwitchRow(title: "Approval requests",
                              detail: "An agent prepared a task or tool call, or a subscription asks you something.",
                              isOn: $approvals)
            SettingsSwitchRow(title: "Incoming calls",
                              detail: ringSound
                                ? "Ring while Allternit is open when someone calls you. The silent switch still silences the ring."
                                : "Calls vibrate only, with no ring sound.",
                              isOn: $ringSound)

            SettingsSectionHeading(text: "Delivery")
                .padding(.top, 16)
            SettingsLabeledRow(title: "iPhone notifications", detail: permissionDetail) {
                permissionControl
            }
            SettingsSwitchRow(title: "Sound",
                              detail: "Play the system sound with each notification.",
                              isOn: $sound)
                .disabled(status != .authorized && status != .provisional && status != .ephemeral)

            PushPreferencesSection()
            QuietHoursSection()
        }
        .task { await refresh() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { Task { await refresh() } }
        }
    }

    private var isOn: Bool { status == .authorized || status == .provisional || status == .ephemeral }

    private var permissionDetail: String {
        switch status {
        case .denied: return "Blocked in iOS Settings. Allow Allternit under Settings → Notifications."
        case .notDetermined: return "Allternit hasn’t been allowed to notify you yet."
        case nil: return "Checking…"
        default: return "Banners from Allternit on this iPhone, even when the app is closed."
        }
    }

    @ViewBuilder
    private var permissionControl: some View {
        switch status {
        case .notDetermined:
            Button("Allow") { Task { await request() } }
                .buttonStyle(SettingsQuietButtonStyle())
        case .denied:
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
            }
            .buttonStyle(SettingsQuietButtonStyle())
        case nil:
            ProgressView()
        default:
            Button(tested ? "Sent" : "Send a test") { Task { await sendTest() } }
                .buttonStyle(SettingsQuietButtonStyle())
                .disabled(tested)
        }
    }

    @MainActor
    private func refresh() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    @MainActor
    private func request() async {
        _ = await NotificationService.requestAuthorization()
        await refresh()
    }

    @MainActor
    private func sendTest() async {
        let content = UNMutableNotificationContent()
        content.title = "Notifications are on"
        content.body = "This is how Allternit will get your attention."
        if sound { content.sound = .default }
        // A second later, so it shows as a banner even with the app open.
        let trigger = UNTimeIntervalNotificationTrigger(timeInterval: 1, repeats: false)
        try? await UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: "settings-test", content: content, trigger: trigger)
        )
        tested = true
    }
}

/// This iPhone's notification switches (the web's NOTIFICATION_KEYS, local).
enum NotificationPrefs {
    static let responseKey = "notifications.response"
    static let approvalKey = "notifications.approval"
    static let soundKey = "notifications.sound"

    static func isOn(_ key: String) -> Bool {
        UserDefaults.standard.object(forKey: key) as? Bool ?? true
    }
}

// MARK: - Cloud notification settings

/// `GET/PUT` against cloud-api for the account-wide notification settings.
private enum NotificationsCloud {
    struct Preference: Codable, Equatable, Identifiable {
        let event: String
        let title: String
        let description: String
        var enabled: Bool
        let `default`: Bool
        var id: String { event }
    }

    struct Preferences: Decodable {
        var pushConfigured: Bool?
        var preferences: [Preference]?
    }

    struct QuietHours: Codable, Equatable {
        var enabled: Bool
        var start: String
        var end: String
        var timeZone: String
        var allowCalls: Bool
        var letThrough: String

        static var defaults: QuietHours {
            QuietHours(enabled: false, start: "22:00", end: "07:00",
                       timeZone: TimeZone.current.identifier, allowCalls: true, letThrough: "urgent")
        }
    }

    struct CriticalSms: Decodable {
        var enabled: Bool
        var phone: String
        var resolvedPhone: String?
    }

    struct SmsVerifyStatus: Decodable {
        var smsVerified: Bool
        var phone: String?
    }

    /// 404 = an older cloud without the route: the section hides.
    struct Unavailable: Error {}

    static func get<T: Decodable>(_ path: String) async throws -> T {
        try await send(path, method: "GET", body: Optional<[String: String]>.none)
    }

    static func send<T: Decodable, B: Encodable>(_ path: String, method: String, body: B?) async throws -> T {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent(path)
        var request = try await APIClient.shared.authorizedRequest(url: url, method: method)
        if let body { request.httpBody = try JSONEncoder().encode(body) }
        let (data, response) = try await URLSession.shared.data(for: request)
        if (response as? HTTPURLResponse)?.statusCode == 404 { throw Unavailable() }
        try APIClient.shared.validate(response, data: data)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// The web's prefsErrorText.
    static func message(_ error: Error, fallback: String) -> String {
        switch error {
        case APIError.httpError(let status, _) where status == 401 || status == 403:
            return "Your Allternit session expired. Sign in again to continue."
        case APIError.transport, is URLError:
            return "Couldn't reach Allternit. Check your connection and try again."
        default:
            return fallback
        }
    }
}

/// A JSON body of `{ key: Bool? }` where nil encodes as `null` (Reset to default).
private struct NullableBoolMap: Encodable {
    let values: [String: Bool?]
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: Key.self)
        for (key, value) in values {
            if let value { try container.encode(value, forKey: Key(key)) } else { try container.encodeNil(forKey: Key(key)) }
        }
    }
    struct Key: CodingKey {
        let stringValue: String
        init(_ s: String) { stringValue = s }
        init?(stringValue: String) { self.stringValue = stringValue }
        var intValue: Int? { nil }
        init?(intValue: Int) { nil }
    }
}

private struct PreferencesBody: Encodable { let preferences: NullableBoolMap }

// MARK: - Phone notifications (account-wide)

/// The web's PushPreferencesSection: one switch per event, Default / Reset to
/// default, saved at once (optimistic, rolled back on error).
private struct PushPreferencesSection: View {
    private enum Load { case loading, error(String), unavailable, ready(pushConfigured: Bool) }

    @State private var load: Load = .loading
    @State private var rows: [NotificationsCloud.Preference] = []
    @State private var saving: Set<String> = []
    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionHeading(text: "Phone notifications")
                .padding(.top, 16)
            SettingsDescription(text: "For your whole account. These reach every phone and browser you turned notifications on for, even when Allternit is closed.")

            switch load {
            case .loading:
                SettingsDescription(text: "Loading your notification choices…")
            case .unavailable:
                SettingsDescription(text: "Choosing what notifies you isn’t available yet.")
            case .error(let message):
                HStack(spacing: 8) {
                    Text(message).font(.system(size: 13)).foregroundColor(Theme.statusError)
                    Button("Try again") { Task { await fetch() } }.buttonStyle(SettingsQuietButtonStyle())
                }
            case .ready(let pushConfigured):
                if !pushConfigured {
                    SettingsDescription(text: "Allternit can’t send phone or browser notifications right now. Your choices are saved and apply once it can.")
                }
                if rows.isEmpty {
                    SettingsDescription(text: "There’s nothing to choose yet.")
                }
                ForEach(rows) { pref in
                    SettingsLabeledRow(title: pref.title, detail: pref.description) {
                        HStack(spacing: 8) {
                            if pref.enabled != pref.default {
                                Button("Reset to default") { Task { await change(pref.event, nil) } }
                                    .buttonStyle(SettingsQuietButtonStyle())
                                    .accessibilityLabel("Reset \(pref.title) to default")
                            } else {
                                Text("Default").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                            }
                            Toggle(pref.title, isOn: Binding(
                                get: { pref.enabled },
                                set: { value in Task { await change(pref.event, value) } }
                            ))
                            .labelsHidden()
                            .tint(Color("BrandAccent"))
                        }
                        .disabled(saving.contains(pref.event))
                    }
                }
            }

            CriticalSmsRow()

            if let error {
                Text(error).font(.system(size: 13)).foregroundColor(Theme.statusError).padding(.top, 8)
            }
        }
        .task { await fetch() }
    }

    @MainActor
    private func fetch() async {
        load = .loading
        do {
            let prefs: NotificationsCloud.Preferences = try await NotificationsCloud.get("api/v1/notifications/preferences")
            rows = prefs.preferences ?? []
            load = .ready(pushConfigured: prefs.pushConfigured == true)
        } catch is NotificationsCloud.Unavailable {
            load = .unavailable
        } catch {
            load = .error(NotificationsCloud.message(error, fallback: "Couldn't load your notification choices."))
        }
    }

    @MainActor
    private func change(_ event: String, _ value: Bool?) async {
        guard let before = rows.first(where: { $0.event == event }) else { return }
        error = nil
        saving.insert(event)
        rows = rows.map { $0.event == event ? with($0, enabled: value ?? $0.default) : $0 }
        do {
            let body = PreferencesBody(preferences: NullableBoolMap(values: [event: value]))
            let res: NotificationsCloud.Preferences = try await NotificationsCloud.send(
                "api/v1/notifications/preferences", method: "PUT", body: body)
            if let fresh = res.preferences?.first(where: { $0.event == event }) {
                rows = rows.map { $0.event == event ? fresh : $0 }
            }
            if case .ready = load { load = .ready(pushConfigured: res.pushConfigured == true) }
        } catch {
            rows = rows.map { $0.event == event ? before : $0 }
            self.error = "\(before.title): \(NotificationsCloud.message(error, fallback: "Couldn't save that change. Try again."))"
        }
        saving.remove(event)
    }

    private func with(_ pref: NotificationsCloud.Preference, enabled: Bool) -> NotificationsCloud.Preference {
        var copy = pref
        copy.enabled = enabled
        return copy
    }
}

// MARK: - Text me critical alerts

/// The web's CriticalSmsRow + SmsVerifyRow: off by default; one text from the
/// bot's own number, only to a number confirmed with a 6-digit code.
private struct CriticalSmsRow: View {
    @State private var sms: NotificationsCloud.CriticalSms? = nil
    @State private var phone = ""
    @State private var busy = false
    @State private var error: String? = nil
    @State private var verified: Bool? = nil
    @State private var codeSent = false
    @State private var code = ""

    private struct SaveBody: Encodable { let enabled: Bool; let phone: String }
    private struct CodeBody: Encodable { let code: String }
    private struct Empty: Encodable {}
    private struct SentReply: Decodable { let sent: Bool? }
    private struct VerifiedReply: Decodable { let smsVerified: Bool }

    var body: some View {
        Group {
            if let sms {
                let number = sms.phone.isEmpty ? sms.resolvedPhone : sms.phone
                VStack(alignment: .leading, spacing: 8) {
                    SettingsLabeledRow(
                        title: "Text me critical alerts",
                        detail: number.map { "One text to \($0) from your bot's number when something critical needs you, at most one per problem every 30 minutes. You confirm the number with a code first." }
                            ?? "Add your phone number below. You get one text from your bot’s number when something critical needs you. You confirm the number with a code before any alert is texted."
                    ) {
                        Toggle("Text me critical alerts", isOn: Binding(
                            get: { sms.enabled },
                            set: { value in Task { await save(enabled: value) } }
                        ))
                        .labelsHidden()
                        .tint(Color("BrandAccent"))
                        .disabled(busy || (!sms.enabled && number == nil))
                    }
                    HStack(spacing: 8) {
                        TextField("+14155550123", text: $phone)
                            .keyboardType(.phonePad)
                            .textContentType(.telephoneNumber)
                            .font(.system(size: 14))
                            .padding(.horizontal, 10)
                            .frame(height: 36)
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                            .accessibilityLabel("Phone number for critical alerts")
                        Button("Save number") { Task { await save(enabled: sms.enabled) } }
                            .buttonStyle(SettingsQuietButtonStyle())
                            .disabled(busy || phone.trimmingCharacters(in: .whitespaces) == sms.phone)
                    }
                    if sms.enabled, !sms.phone.isEmpty, sms.phone == phone.trimmingCharacters(in: .whitespaces) {
                        verifyRow(number: sms.phone)
                    }
                    if let error {
                        Text(error).font(.system(size: 13)).foregroundColor(Theme.statusError)
                    }
                }
                .padding(.top, 8)
            }
        }
        .task { await fetch() }
    }

    @ViewBuilder
    private func verifyRow(number: String) -> some View {
        if verified == true {
            Text("Verified: \(number) gets critical alerts.")
                .font(.system(size: 13))
                .foregroundColor(Theme.statusSuccess)
        } else if verified == false {
            VStack(alignment: .leading, spacing: 6) {
                Text("Not verified yet: no alert texts go to \(number) until you confirm it.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                if codeSent {
                    HStack(spacing: 8) {
                        TextField("6-digit code", text: $code)
                            .keyboardType(.numberPad)
                            .textContentType(.oneTimeCode)
                            .font(.system(size: 14))
                            .padding(.horizontal, 10)
                            .frame(width: 120, height: 36)
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
                            .onChange(of: code) { _, value in
                                let digits = String(value.filter(\.isNumber).prefix(6))
                                if digits != value { code = digits }
                            }
                            .accessibilityLabel("Verification code")
                        Button("Confirm") { Task { await confirm() } }
                            .buttonStyle(SettingsQuietButtonStyle())
                            .disabled(busy || code.count != 6)
                        Button("Resend") { Task { await startVerify() } }
                            .buttonStyle(SettingsQuietButtonStyle())
                            .disabled(busy)
                    }
                } else {
                    Button("Send code") { Task { await startVerify() } }
                        .buttonStyle(SettingsQuietButtonStyle())
                        .disabled(busy)
                }
            }
        }
    }

    @MainActor
    private func fetch() async {
        // An older runtime without the route: nothing to choose, say nothing.
        guard let loaded: NotificationsCloud.CriticalSms = try? await NotificationsCloud.get("api/v1/alerts/sms") else { return }
        sms = loaded
        phone = loaded.phone
        await fetchVerified()
    }

    @MainActor
    private func fetchVerified() async {
        guard let sms, sms.enabled, !sms.phone.isEmpty else { verified = nil; return }
        if let status: NotificationsCloud.SmsVerifyStatus = try? await NotificationsCloud.get("api/v1/alerts/sms/verify/status") {
            verified = status.smsVerified && status.phone == sms.phone
        } else {
            verified = nil
        }
    }

    @MainActor
    private func save(enabled: Bool) async {
        busy = true
        error = nil
        do {
            let next: NotificationsCloud.CriticalSms = try await NotificationsCloud.send(
                "api/v1/alerts/sms", method: "PUT",
                body: SaveBody(enabled: enabled, phone: phone.trimmingCharacters(in: .whitespaces)))
            sms = next
            phone = next.phone
            codeSent = false
            code = ""
            await fetchVerified()
        } catch {
            self.error = NotificationsCloud.message(error, fallback: "Couldn't save that change. Try again.")
        }
        busy = false
    }

    @MainActor
    private func startVerify() async {
        busy = true
        error = nil
        do {
            let _: SentReply = try await NotificationsCloud.send("api/v1/alerts/sms/verify/start", method: "POST", body: Empty())
            codeSent = true
        } catch {
            self.error = NotificationsCloud.message(error, fallback: "Couldn't send the code. Try again.")
        }
        busy = false
    }

    @MainActor
    private func confirm() async {
        busy = true
        error = nil
        do {
            let reply: VerifiedReply = try await NotificationsCloud.send(
                "api/v1/alerts/sms/verify/confirm", method: "POST", body: CodeBody(code: code))
            verified = reply.smsVerified
        } catch {
            self.error = NotificationsCloud.message(error, fallback: "That code didn't work. Try again.")
        }
        busy = false
    }
}

// MARK: - Quiet hours (account-wide)

/// The web's QuietHoursSection: switch, From/to (15-minute steps), time zone,
/// Let through, Let calls ring. Each change saves at once; a cloud without the
/// route (404) hides the block.
private struct QuietHoursSection: View {
    private enum Load { case loading, error(String), unavailable, ready }

    @State private var load: Load = .loading
    @State private var qh: NotificationsCloud.QuietHours? = nil
    @State private var saving = false
    @State private var error: String? = nil

    private static let letThrough: [(id: String, label: String, help: String)] = [
        ("urgent", "Urgent only", "Urgent alerts reach you and repeat every 5 minutes until you open them (3 times). The rest waits."),
        ("needs_you", "Urgent and needs you", "Also approvals and bots waiting on you. Urgent alerts repeat until you open them."),
        ("none", "Nothing", "Everything waits for the summary."),
    ]

    var body: some View {
        if case .unavailable = load {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: 0) {
                SettingsSectionHeading(text: "Quiet hours")
                    .padding(.top, 16)
                SettingsDescription(text: "Notifications wait until quiet hours end, then you get one summary. Urgent ones still reach you. For your whole account.")

                switch load {
                case .loading:
                    SettingsDescription(text: "Loading your quiet hours…")
                case .error(let message):
                    HStack(spacing: 8) {
                        Text(message).font(.system(size: 13)).foregroundColor(Theme.statusError)
                        Button("Try again") { Task { await fetch() } }.buttonStyle(SettingsQuietButtonStyle())
                    }
                case .ready:
                    if let qh { rows(qh) }
                case .unavailable:
                    EmptyView()
                }

                if let error {
                    Text(error).font(.system(size: 13)).foregroundColor(Theme.statusError).padding(.top, 8)
                }
            }
            .task { await fetch() }
        }
    }

    @ViewBuilder
    private func rows(_ qh: NotificationsCloud.QuietHours) -> some View {
        let off = !qh.enabled
        SettingsSwitchRow(title: "Quiet hours", detail: "It won’t bug you at night.",
                          isOn: Binding(get: { qh.enabled }, set: { v in Task { await change { $0.enabled = v } } }))
            .disabled(saving)
        SettingsLabeledRow(title: "From", detail: qh.end <= qh.start ? "Runs past midnight." : nil) {
            HStack(spacing: 6) {
                timePicker(qh.start, label: "Quiet hours start") { t in Task { await change { $0.start = t } } }
                Text("to").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                timePicker(qh.end, label: "Quiet hours end") { t in Task { await change { $0.end = t } } }
            }
            .disabled(off || saving)
            .opacity(off ? 0.5 : 1)
        }
        SettingsLabeledRow(title: "Time zone",
                           detail: qh.timeZone == TimeZone.current.identifier
                            ? "This device’s time zone."
                            : "This device is on \(TimeZone.current.identifier).") {
            Menu {
                Picker("Time zone", selection: Binding(get: { qh.timeZone }, set: { z in Task { await change { $0.timeZone = z } } })) {
                    ForEach(zones(including: qh.timeZone), id: \.self) { Text($0.replacingOccurrences(of: "_", with: " ")).tag($0) }
                }
            } label: {
                selectLabel(qh.timeZone.replacingOccurrences(of: "_", with: " "))
            }
            .disabled(off || saving)
            .opacity(off ? 0.5 : 1)
            .accessibilityLabel("Quiet hours time zone")
        }
        SettingsLabeledRow(title: "Let through",
                           detail: Self.letThrough.first { $0.id == qh.letThrough }?.help) {
            Menu {
                Picker("Let through", selection: Binding(get: { qh.letThrough }, set: { l in Task { await change { $0.letThrough = l } } })) {
                    ForEach(Self.letThrough, id: \.id) { Text($0.label).tag($0.id) }
                }
            } label: {
                selectLabel(Self.letThrough.first { $0.id == qh.letThrough }?.label ?? "Urgent only")
            }
            .disabled(off || saving)
            .opacity(off ? 0.5 : 1)
            .accessibilityLabel("Let through during quiet hours")
        }
        SettingsSwitchRow(title: "Let calls ring", detail: "Incoming calls still ring during quiet hours.",
                          isOn: Binding(get: { qh.allowCalls }, set: { v in Task { await change { $0.allowCalls = v } } }))
            .disabled(off || saving)
    }

    private func selectLabel(_ text: String) -> some View {
        HStack(spacing: 6) {
            Text(text).foregroundColor(Color("TextPrimary")).lineLimit(1)
            Image(systemName: "chevron.down").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
        }
        .font(.system(size: 14))
        .padding(.horizontal, 10)
        .frame(minHeight: 36)
        .frame(maxWidth: 170)
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
    }

    /// An HH:mm field in 15-minute steps (the web's `<input type="time" step=900>`).
    private func timePicker(_ value: String, label: String, onChange: @escaping (String) -> Void) -> some View {
        Menu {
            Picker(label, selection: Binding(get: { value }, set: onChange)) {
                ForEach(Self.quarterHours(including: value), id: \.self) { Text($0).tag($0) }
            }
        } label: {
            Text(value)
                .font(.system(size: 14).monospacedDigit())
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 10)
                .frame(minHeight: 36)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
        }
        .accessibilityLabel(label)
    }

    private static func quarterHours(including value: String) -> [String] {
        var all = (0..<96).map { String(format: "%02d:%02d", $0 / 4, ($0 % 4) * 15) }
        if !all.contains(value) { all.append(value); all.sort() }
        return all
    }

    private func zones(including current: String) -> [String] {
        var set = Set(TimeZone.knownTimeZoneIdentifiers)
        set.insert(TimeZone.current.identifier)
        if !current.isEmpty { set.insert(current) }
        return set.sorted()
    }

    @MainActor
    private func fetch() async {
        load = .loading
        do {
            let loaded: NotificationsCloud.QuietHours = try await NotificationsCloud.get("api/v1/notifications/quiet-hours")
            qh = loaded
            load = .ready
        } catch is NotificationsCloud.Unavailable {
            load = .unavailable
        } catch is DecodingError {
            // A partial body: the web normalizes missing fields to the defaults.
            qh = .defaults
            load = .ready
        } catch {
            load = .error(NotificationsCloud.message(error, fallback: "Couldn't load your quiet hours."))
        }
    }

    /// Optimistic: apply, PUT the whole value, roll back on error.
    @MainActor
    private func change(_ edit: (inout NotificationsCloud.QuietHours) -> Void) async {
        guard let before = qh else { return }
        var next = before
        edit(&next)
        guard next != before else { return }
        error = nil
        saving = true
        qh = next
        do {
            let saved: NotificationsCloud.QuietHours = try await NotificationsCloud.send(
                "api/v1/notifications/quiet-hours", method: "PUT", body: next)
            qh = saved
        } catch {
            qh = before
            self.error = "Quiet hours: \(NotificationsCloud.message(error, fallback: "Couldn't save that change. Try again."))"
        }
        saving = false
    }
}
