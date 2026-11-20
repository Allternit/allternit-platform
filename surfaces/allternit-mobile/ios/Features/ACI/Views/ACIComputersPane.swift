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
                             + Text("ao fabric pair").font(.system(size: 14, design: .monospaced))
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
    @State private var selected: ACIComputersClient.ConsoleComputer? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            content
        }
        .padding(.bottom, 8)
        .task(id: attempt) { await load() }
        .sheet(item: $selected) { ACIConsoleDetailView(computer: $0) }
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
            ForEach(rows) { row in
                Button { selected = row } label: { card(row) }
                    .buttonStyle(.plain)
                    .accessibilityHint("Opens details")
            }
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

// MARK: - Console detail (flag `computers_console`)

/// One computer's detail: Overview, Services (+ live logs), Usage, Ports,
/// Access, Backups, Health (approve a fix) and Settings.
/// Reads `GET /api/v1/computers/:id/host/*` and `/metrics` through the
/// cloud relay. Changes (a Health fix, restore, restart, move, rebuild,
/// SSH keys, rename, unpair) always ask first. A 409 means the computer is
/// asleep, so the last sample shows with "Asleep, not woken" and nothing
/// is retried; 404/501 shows "not available". Live logs run only while
/// the Services log view is on screen and the app is in the foreground.
struct ACIConsoleDetailView: View {
    let computer: ACIComputersClient.ConsoleComputer
    @Environment(\.dismiss) private var dismiss

    private enum Section: String, CaseIterable, Identifiable {
        case overview = "Overview", services = "Services", usage = "Usage", ports = "Ports"
        case terminal = "Terminal", files = "Files"
        case access = "Access", backups = "Backups", health = "Health", settings = "Settings"
        var id: String { rawValue }
    }
    @State private var section: Section = .overview

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(Section.allCases) { item in
                            Button { section = item } label: {
                                Text(item.rawValue)
                                    .font(.system(size: 14, weight: section == item ? .semibold : .regular))
                                    .foregroundColor(Color("TextPrimary"))
                                    .padding(.horizontal, 14).frame(minHeight: 36)
                                    .background(section == item ? Color("FillNeutral") : Color.clear)
                                    .clipShape(Capsule())
                                    .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(section == item ? .isSelected : [])
                        }
                    }
                    .padding(.horizontal, 16).padding(.vertical, 10)
                }
                if section == .terminal {
                    ACIConsoleTerminalTab(computer: computer)
                } else if section == .files {
                    ACIConsoleFilesTab(computer: computer)
                } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        switch section {
                        case .terminal, .files: EmptyView()
                        case .overview: ACIConsoleOverviewTab(computer: computer)
                        case .services: ACIConsoleServicesTab(computer: computer)
                        case .usage: ACIConsoleUsageTab(computer: computer)
                        case .ports: ACIConsolePortsTab(computer: computer)
                        case .access: ACIConsoleAccessTab(computer: computer)
                        case .backups: ACIConsoleBackupsTab(computer: computer)
                        case .health: ACIConsoleHealthTab(computer: computer)
                        case .settings: ACIConsoleSettingsTab(computer: computer)
                        }
                    }
                    .padding(16)
                }
                }
            }
            .background(Color("BgPrimary"))
            .navigationTitle(computer.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
        }
    }
}

/// Loads one host read and exposes the five states every tab shares.
enum ACIConsoleLoad<T> {
    case loading
    case ready(T)
    case asleep(ACIComputersClient.ConsoleSnapshot?)
    case unavailable
    case failed(String)

    static func run(_ work: () async throws -> T) async -> ACIConsoleLoad<T> {
        do { return .ready(try await work()) }
        catch ACIComputersClient.Failure.notAvailable { return .unavailable }
        catch ACIComputersClient.Failure.asleep(let s) { return .asleep(s) }
        catch is CancellationError { return .loading }
        catch { return .failed((error as? LocalizedError)?.errorDescription ?? "Couldn't load this. Try again.") }
    }
}

func consoleNote(_ text: String) -> some View {
    Text(text)
        .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity).padding(.vertical, 24)
}

func consoleCard<C: View>(@ViewBuilder _ content: () -> C) -> some View {
    VStack(alignment: .leading, spacing: 10) { content() }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
}

private func consoleMeter(_ label: String, _ percent: Double) -> some View {
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

func consoleRow(_ k: String, _ v: String) -> some View {
    HStack(alignment: .firstTextBaseline) {
        Text(k).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        Spacer(minLength: 12)
        Text(v).font(.system(size: 14)).foregroundColor(Color("TextPrimary")).multilineTextAlignment(.trailing)
    }
    .accessibilityElement(children: .combine)
}

private func consoleDuration(_ sec: Double) -> String {
    let s = Int(sec)
    let d = s / 86_400, h = (s % 86_400) / 3_600, m = (s % 3_600) / 60
    if d > 0 { return "\(d)d \(h)h" }
    if h > 0 { return "\(h)h \(m)m" }
    return "\(max(m, 1))m"
}

func consoleBytes(_ b: Double) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .binary)
}

// MARK: Overview

private struct ACIConsoleOverviewTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<ACIComputersClient.HostSnapshot> = .loading
    @State private var attempt = 0

    var body: some View {
        Group {
            switch state {
            case .loading: consoleNote("Loading…")
            case .unavailable: consoleNote("Live details aren't available for this computer yet.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .asleep(let last):
                consoleNote("Asleep, not woken. This is the last sample from while it was awake.")
                if let last = last ?? computer.lastSample { meters(cpu: last.cpu.percent, mem: last.memoryPercent, disk: last.diskPercent) }
                facts(snapshot: nil)
            case .ready(let s):
                meters(cpu: s.cpu.percent, mem: s.memoryPercent, disk: s.diskPercent)
                facts(snapshot: s)
            }
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.hostSnapshot(id: computer.id) } }
    }

    private func meters(cpu: Double, mem: Double, disk: Double) -> some View {
        consoleCard {
            Text("Right now").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            consoleMeter("CPU", cpu); consoleMeter("Memory", mem); consoleMeter("Disk", disk)
        }
    }

    private func facts(snapshot s: ACIComputersClient.HostSnapshot?) -> some View {
        consoleCard {
            Text("About").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            consoleRow("System", [s?.os?.name, s?.os?.version].compactMap { $0 }.joined(separator: " ").nilIfEmpty ?? computer.os)
            if let arch = s?.os?.arch { consoleRow("Architecture", arch) }
            if let cores = s?.cpu.cores { consoleRow("Processor", "\(cores) cores") }
            if let s { consoleRow("Memory", "\(consoleBytes(s.memory.usedBytes)) of \(consoleBytes(s.memory.totalBytes))") }
            consoleRow("Provider", [computer.provider, computer.region].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
            if let a = computer.address, !a.isEmpty { consoleRow("Address", a) }
            if let up = s?.uptimeSec { consoleRow("Uptime", consoleDuration(up)) }
        }
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

// MARK: Services + logs

private struct ACIConsoleServicesTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.HostService]> = .loading
    @State private var attempt = 0
    @State private var logsFor: ACIComputersClient.HostService? = nil

    var body: some View {
        Group {
            switch state {
            case .loading: consoleNote("Loading services…")
            case .unavailable: consoleNote("Services aren't available for this computer yet.")
            case .asleep: consoleNote("Asleep, not woken. Services show when it is awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let rows) where rows.isEmpty: consoleNote("No services reported.")
            case .ready(let rows):
                ForEach(rows) { svc in
                    Button { logsFor = svc } label: {
                        HStack(spacing: 10) {
                            Circle().fill(color(svc.state)).frame(width: 8, height: 8)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(svc.name).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
                                Text(svc.description ?? svc.unit).font(.system(size: 12)).foregroundColor(Color("TextTertiary")).lineLimit(1)
                            }
                            Spacer(minLength: 0)
                            Text(svc.state.capitalized).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                            Image(systemName: "chevron.right").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                        }
                        .padding(14)
                        .background(Color("FillNeutral"))
                        .clipShape(RoundedRectangle(cornerRadius: 14))
                        .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .accessibilityHint("Shows live logs")
                }
            }
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.hostServices(id: computer.id) } }
        .sheet(item: $logsFor) { ACIConsoleLogsView(computer: computer, service: $0) }
    }

    private func color(_ state: String) -> Color {
        switch state {
        case "running": return Theme.statusSuccess
        case "failed": return Theme.statusWarning
        default: return Color("TextTertiary")
        }
    }
}

/// Live log stream for one unit. Streams only while visible and active in
/// the foreground; lines are masked at the node and kept in memory only
/// (last 300 lines).
private struct ACIConsoleLogsView: View {
    let computer: ACIComputersClient.ConsoleComputer
    let service: ACIComputersClient.HostService
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase
    @State private var lines: [ACIComputersClient.LogLine] = []
    @State private var note: String? = nil
    @State private var errorsOnly = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Toggle("Errors only", isOn: $errorsOnly)
                    .font(.system(size: 14)).padding(.horizontal, 16).padding(.vertical, 8)
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 4) {
                            if let note { consoleNote(note) }
                            else if lines.isEmpty { consoleNote("Waiting for log lines…") }
                            ForEach(Array(shown.enumerated()), id: \.offset) { i, l in
                                Text(l.message)
                                    .font(.system(size: 12, design: .monospaced))
                                    .foregroundColor(tint(l.level))
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .id(i)
                            }
                        }
                        .padding(12)
                    }
                    .background(Color("FillNeutral"))
                    .onChange(of: shown.count) { _, n in if n > 0 { proxy.scrollTo(n - 1, anchor: .bottom) } }
                }
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Logs · \(service.name)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
        }
        // Restarts when the app returns to the foreground; cancelled when it leaves or the sheet closes.
        .task(id: scenePhase) {
            guard scenePhase == .active else { return }
            note = nil
            do {
                for try await line in ACIComputersClient.shared.logStream(id: computer.id, unit: service.unit) {
                    lines.append(line)
                    if lines.count > 300 { lines.removeFirst(lines.count - 300) }
                }
            } catch ACIComputersClient.Failure.notAvailable {
                note = "Logs aren't available for this computer yet."
            } catch ACIComputersClient.Failure.asleep {
                note = "Asleep, not woken."
            } catch is CancellationError {
            } catch {
                note = (error as? LocalizedError)?.errorDescription ?? "The log stream stopped."
            }
        }
    }

    private var shown: [ACIComputersClient.LogLine] {
        errorsOnly ? lines.filter { $0.level == "error" || $0.level == "warn" } : lines
    }

    private func tint(_ level: String) -> Color {
        switch level {
        case "error": return Theme.statusWarning
        case "warn": return Theme.statusWarning.opacity(0.8)
        default: return Color("TextPrimary")
        }
    }
}

// MARK: Usage

private struct ACIConsoleUsageTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.MetricSeries]> = .loading
    @State private var attempt = 0

    var body: some View {
        Group {
            switch state {
            case .loading: consoleNote("Loading usage…")
            case .unavailable: consoleNote("Usage history isn't available for this computer yet.")
            case .asleep: consoleNote("Asleep, not woken. History shows what it recorded while awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let series) where series.allSatisfy({ $0.values.isEmpty }):
                consoleNote("No usage recorded in the last 24 hours.")
            case .ready(let series):
                chart("CPU", "cpu", series, percent: true)
                chart("Memory", "memory", series, percent: true)
                chart("Disk", "disk", series, percent: true)
                chart("Network in", "net_rx", series, percent: false)
                chart("Network out", "net_tx", series, percent: false)
            }
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.metrics(id: computer.id) } }
    }

    /// Simple bars: one per sample, neutral grey, amber at 85% or more for percent charts.
    @ViewBuilder
    private func chart(_ title: String, _ metric: String, _ all: [ACIComputersClient.MetricSeries], percent: Bool) -> some View {
        if let values = all.first(where: { $0.metric == metric })?.values, !values.isEmpty {
            let top = percent ? 100 : max(values.max() ?? 1, 1)
            let bars = Self.bucket(values, into: 48)
            consoleCard {
                HStack {
                    Text("\(title) · 24 hours").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Spacer(minLength: 0)
                    Text(percent ? "\(Int(values.last ?? 0))%" : "\(consoleBytes(values.last ?? 0))/s")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
                HStack(alignment: .bottom, spacing: 2) {
                    ForEach(Array(bars.enumerated()), id: \.offset) { _, v in
                        RoundedRectangle(cornerRadius: 1.5)
                            .fill(percent && v >= 85 ? Theme.statusWarning : Color("TextPrimary").opacity(0.45))
                            .frame(height: max(2, 64 * CGFloat(min(v / top, 1))))
                            .frame(maxWidth: .infinity)
                    }
                }
                .frame(height: 64, alignment: .bottom)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(title), last 24 hours, now \(Int(values.last ?? 0))\(percent ? " percent" : "")")
            }
        }
    }

    private static func bucket(_ v: [Double], into n: Int) -> [Double] {
        guard v.count > n else { return v }
        let size = Double(v.count) / Double(n)
        return (0..<n).map { i in
            let slice = v[Int(Double(i) * size)..<min(v.count, max(Int(Double(i + 1) * size), Int(Double(i) * size) + 1))]
            return slice.reduce(0, +) / Double(slice.count)
        }
    }
}

// MARK: Ports

private struct ACIConsolePortsTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.HostPort]> = .loading
    @State private var attempt = 0

    var body: some View {
        Group {
            switch state {
            case .loading: consoleNote("Loading ports…")
            case .unavailable: consoleNote("Ports aren't available for this computer yet.")
            case .asleep: consoleNote("Asleep, not woken. Ports show when it is awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let rows) where rows.isEmpty: consoleNote("No listening ports.")
            case .ready(let rows):
                ForEach(rows.sorted { $0.port < $1.port }) { p in
                    HStack(spacing: 10) {
                        Text("\(p.port)").font(.system(size: 15, weight: .medium, design: .monospaced)).foregroundColor(Color("TextPrimary"))
                        Text(p.protocol.uppercased()).font(.system(size: 11)).foregroundColor(Color("TextTertiary"))
                        Text(p.process ?? "Unknown program").font(.system(size: 14)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                        Spacer(minLength: 0)
                        Text(label(p.exposure))
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(p.exposure == "public" ? Theme.statusWarning : Color("TextSecondary"))
                    }
                    .padding(14)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
                    .accessibilityElement(children: .combine)
                }
            }
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.hostPorts(id: computer.id) } }
    }

    private func label(_ exposure: String) -> String {
        switch exposure {
        case "public": return "Open to the internet"
        case "lan": return "Local network"
        default: return "This computer only"
        }
    }
}

// MARK: - Console manage tabs: Access, Backups, Health, Settings

/// Press and hold for one second to confirm (the iOS form of Arc HoldToConfirm).
/// VoiceOver users get a plain activate action instead of the hold.
private struct ACIHoldButton: View {
    let title: String
    var danger = false
    var disabled = false
    let action: () -> Void
    @State private var progress: CGFloat = 0

    var body: some View {
        ZStack {
            GeometryReader { geo in
                Rectangle()
                    .fill((danger ? Theme.statusError : Color("TextPrimary")).opacity(0.18))
                    .frame(width: geo.size.width * progress)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(danger ? Theme.statusError : Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, minHeight: 48)
        .background(Color("FillNeutral"))
        .clipShape(Capsule())
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .opacity(disabled ? 0.45 : 1)
        .allowsHitTesting(!disabled)
        .onLongPressGesture(minimumDuration: 1.0, maximumDistance: 40, perform: {
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            action()
        }, onPressingChanged: { pressing in
            if pressing { withAnimation(.linear(duration: 1.0)) { progress = 1 } }
            else { withAnimation(.easeOut(duration: 0.2)) { progress = 0 } }
        })
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityHint("Press and hold to confirm")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { if !disabled { action() } }
    }
}

/// Plain sentence for a failed change. Never a status code.
func consoleFailureText(_ error: Error) -> String {
    switch error {
    case ACIComputersClient.Failure.notAvailable: return "This isn't available for this computer yet."
    case ACIComputersClient.Failure.asleep: return "This computer is asleep. It is not woken for this. Nothing was changed."
    default: return (error as? LocalizedError)?.errorDescription ?? "That didn't work. Nothing was changed."
    }
}

private func consoleAgo(_ raw: String) -> String {
    guard let date = ACIDates.parse(raw) else { return raw }
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
}

private func consoleWhen(_ raw: String) -> String {
    guard let date = ACIDates.parse(raw) else { return raw }
    return date.formatted(date: .abbreviated, time: .shortened)
}

/// Newest backup newer than 24 hours, or nil.
private func consoleFreshBackup(_ backups: [ACIComputersClient.CloudBackup]) -> ACIComputersClient.CloudBackup? {
    backups.compactMap { b in ACIDates.parse(b.createdAt).map { (b, $0) } }
        .filter { Date().timeIntervalSince($0.1) < 24 * 3600 }
        .max { $0.1 < $1.1 }?.0
}

private func consoleStatusLine(_ text: String?, error: Bool = false) -> some View {
    Group {
        if let text {
            Text(text).font(.system(size: 13))
                .foregroundColor(error ? Theme.statusError : Color("TextSecondary"))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: Access

private struct ACIConsoleAccessTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.SSHKey]> = .loading
    @State private var attempt = 0
    @State private var adding = false
    @State private var removing: ACIComputersClient.SSHKey? = nil
    @State private var message: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            consoleCard {
                Text("Who can use it").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                if computer.group == "Shared with you" {
                    consoleRow("You", "Member")
                    Text("It was shared with you. The owner decides who else can use it.")
                        .font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                } else {
                    consoleRow("You", "Owner")
                    Text("Members of your organization can use it when it is shared with them. Agents ask before they change anything.")
                        .font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
            }
            if computer.kind == "cloud" {
                consoleCard {
                    Text("Hosted by Allternit").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Text("Part of your plan. You reach it through the Allternit relay. Nothing is opened to the internet.")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            } else {
                consoleCard {
                    Text("Pairing").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Text("To pair another computer, install Gizzi there and run:")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                    Text("gizzi pair").font(.system(size: 13, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12).padding(.vertical, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color("TextPrimary").opacity(0.08))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .textSelection(.enabled)
                    Text("Install: npm install -g @allternit/gizzi-code")
                        .font(.system(size: 12)).foregroundColor(Color("TextTertiary")).textSelection(.enabled)
                }
            }
            keysCard
        }
        .sheet(isPresented: $adding) {
            ACIConsoleAddKeySheet(computer: computer) { attempt += 1 }
        }
        .confirmationDialog("Remove this key?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                            titleVisibility: .visible, presenting: removing) { key in
            Button("Remove \(key.label)", role: .destructive) { Task { await remove(key) } }
            Button("Cancel", role: .cancel) {}
        } message: { _ in Text("It can no longer sign in to this computer.") }
    }

    private var keysCard: some View {
        consoleCard {
            HStack {
                Text("SSH keys").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Spacer()
                if case .ready = state { ACIPillButton(title: "Add a key") { adding = true } }
            }
            switch state {
            case .loading: consoleNote("Loading…")
            case .unavailable: consoleNote("SSH keys aren't available on your account yet.")
            case .asleep: consoleNote("Asleep, not woken. Keys show when it is awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let keys):
                if keys.isEmpty {
                    consoleNote("No keys yet. Add one to sign in over SSH.")
                } else {
                    ForEach(keys) { key in
                        HStack(spacing: 10) {
                            Image(systemName: "key").foregroundColor(Color("TextTertiary"))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(key.label).font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                                Text(key.kind == "agent" ? "Short-lived, rotates on its own" : (key.fingerprint ?? ""))
                                    .font(.system(size: 11, design: .monospaced)).foregroundColor(Color("TextTertiary"))
                                    .lineLimit(1).truncationMode(.middle)
                            }
                            Spacer()
                            if key.kind != "agent" {
                                Button { removing = key } label: { Image(systemName: "trash") }
                                    .frame(minWidth: 44, minHeight: 44)
                                    .accessibilityLabel("Remove \(key.label)")
                            }
                        }
                    }
                }
            }
            consoleStatusLine(message)
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.sshKeys(id: computer.id) } }
    }

    private func remove(_ key: ACIComputersClient.SSHKey) async {
        do {
            try await ACIComputersClient.shared.removeSSHKey(id: computer.id, keyId: key.id)
            message = "Removed \(key.label)."
            attempt += 1
        } catch { message = consoleFailureText(error) }
    }
}

private struct ACIConsoleAddKeySheet: View {
    let computer: ACIComputersClient.ConsoleComputer
    let onAdded: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var label = ""
    @State private var publicKey = ""
    @State private var busy = false
    @State private var error: String? = nil

    private var looksPrivate: Bool { publicKey.contains("PRIVATE KEY") }
    private var valid: Bool {
        let k = publicKey.trimmingCharacters(in: .whitespacesAndNewlines)
        return !label.trimmingCharacters(in: .whitespaces).isEmpty && !looksPrivate
            && (k.hasPrefix("ssh-") || k.hasPrefix("ecdsa-") || k.hasPrefix("sk-"))
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Name") { TextField("Studio Mac", text: $label) }
                Section("Public key") {
                    TextField("ssh-ed25519 AAAA…", text: $publicKey, axis: .vertical)
                        .font(.system(size: 13, design: .monospaced))
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .lineLimit(3...6)
                    if looksPrivate {
                        Text("That is a private key. Paste the public key (the .pub file) and keep the private one to yourself.")
                            .font(.system(size: 12)).foregroundColor(Theme.statusError)
                    }
                }
                if let error { Text(error).font(.system(size: 12)).foregroundColor(Theme.statusError) }
            }
            .navigationTitle("Add a key").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(busy ? "Adding…" : "Add") { Task { await add() } }.disabled(!valid || busy)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func add() async {
        busy = true; defer { busy = false }
        do {
            try await ACIComputersClient.shared.addSSHKey(
                id: computer.id, label: label.trimmingCharacters(in: .whitespaces),
                publicKey: publicKey.trimmingCharacters(in: .whitespacesAndNewlines))
            onAdded(); dismiss()
        } catch { error = consoleFailureText(error) }
    }
}

// MARK: Backups

private struct ACIConsoleBackupsTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.CloudBackup]> = .loading
    @State private var attempt = 0
    @State private var confirming: ACIComputersClient.CloudBackup? = nil
    @State private var busy = false
    @State private var message: String? = nil
    @State private var showPoints = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if computer.kind == "cloud" { cloudBody } else {
                consoleCard {
                    Text("Change records only").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    Text("Allternit does not copy the disk of a paired computer. Every fix writes a change record before it runs, so you can see what changed and undo it from Health.")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            }
            consoleCard {
                Text("Snapshots on demand").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Text("Taking a snapshot yourself isn't supported yet. Daily backups and change records are what protect a fix.")
                    .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
        }
        .sheet(isPresented: $showPoints) { NavigationStack { CloudComputerBackupsView() } }
        .confirmationDialog("Restore this backup?", isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                            titleVisibility: .visible, presenting: confirming) { backup in
            Button("Restore \(consoleWhen(backup.createdAt))", role: .destructive) { Task { await restore(backup) } }
            Button("Cancel", role: .cancel) {}
        } message: { _ in Text("The computer goes back to this point and restarts. Changes made after it are lost.") }
    }

    @ViewBuilder private var cloudBody: some View {
        consoleCard {
            Text("Daily backups").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            switch state {
            case .loading: consoleNote("Loading…")
            case .unavailable: consoleNote("Backups aren't available for this computer yet.")
            case .asleep: consoleNote("Asleep, not woken. Backups show when it is awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let list):
                if list.isEmpty { consoleNote("No backups yet. The first one is made within a day.") }
                ForEach(list) { b in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(consoleWhen(b.createdAt)).font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                            Text([consoleAgo(b.createdAt), b.sizeBytes.map(consoleBytes)].compactMap { $0 }.joined(separator: " · "))
                                .font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                        }
                        Spacer()
                        ACIPillButton(title: "Restore", accessibilityLabel: "Restore backup from \(consoleWhen(b.createdAt))") { confirming = b }
                            .disabled(busy)
                    }
                }
            }
            consoleStatusLine(message)
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.cloudBackups(id: computer.id) } }
        consoleCard {
            Text("Restore points and copies").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text("Your existing restore points and \"Download a copy\" stay here.")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            ACIPillButton(title: "Open") { showPoints = true }
        }
    }

    private func restore(_ backup: ACIComputersClient.CloudBackup) async {
        busy = true; defer { busy = false }
        do {
            try await ACIComputersClient.shared.restoreCloudBackup(id: computer.id, backupId: backup.id)
            message = "Restoring from \(consoleWhen(backup.createdAt)). The computer restarts."
        } catch { message = consoleFailureText(error) }
    }
}

// MARK: Health

private struct ACIConsoleHealthTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var state: ACIConsoleLoad<[ACIComputersClient.HostFinding]> = .loading
    @State private var attempt = 0
    @State private var fixing: ACIComputersClient.HostFinding? = nil

    var body: some View {
        Group {
            switch state {
            case .loading: consoleNote("Loading…")
            case .unavailable: consoleNote("Health checks aren't available for this computer yet.")
            case .asleep: consoleNote("Asleep, not woken. Health shows when it is awake.")
            case .failed(let m):
                VStack(spacing: 8) { consoleNote(m); ACIPillButton(title: "Try again") { attempt += 1 } }
            case .ready(let findings):
                if findings.isEmpty {
                    consoleNote("Nothing needs a look. Disk, services, updates and open ports are all fine.")
                } else {
                    ForEach(findings) { f in
                        consoleCard {
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Circle().fill(tint(f.severity)).frame(width: 8, height: 8)
                                Text(f.title).font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                            }
                            Text(f.detail).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                            if f.suggestedAction != nil {
                                ACIPillButton(title: "Fix", accessibilityLabel: "Fix: \(f.title)") { fixing = f }
                            }
                        }
                    }
                }
            }
        }
        .task(id: attempt) { state = .loading; state = await .run { try await ACIComputersClient.shared.hostFindings(id: computer.id) } }
        .sheet(item: $fixing) { f in ACIConsoleFixSheet(computer: computer, finding: f) { attempt += 1 } }
    }

    private func tint(_ severity: String) -> Color {
        switch severity {
        case "critical": return Theme.statusError
        case "warning": return Theme.statusWarning
        default: return Color("TextTertiary")
        }
    }
}

/// The approval sheet: the exact action, the restore point status, then Hold to approve.
struct ACIConsoleFixSheet: View {
    let computer: ACIComputersClient.ConsoleComputer
    let finding: ACIComputersClient.HostFinding
    let onChanged: () -> Void
    /// Words for the review card and title (the Space and snippet runs reuse this sheet).
    var heading = "What is wrong"
    var navTitle = "Approve a fix"
    @Environment(\.dismiss) private var dismiss

    private enum Phase: Equatable {
        case review
        case running
        case done(changeId: String, undo: ACIComputersClient.HostAction?)
        case undone(changeId: String)
        case needsElevation
        case failed(String)
    }
    @State private var phase: Phase = .review
    @State private var restore: ACIConsoleLoad<[ACIComputersClient.CloudBackup]> = .loading

    private var action: ACIComputersClient.HostAction? { finding.suggestedAction }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    consoleCard {
                        Text(heading).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        Text(finding.title).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        Text(finding.detail).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                    }
                    if let action {
                        consoleCard {
                            Text("This will run on \(computer.name)").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                            if let label = action.label, !label.isEmpty {
                                Text(label).font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
                            }
                            Text("\(action.type) \(action.target)")
                                .font(.system(size: 13, design: .monospaced)).foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 12).padding(.vertical, 8)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Color("TextPrimary").opacity(0.08))
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                                .textSelection(.enabled)
                        }
                        consoleCard {
                            Text("Restore point").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                            restoreStatus
                        }
                        footer(action)
                    } else {
                        consoleNote("There is no one-tap fix for this one.")
                    }
                }
                .padding(16)
            }
            .background(Color("BgPrimary"))
            .navigationTitle(navTitle).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Close") { dismiss() } } }
        }
        .task {
            guard computer.kind == "cloud" else { return }
            restore = await .run { try await ACIComputersClient.shared.cloudBackups(id: computer.id) }
        }
    }

    @ViewBuilder private var restoreStatus: some View {
        if computer.kind != "cloud" {
            Text("A change record is written first, so you can see exactly what changed and undo it.")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        } else {
            switch restore {
            case .loading: Text("Checking backups…").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            case .ready(let list):
                if let fresh = consoleFreshBackup(list) {
                    Label("Backup from \(consoleAgo(fresh.createdAt)). A change record is written too.", systemImage: "checkmark.circle")
                        .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                } else {
                    Label("No backup newer than 24 hours. You can still approve, but this change could not be rolled back from a backup.",
                          systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 13)).foregroundColor(Theme.statusWarning)
                }
            case .asleep:
                Text("Asleep, not woken. Backups can't be checked right now.").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            case .unavailable, .failed:
                Label("Couldn't check backups. A change record is still written first.", systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: 13)).foregroundColor(Theme.statusWarning)
            }
        }
    }

    @ViewBuilder private func footer(_ action: ACIComputersClient.HostAction) -> some View {
        switch phase {
        case .review:
            VStack(spacing: 10) {
                ACIHoldButton(title: "Hold to approve") { Task { await run(action, undoing: false) } }
                Button("Not now") { dismiss() }.font(.system(size: 14)).foregroundColor(Color("TextSecondary")).frame(minHeight: 44)
            }
        case .running:
            HStack(spacing: 8) { ProgressView(); Text("Running…").font(.system(size: 14)).foregroundColor(Color("TextSecondary")) }
        case .done(let changeId, let undo):
            consoleCard {
                Label("Approved by you. It ran.", systemImage: "checkmark.circle.fill")
                    .font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                consoleRow("Change", changeId)
                if let undo {
                    Text("You can undo this. Undo needs its own approval.").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                    ACIHoldButton(title: "Hold to undo") { Task { await run(undo, undoing: true) } }
                }
            }
        case .undone(let changeId):
            consoleCard {
                Label("Undone.", systemImage: "arrow.uturn.backward.circle.fill")
                    .font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                consoleRow("Change", changeId)
            }
        case .needsElevation:
            consoleCard {
                Label("One-time setup needed", systemImage: "lock.shield").font(.system(size: 14, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Text("This change needs administrator rights, and the helper that grants them isn't set up on \(computer.name). You set it up once, on that computer. It asks for the admin password one time and never stores it. Nothing was changed. Come back and approve again after the setup.")
                    .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
        case .failed(let message):
            consoleCard {
                Label(message, systemImage: "exclamationmark.triangle.fill").font(.system(size: 13)).foregroundColor(Theme.statusError)
                ACIPillButton(title: "Try again") { phase = .review }
            }
        }
    }

    private func run(_ action: ACIComputersClient.HostAction, undoing: Bool) async {
        phase = .running
        do {
            let approval = try await ACIComputersClient.shared.requestApproval(id: computer.id, type: action.type, target: action.target)
            let result = try await ACIComputersClient.shared.runHostAction(id: computer.id, type: action.type, target: action.target, approvalId: approval)
            phase = undoing ? .undone(changeId: result.changeId) : .done(changeId: result.changeId, undo: result.undo)
            onChanged()
        } catch ACIComputersClient.Failure.needsElevation {
            phase = .needsElevation
        } catch is CancellationError {
            phase = .review
        } catch { phase = .failed(consoleFailureText(error)) }
    }
}

// MARK: Settings

private struct ACIConsoleSettingsTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @Environment(\.dismiss) private var dismiss
    @State private var message: String? = nil
    @State private var isError = false
    @State private var busy = false

    // cloud
    private static let locations: [(String, String)] = [("nyc", "New York"), ("chi", "Chicago"), ("lon", "London"), ("syd", "Sydney")]
    @State private var location = ""
    @State private var confirmRestart = false
    @State private var confirmMove = false
    @State private var backups: ACIConsoleLoad<[ACIComputersClient.CloudBackup]> = .loading

    // paired
    @State private var name = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            switch computer.kind {
            case "cloud": cloudBody
            case "paired": pairedBody
            default:
                consoleNote("This is the computer you are holding. Manage it from Allternit Desktop.")
            }
            consoleStatusLine(message, error: isError)
        }
    }

    // MARK: cloud

    @ViewBuilder private var cloudBody: some View {
        consoleCard {
            Text("Location").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Picker("Location", selection: $location) {
                ForEach(Self.locations, id: \.0) { Text($0.1).tag($0.0) }
            }
            .pickerStyle(.menu)
            Text("Moving restarts the computer and takes a few minutes.").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            ACIPillButton(title: "Move") { confirmMove = true }
                .disabled(busy || location.isEmpty || location == currentLocation)
        }
        .onAppear { if location.isEmpty { location = currentLocation ?? "" } }
        .confirmationDialog("Move this computer?", isPresented: $confirmMove, titleVisibility: .visible) {
            Button("Move to \(Self.locations.first { $0.0 == location }?.1 ?? location)") {
                Task { await perform("Moving. The computer restarts.") { try await ACIComputersClient.shared.moveCloud(id: computer.id, location: location) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: { Text("It restarts, and anything running stops.") }

        consoleCard {
            Text("Restart").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text("Running work stops. Saved files stay.").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            ACIPillButton(title: "Restart") { confirmRestart = true }.disabled(busy)
        }
        .confirmationDialog("Restart this computer?", isPresented: $confirmRestart, titleVisibility: .visible) {
            Button("Restart", role: .destructive) {
                Task { await perform("Restarting.") { try await ACIComputersClient.shared.restartCloud(id: computer.id) } }
            }
            Button("Cancel", role: .cancel) {}
        }

        consoleCard {
            Text("Rebuild").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text("Rebuild erases the disk and sets the computer up again from the Ubuntu image, in the same location. Your Files drive copy stays.")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            rebuildGate
        }
        .task {
            backups = await .run { try await ACIComputersClient.shared.cloudBackups(id: computer.id) }
        }

        consoleCard {
            Text("Coming soon").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            ForEach(["Resize", "Grow disk", "Clone"], id: \.self) { item in
                HStack {
                    Text(item).font(.system(size: 14)).foregroundColor(Color("TextTertiary"))
                    Spacer()
                    Text("Coming soon").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isStaticText)
                .accessibilityHint("Not available yet")
            }
            Text("Prices show before you confirm, once they are set.").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
        }
    }

    @ViewBuilder private var rebuildGate: some View {
        switch backups {
        case .loading:
            Text("Checking backups…").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        case .ready(let list):
            if let fresh = consoleFreshBackup(list) {
                Label("Backup from \(consoleAgo(fresh.createdAt)).", systemImage: "checkmark.circle")
                    .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                ACIHoldButton(title: "Hold to rebuild", danger: true, disabled: busy) {
                    Task { await perform("Rebuilding. This takes a few minutes.") { try await ACIComputersClient.shared.rebuildCloud(id: computer.id) } }
                }
            } else {
                Label("Rebuild is held. There is no backup newer than 24 hours, and a rebuild erases the only copy of the disk.",
                      systemImage: "exclamationmark.triangle.fill").font(.system(size: 13)).foregroundColor(Theme.statusWarning)
                ACIHoldButton(title: "Hold to rebuild", danger: true, disabled: true) {}
            }
        case .asleep:
            Text("Asleep, not woken. Rebuild is held until the backup check can run.").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            ACIHoldButton(title: "Hold to rebuild", danger: true, disabled: true) {}
        case .unavailable:
            Text("Rebuild isn't available for this computer yet.").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        case .failed(let m):
            Text("\(m) Rebuild is held until the backup check works.").font(.system(size: 13)).foregroundColor(Theme.statusWarning)
        }
    }

    private var currentLocation: String? {
        let region = (computer.region ?? "").lowercased()
        return Self.locations.first { $0.0 == region || $0.1.lowercased() == region }?.0
    }

    // MARK: paired

    @ViewBuilder private var pairedBody: some View {
        consoleCard {
            Text("Name").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            TextField("Name", text: $name)
                .textFieldStyle(.roundedBorder)
                .onAppear { if name.isEmpty { name = computer.name } }
            ACIPillButton(title: "Save") {
                Task { await perform("Renamed.") { try await ACIComputersClient.shared.renamePaired(id: computer.id, name: name.trimmingCharacters(in: .whitespaces)) } }
            }
            .disabled(busy || name.trimmingCharacters(in: .whitespaces).isEmpty || name == computer.name)
        }
        consoleCard {
            Text("Remove").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text("Unpair to take it off your account and away from every agent. The computer itself keeps running at \(computer.provider).")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            ACIHoldButton(title: "Hold to unpair", danger: true, disabled: busy) {
                Task {
                    await perform("Unpaired.") { try await ACIComputersClient.shared.removePaired(id: computer.id) }
                    if !isError { dismiss() }
                }
            }
        }
    }

    private func perform(_ success: String, _ work: () async throws -> Void) async {
        busy = true; defer { busy = false }
        do { try await work(); message = success; isError = false }
        catch { message = consoleFailureText(error); isError = true }
    }
}
