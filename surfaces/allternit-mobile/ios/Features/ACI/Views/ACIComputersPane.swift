import SwiftUI
import UIKit

/// ACI › Computers, as the web phone draws it (AciScreen.tsx `ComputersPane`):
/// "Computers" header, Add a computer (pairing steps sheet), View a
/// computer's screen, the paired computers (MachinesPanel `list`), linked
/// phones, cloud computer backups, workflows and operator keys.
struct ACIComputersPane: View {
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var files = FilesDrive.shared
    @State private var isPairOpen = false
    @State private var isScreenPickerOpen = false
    @State private var openComputer: ComputerRecord? = nil
    /// A computer with the Files drive on opens its detail (design 4A).
    @State private var detailComputer: RelayRuntimeDevice? = nil
    @State private var toast: ACIToast.Message? = nil
    /// Account flag `computers_console`: off = today's screen, unchanged.
    @State private var consoleOn = false

    private var loading: Bool { relay.availability == .unknown }

    /// Workflows run on the first online computer, else the first one.
    private var workflowRuntime: RelayRuntimeDevice? {
        relay.devices.first(where: \.isOnline) ?? relay.devices.first
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ACIViewHeader(title: "Computers")
                ACIWideButton(systemName: "plus", title: "Add a computer") { isPairOpen = true }
                ACIWideButton(systemName: "display", title: "View a computer's screen") { isScreenPickerOpen = true }
                if consoleOn { ACIConsoleSection() }
                machines
                ACILinkedPhonesSection()
                    .padding(.top, 24)
                ACICloudBackupsSection()
                ACIWorkflowsSection(runtime: workflowRuntime)
                    .padding(.top, 24)
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 32)
        }
        .refreshable { await relay.refresh() }
        .task { await relay.refresh() }
        .task { consoleOn = (try? await AccountFlagsClient.shared.string("computers_console")) != nil }
        .overlay(alignment: .bottom) { ACIToast(message: $toast) }
        .sheet(isPresented: $isPairOpen) { ACIPairComputerSheet() }
        .sheet(isPresented: $isScreenPickerOpen) { ACIComputerScreenSheet() }
        .fullScreenCover(item: $openComputer) { computer in
            ACIComputerDesktopCover(computer: computer, backTitle: "Computers")
        }
        .fullScreenCover(item: $detailComputer) { device in
            ACIComputerDetailCover(computerId: device.id, name: device.displayName)
        }
    }

    // MARK: Computers list (MachinesPanel `list`)

    @ViewBuilder
    private var machines: some View {
        if loading {
            Text("Loading computers…")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity)
                .padding(.vertical, 48)
        } else if relay.devices.isEmpty {
            ACIDashedCard {
                VStack(spacing: 0) {
                    Image(systemName: "desktopcomputer")
                        .font(.system(size: 40, weight: .light))
                        .foregroundColor(Color("TextPrimary").opacity(0.4))
                        .padding(.bottom, 12)
                    Text("No computers paired")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.bottom, 4)
                    Text("Open Allternit Desktop on a computer signed in to this account, and it shows up here.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                        .multilineTextAlignment(.center)
                }
                .padding(32)
            }
        } else {
            VStack(spacing: 0) {
                ForEach(relay.devices) { device in
                    computerRow(device)
                }
            }
            .accessibilityLabel("Your computers")
        }
    }

    private func computerRow(_ device: RelayRuntimeDevice) -> some View {
        let online = device.status != "offline"
        return Button {
            Haptics.light()
            if files.hasFiles(device.id) {
                detailComputer = device
            } else {
                Task { await openDesktop(device) }
            }
        } label: {
            VStack(alignment: .leading, spacing: 6) {
                Text(device.displayName)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.leading)
                HStack(spacing: 7) {
                    Circle()
                        .fill(online ? Theme.statusSuccess : Color("TextTertiary"))
                        .frame(width: 8, height: 8)
                    Text(online ? "Online" : "Offline")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                }
                Text(Self.lastSeen(device.lastSeenAt))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(.vertical, 16)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
        .accessibilityLabel(files.hasFiles(device.id) ? "Open \(device.displayName)" : "Open desktop for \(device.displayName)")
    }

    /// "Last seen Oct 29, 2026, 3:04 PM", or the web's fallback line.
    static func lastSeen(_ raw: String?) -> String {
        guard let raw, let date = ACIDates.parse(raw) else { return "Last seen time unavailable" }
        let text = date.formatted(.dateTime.year().month(.abbreviated).day().hour().minute())
        return "Last seen \(text)"
    }

    /// The web opens the computer's live desktop (PhoneDesktopOverlay). The
    /// app's desktop viewer is keyed by the computers registry, so find the
    /// paired computer's record there; without one, offer the screen picker.
    private func openDesktop(_ device: RelayRuntimeDevice) async {
        let computers = (try? await BotModeClient.shared.listComputers()) ?? []
        if let match = computers.first(where: { $0.id == device.id })
            ?? computers.first(where: { $0.name == device.displayName || ($0.host != nil && $0.host == device.hostname) }) {
            openComputer = match
        } else {
            isScreenPickerOpen = true
        }
    }
}

// MARK: - Dates

enum ACIDates {
    static func parse(_ raw: String) -> Date? {
        if let date = try? Date(raw, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) { return date }
        return try? Date(raw, strategy: Date.ISO8601FormatStyle())
    }
}

// MARK: - Toast

/// The web's toast (use-toast): a floating card with a status icon, title
/// and line, dismissed by its × or after a few seconds.
struct ACIToast: View {
    struct Message: Equatable {
        enum Kind { case error, success, info }
        let kind: Kind
        let title: String
        var detail: String? = nil
    }

    @Binding var message: Message?

    var body: some View {
        if let message {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: icon(message.kind))
                    .font(.system(size: 18))
                    .foregroundColor(color(message.kind))
                VStack(alignment: .leading, spacing: 4) {
                    Text(message.title)
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if let detail = message.detail, !detail.isEmpty {
                        Text(detail)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(3)
                    }
                }
                Spacer(minLength: 0)
                Button { self.message = nil } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 28, height: 28)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss")
            }
            .padding(16)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
            .shadow(color: Color.black.opacity(0.08), radius: 12, x: 0, y: 4)
            .padding(.horizontal, 40)
            .padding(.bottom, 16)
            .transition(.move(edge: .bottom).combined(with: .opacity))
            .task(id: message) {
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                if !Task.isCancelled { withAnimation { self.message = nil } }
            }
        }
    }

    private func icon(_ kind: Message.Kind) -> String {
        switch kind {
        case .error: return "xmark.circle"
        case .success: return "checkmark.circle"
        case .info: return "info.circle"
        }
    }

    private func color(_ kind: Message.Kind) -> Color {
        switch kind {
        case .error: return Theme.statusError
        case .success: return Theme.statusSuccess
        case .info: return Color("TextSecondary")
        }
    }
}

// MARK: - Pairing sheet

/// "Add a computer" (the pair promo sheet): hero, close over it, title,
/// one line and the three numbered steps.
struct ACIPairComputerSheet: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                ACIPromoHero(kind: .pair, height: 176, imageHeight: 92, topRadius: 0)
                    .overlay(alignment: .topLeading) {
                        Button { dismiss() } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 18))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 44, height: 44)
                                .background(Color("BgPrimary"))
                                .clipShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .padding(16)
                        .accessibilityLabel("Close pairing instructions")
                    }
                VStack(alignment: .leading, spacing: 14) {
                    Text("Add a computer")
                        .font(.system(size: 26, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Text("Connect a computer to open its desktop from your phone.")
                        .font(.system(size: 17))
                        .lineSpacing(4)
                        .foregroundColor(Color("TextSecondary"))
                    VStack(alignment: .leading, spacing: 12) {
                        step(1, Text("Install Allternit Desktop on the computer."))
                        step(2, Text("Open it and sign in with the same account you use on this phone. Or open a terminal on the computer and run ")
                             + Text("gizzi pair").font(.system(size: 14, design: .monospaced))
                             + Text("."))
                        step(3, Text("Return to Computers on your phone. Tap your computer to open its live desktop."))
                    }
                    .padding(.leading, 4)
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
            }
        }
        .background(Color("BgPanel").ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(28)
    }

    private func step(_ number: Int, _ text: Text) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("\(number).")
                .font(.system(size: 16))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 20, alignment: .trailing)
            text
                .font(.system(size: 16))
                .lineSpacing(5)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Computer screen

/// "View a computer's screen" (PhoneComputerViewOverlay): pick any computer
/// — cloud, a bot's, or a paired Mac — and watch or drive its live screen.
struct ACIComputerScreenSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var computers: [ComputerRecord] = []
    @State private var isLoading = true
    @State private var error: String? = nil

    var body: some View {
        NavigationStack {
            List {
                if isLoading && computers.isEmpty {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let error {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load computers",
                                      message: error, actionTitle: "Retry", action: { Task { await load() } })
                } else if computers.isEmpty {
                    FriendlyStateView(style: .empty, icon: "desktopcomputer", title: "No computers yet",
                                      message: "Open Allternit Desktop on a computer signed in to this account, and it shows up here.")
                } else {
                    ForEach(computers) { computer in
                        NavigationLink(value: computer) {
                            ComputerRowLabel(computer: computer)
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Computer")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: ComputerRecord.self) { computer in
                ComputerDriveView(computer: computer)
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { dismiss() } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                            Text("Back")
                        }
                    }
                }
            }
            .refreshable { await load() }
            .task { await load() }
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            computers = try await BotModeClient.shared.listComputers()
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

/// A paired computer's live desktop, full screen, with a "‹ Computers" back.
struct ACIComputerDesktopCover: View {
    let computer: ComputerRecord
    let backTitle: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ComputerDriveView(computer: computer)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button { dismiss() } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                                Text(backTitle)
                            }
                        }
                    }
                }
        }
    }
}

// MARK: - Linked phones

/// LinkedPhonesSection.tsx: phones paired to this account, from the cloud
/// relay. Signed out, or none linked, shows the setup line instead.
struct ACILinkedPhonesSection: View {
    @ObservedObject private var auth = AuthManager.shared
    @State private var phones: [ACIComputersClient.CloudPhone]? = nil
    @State private var error: String? = nil
    @State private var attempt = 0
    @State private var callAudioFor: ACIComputersClient.CloudPhone? = nil
    @State private var liveFor: ACIComputersClient.CloudPhone? = nil

    /// PHONE_STREAM_UNSUPPORTED_MESSAGE.
    private static let setupCopy = "Live phone screens need Allternit Desktop with a phone connected there. Set the phone up from Allternit Desktop, then open it here again."

    var body: some View {
        Group {
            if !auth.isSignedIn || (phones != nil && phones?.isEmpty == true) {
                ACIDashedCard(radius: 12) {
                    HStack(alignment: .top, spacing: 12) {
                        Image(systemName: "iphone")
                            .font(.system(size: 26))
                            .foregroundColor(Color("TextSecondary"))
                            .accessibilityHidden(true)
                        Text(Self.setupCopy)
                            .font(.system(size: 14))
                            .lineSpacing(3)
                            .foregroundColor(Color("TextSecondary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(16)
                }
            } else {
                VStack(alignment: .leading, spacing: 0) {
                    Text("Phones")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.bottom, 4)
                        .accessibilityAddTraits(.isHeader)
                    if let error {
                        HStack(spacing: 8) {
                            Text(error)
                                .font(.system(size: 14))
                                .foregroundColor(Theme.statusError)
                            Spacer()
                            ACIPillButton(title: "Retry") { attempt += 1 }
                        }
                        .padding(.vertical, 12)
                    } else if let phones {
                        ForEach(phones) { phone in phoneRow(phone) }
                    } else {
                        Text("Loading phones…")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.vertical, 12)
                    }
                }
            }
        }
        .task(id: "\(auth.isSignedIn)-\(attempt)") { await load() }
        .sheet(item: $callAudioFor) { phone in ACICallAudioSheet(phone: phone) }
        .fullScreenCover(item: $liveFor) { phone in PhoneLiveView(phone: phone) }
    }

    private func phoneRow(_ phone: ACIComputersClient.CloudPhone) -> some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 6) {
                Text(phone.name)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                HStack(spacing: 7) {
                    Circle()
                        .fill(phone.isLive ? Theme.statusSuccess : Color("TextTertiary"))
                        .frame(width: 8, height: 8)
                    Text(phone.isAndroidApp
                         ? (phone.online ? (phone.canTap == true ? "Screen and taps" : "Screen only") : "Phone app offline")
                         : phone.isKeyboard ? phone.keyboardLabel
                         : phone.isAppPhone ? (phone.tapRouteReady ? "Screen and taps" : "Screen") : phone.online ? "Online" : "Desktop offline")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                }
                if phone.isAndroidApp && !phone.online {
                    Text("Open the Allternit app on the phone")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                } else if phone.isKeyboard {
                    Text("A Bluetooth keyboard and mouse, not another phone")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                } else if phone.isAppPhone && !phone.isLive {
                    Text("Open the Allternit app on your iPhone and tap Share screen")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                } else if !phone.model.isEmpty {
                    Text(phone.model)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(.vertical, 16)
            // Call audio is the Android cable bridge; a linked iPhone has only its screen.
            if phone.isAndroidApp {
                ACIPillButton(title: "Open", accessibilityLabel: "Open the screen of \(phone.name)") {
                    liveFor = phone
                }
                .padding(.trailing, 8)
            } else if !phone.isIphone {
                ACIPillButton(title: "Call audio", accessibilityLabel: "Call audio for \(phone.name)") {
                    callAudioFor = phone
                }
                .padding(.trailing, 8)
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private func load() async {
        guard auth.isSignedIn else { return }
        phones = nil
        error = nil
        do {
            phones = try await ACIComputersClient.shared.listPhones()
        } catch is CancellationError {
        } catch {
            let message = error.localizedDescription
            self.error = message.isEmpty ? "Couldn’t load your phones." : message
        }
    }
}

/// CallAudioSheet's read-only view: whether a bot answers calls on this
/// phone, as Desktop last reported it. Changing it needs the wired Mac.
private struct ACICallAudioSheet: View {
    let phone: ACIComputersClient.CloudPhone
    @Environment(\.dismiss) private var dismiss

    private var answers: String {
        guard let bridge = phone.callBridge else { return "Shown in Allternit Desktop" }
        guard bridge.enabled else { return "Off" }
        if let bot = bridge.botName, !bot.isEmpty { return "On · \(bot)" }
        return "On"
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Bot answers calls")
                        .foregroundColor(Color("TextTertiary"))
                    Spacer()
                    Text(answers)
                        .foregroundColor(Color("TextPrimary"))
                }
                .font(.system(size: 14))
                Text("Changing it needs Allternit Desktop on the Mac the phone is wired to, because the audio devices are on that Mac.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
            }
            .padding(20)
            .background(Color("BgPanel").ignoresSafeArea())
            .navigationTitle("Call audio · \(phone.name)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel("Close")
                }
            }
        }
        .presentationDetents([.medium])
    }
}

// MARK: - Cloud computer backups

/// CloudComputerBackups: hidden until the account has a cloud computer.
/// Each computer opens its restore points and "Download a copy".
struct ACICloudBackupsSection: View {
    @State private var computers: [CloudComputerSummary] = []
    @State private var error: String? = nil
    @State private var isOpen = false

    var body: some View {
        Group {
            if error != nil || !computers.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
                        .padding(.bottom, 4)
                    Text("Restore points and backups")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Your cloud computer is also backed up off-site every night, encrypted.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextTertiary"))
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .font(.system(size: 12))
                            .foregroundColor(Theme.statusError)
                    } else {
                        ForEach(computers) { computer in
                            Button { isOpen = true } label: {
                                HStack {
                                    Text(computer.label)
                                        .font(.system(size: 14, weight: .medium))
                                        .foregroundColor(Color("TextPrimary"))
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundColor(Color("TextTertiary"))
                                }
                                .padding(.horizontal, 14)
                                .frame(minHeight: 44)
                                .background(Color("FillNeutral"))
                                .clipShape(RoundedRectangle(cornerRadius: 12))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .padding(.top, 24)
                .sheet(isPresented: $isOpen) {
                    NavigationStack { CloudComputerBackupsView() }
                }
            }
        }
        .task { await load() }
    }

    private func load() async {
        struct Envelope: Decodable { let instances: [CloudComputerSummary]? }
        do {
            let envelope: Envelope = try await APIClient.shared.get(path: "provisioned-instances")
            computers = (envelope.instances ?? []).filter { $0.status != "deleted" }
        } catch is CancellationError {
        } catch {
            if let list: [CloudComputerSummary] = try? await APIClient.shared.get(path: "provisioned-instances") {
                computers = list.filter { $0.status != "deleted" }
            } else if AuthManager.shared.isSignedIn {
                self.error = "Couldn’t load cloud computers"
            }
        }
    }
}

// MARK: - Workflows

/// FabricWorkflowsPanel on the phone: the compiled workflow specs on a
/// paired computer, or why there are none.
struct ACIWorkflowsSection: View {
    let runtime: RelayRuntimeDevice?
    @State private var specs: [ACIComputersClient.WorkflowSpec] = []
    @State private var loading = true
    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("WORKFLOWS (RECORD → TEACH → BATCH → VERIFY)")
                .font(.system(size: 14, weight: .semibold))
                .tracking(0.4)
                .foregroundColor(Color("TextPrimary"))
            if let runtime {
                Text("On \(runtime.displayName)")
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
                specList
            } else {
                Text("Workflows run on a paired computer running Allternit Desktop.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderStrong"), lineWidth: 1))
        .task(id: runtime?.id) { await load() }
    }

    private var specList: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("WORKFLOW SPECS")
                    .font(.system(size: 12, weight: .semibold))
                    .tracking(0.4)
                    .foregroundColor(Color("TextTertiary"))
                Spacer()
                Button { Task { await load() } } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 32, height: 32)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Refresh workflow specs")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderStrong")).frame(height: 1) }
            Group {
                if loading {
                    Text("Loading…").foregroundColor(Color("TextTertiary"))
                } else if let error {
                    // A failed load reads like the machines list's: a neutral
                    // line and a retry, never a red status string.
                    VStack(alignment: .leading, spacing: 10) {
                        Text(error)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                        Button { Task { await load() } } label: {
                            Text("Try again")
                                .font(.system(size: 13, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 14)
                                .frame(minHeight: 32)
                                .background(Color("BgPanel"))
                                .clipShape(Capsule())
                                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                        .accessibilityHint("Loads the workflow specs again")
                    }
                } else if specs.isEmpty {
                    Text("No compiled workflow specs on the gateway yet. Compile a recording first (POST /v1/browser-skills/from-recording).")
                        .foregroundColor(Color("TextTertiary"))
                } else {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(specs) { spec in
                            HStack(spacing: 8) {
                                Image(systemName: "point.topleft.down.to.point.bottomright.curvepath")
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.statusInfo)
                                Text(spec.title ?? spec.skillId)
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Spacer()
                                if spec.hasNetworkTrace == true {
                                    Image(systemName: "checkmark.shield")
                                        .font(.system(size: 12))
                                        .foregroundColor(Theme.statusSuccess)
                                        .accessibilityLabel("Has NetworkTrace")
                                } else {
                                    Text("NO TRACE")
                                        .font(.system(size: 10))
                                        .tracking(0.6)
                                        .foregroundColor(Color("TextTertiary"))
                                }
                            }
                            .padding(.vertical, 8)
                            .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                        }
                    }
                }
            }
            .font(.system(size: 12))
            .padding(12)
        }
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color("BorderStrong"), lineWidth: 1))
    }

    private func load() async {
        guard let runtime else { loading = false; return }
        loading = true
        error = nil
        defer { loading = false }
        do {
            specs = try await ACIComputersClient.shared.listWorkflowSpecs(runtimeId: runtime.id)
        } catch is CancellationError {
        } catch {
            specs = []
            let message = error.localizedDescription
            self.error = message.isEmpty ? "Could not load workflow specs." : message
        }
    }
}


// MARK: - Computers console (flag `computers_console`)

/// Console list from `GET /api/v1/computers/console`: a status strip, a
/// "Needs attention" group, then one card per computer with CPU / memory /
/// disk meters. States: loading, empty, error (Try again), and "not
/// available on your account yet" (route missing). Asleep cloud computers
/// show their last sample and are never woken. Neutral grey meters; a
/// meter turns amber at 85% or more.
struct ACIConsoleSection: View {
    private enum Phase { case loading, ready([ACIComputersClient.ConsoleComputer]), unavailable, failed(String) }
    @State private var phase: Phase = .loading
    @State private var attempt = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            content
        }
        .padding(.bottom, 8)
        .task(id: attempt) { await load() }
    }

    @ViewBuilder
    private var content: some View {
        switch phase {
        case .loading:
            Text("Loading computers…")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity).padding(.vertical, 32)
        case .unavailable:
            note("Computers console isn't available on your account yet.")
        case .failed(let message):
            VStack(spacing: 8) {
                note(message)
                ACIPillButton(title: "Try again") { attempt += 1 }
            }
        case .ready(let rows) where rows.isEmpty:
            note("No computers yet. Add one above and it shows its health here.")
        case .ready(let rows):
            let online = rows.filter(\.isOnline).count
            Text("\(rows.count) computers · \(online) online")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            let flagged = rows.filter { !$0.attention.isEmpty }
            if !flagged.isEmpty {
                Text("Needs attention")
                    .font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                ForEach(flagged) { row in
                    HStack(spacing: 8) {
                        Circle().fill(Theme.statusWarning).frame(width: 8, height: 8)
                        Text("\(row.name): \(row.attention.joined(separator: ", "))")
                            .font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                        Spacer(minLength: 0)
                    }
                    .padding(12)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .accessibilityElement(children: .combine)
                }
            }
            ForEach(rows) { card($0) }
        }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity).padding(.vertical, 24)
    }

    private func card(_ c: ACIComputersClient.ConsoleComputer) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Circle().fill(c.isOnline ? Theme.statusSuccess : Color("TextTertiary")).frame(width: 8, height: 8)
                Text(c.name).font(.system(size: 16, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 0)
                Text(c.isAsleep ? "Asleep" : (c.isOnline ? "Online" : "Offline"))
                    .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
            Text([c.provider, c.region, c.os].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            if let s = c.lastSample {
                meter("CPU", s.cpu.percent)
                meter("Memory", s.memoryPercent)
                meter("Disk", s.diskPercent)
                if c.isAsleep {
                    Text("Last sample while awake. We never wake it to read it.")
                        .font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
            } else {
                Text("No health sample yet")
                    .font(.system(size: 13)).foregroundColor(Color("TextTertiary"))
            }
            if let n = c.servicesCount {
                Text("\(n) services").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .combine)
    }

    private func meter(_ label: String, _ percent: Double) -> some View {
        let p = min(max(percent, 0), 100)
        return HStack(spacing: 10) {
            Text(label).font(.system(size: 13)).foregroundColor(Color("TextSecondary")).frame(width: 56, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Color("TextPrimary").opacity(0.1))
                    Capsule().fill(p >= 85 ? Theme.statusWarning : Color("TextPrimary").opacity(0.5))
                        .frame(width: geo.size.width * p / 100)
                }
            }
            .frame(height: 6)
            Text("\(Int(p))%").font(.system(size: 13)).foregroundColor(Color("TextPrimary")).frame(width: 40, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label) \(Int(p)) percent")
    }

    private func load() async {
        phase = .loading
        do {
            phase = .ready(try await ACIComputersClient.shared.listConsole())
        } catch ACIComputersClient.Failure.notAvailable {
            phase = .unavailable
        } catch {
            phase = .failed((error as? LocalizedError)?.errorDescription ?? "Couldn't load computers. Try again.")
        }
    }
}
