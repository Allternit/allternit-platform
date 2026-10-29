import SwiftUI
import UIKit

/// ACI › Computers, as the web phone draws it (AciScreen.tsx `ComputersPane`):
/// "Computers" header, Add a computer (pairing steps sheet), View a
/// computer's screen, the paired computers (MachinesPanel `list`), linked
/// phones, cloud computer backups, workflows and operator keys.
struct ACIComputersPane: View {
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @State private var isPairOpen = false
    @State private var isScreenPickerOpen = false
    @State private var openComputer: ComputerRecord? = nil
    @State private var toast: ACIToast.Message? = nil

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
                machines
                ACILinkedPhonesSection()
                    .padding(.top, 24)
                ACICloudBackupsSection()
                ACIWorkflowsSection(runtime: workflowRuntime)
                    .padding(.top, 24)
                ACIOperatorKeysSection(notify: { toast = $0 })
                    .padding(.top, 24)
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 32)
        }
        .refreshable { await relay.refresh() }
        .task { await relay.refresh() }
        .overlay(alignment: .bottom) { ACIToast(message: $toast) }
        .sheet(isPresented: $isPairOpen) { ACIPairComputerSheet() }
        .sheet(isPresented: $isScreenPickerOpen) { ACIComputerScreenSheet() }
        .fullScreenCover(item: $openComputer) { computer in
            ACIComputerDesktopCover(computer: computer, backTitle: "Computers")
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
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            Task { await openDesktop(device) }
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
        .accessibilityLabel("Open desktop for \(device.displayName)")
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
                    Text(phone.isAppPhone ? "Screen only" : phone.online ? "Online" : "Desktop offline")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                }
                if phone.isAppPhone && !phone.isLive {
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
            if !phone.isIphone {
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
                    Text(error).foregroundColor(Theme.statusError)
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

// MARK: - Operator keys

/// FabricOperatorKeys: mint, copy (once) and revoke the `alt_` keys bots use
/// to sign in as an operator.
struct ACIOperatorKeysSection: View {
    let notify: (ACIToast.Message) -> Void

    @State private var keys: [ACIComputersClient.OperatorKey] = []
    @State private var loading = true
    @State private var minting = false
    @State private var name = "Grokbot"
    /// Off by default: a key that can run models spends the owner's credits.
    @State private var canRunModels = false
    @State private var revealed: ACIComputersClient.OperatorKey? = nil
    @State private var revokingId: String? = nil
    @State private var revokedIds: Set<String> = []
    @State private var confirmRevoke: ACIComputersClient.OperatorKey? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Operator keys")
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            (Text("Bots such as Grokbot sign in as an operator with an ")
             + Text("alt_").font(.system(size: 13, design: .monospaced))
             + Text(" key — they are not a node. Use ")
             + Text("Authorization: Bearer alt_…").font(.system(size: 13, design: .monospaced))
             + Text(" against the same APIs as this page. The token is shown once."))
                .font(.system(size: 13))
                .lineSpacing(3)
                .foregroundColor(Color("TextSecondary"))

            if let token = revealed?.token {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Copy this token now")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    HStack(spacing: 8) {
                        Text(token)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button {
                            UIPasteboard.general.string = token
                            notify(.init(kind: .success, title: "Copied"))
                        } label: {
                            Label("Copy", systemImage: "doc.on.doc")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 10)
                                .frame(height: 32)
                                .background(Color("FillNeutral"))
                                .clipShape(RoundedRectangle(cornerRadius: 8))
                                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).stroke(Theme.statusWarning, lineWidth: 1))
            }

            TextField("Key name", text: $name)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 10)
                .frame(height: 36)
                .frame(maxWidth: 220)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))

            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { modelsToggle; mintButton }
                VStack(alignment: .leading, spacing: 8) { modelsToggle; mintButton }
            }

            if loading {
                Text("Loading keys…")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
            } else if keys.isEmpty {
                ACIDashedCard(radius: 12) {
                    Text("No operator keys yet. Mint one for Grokbot or any other bot that should drive your nodes.")
                        .font(.system(size: 13))
                        .lineSpacing(3)
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 20)
                }
            } else {
                VStack(spacing: 8) {
                    ForEach(keys) { key in keyRow(key) }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 8)
        .task { await refresh() }
        .confirmationDialog("Revoke this key?", isPresented: Binding(get: { confirmRevoke != nil }, set: { if !$0 { confirmRevoke = nil } }),
                            titleVisibility: .visible, presenting: confirmRevoke) { key in
            Button("Revoke \(key.name)", role: .destructive) { Task { await revoke(key) } }
        }
    }

    private var modelsToggle: some View {
        Button { canRunModels.toggle() } label: {
            HStack(spacing: 6) {
                Image(systemName: canRunModels ? "checkmark.square.fill" : "square")
                    .font(.system(size: 17))
                Text("Can run models (spends your credits)")
                    .font(.system(size: 13, weight: .semibold))
            }
            .foregroundColor(Color("TextPrimary"))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(canRunModels ? .isSelected : [])
    }

    private var mintButton: some View {
        Button { Task { await mint() } } label: {
            Label(minting ? "Minting…" : canRunModels ? "Mint compute + models key" : "Mint compute key", systemImage: "key")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .disabled(minting)
        .opacity(minting ? 0.5 : 1)
    }

    private func keyRow(_ key: ACIComputersClient.OperatorKey) -> some View {
        let revoked = revokedIds.contains(key.id)
        let scopes = (key.scopes ?? []).joined(separator: ", ")
        return HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(key.name)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .strikethrough(revoked)
                    .opacity(revoked ? 0.6 : 1)
                    .lineLimit(1)
                Text("\(key.prefix)… · \(scopes.isEmpty ? "no scopes" : scopes)")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer()
            Button { confirmRevoke = key } label: {
                Label(revoked ? "Revoked" : "Revoke", systemImage: "trash")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(revoked ? Color("TextTertiary") : Theme.statusError)
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
            }
            .buttonStyle(.plain)
            .disabled(revoked || revokingId == key.id)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func refresh() async {
        loading = true
        defer { loading = false }
        do {
            keys = try await ACIComputersClient.shared.listOperatorKeys()
            revokedIds = []
        } catch is CancellationError {
        } catch {
            notify(.init(kind: .error, title: "Could not load operator keys", detail: error.localizedDescription))
        }
    }

    private func mint() async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            notify(.init(kind: .error, title: "Name required", detail: "Give this key a name."))
            return
        }
        minting = true
        defer { minting = false }
        do {
            let created = try await ACIComputersClient.shared.mintOperatorKey(name: trimmed, canRunModels: canRunModels)
            revealed = created
            keys = [created] + keys.filter { $0.id != created.id }
            notify(.init(kind: .success, title: "Operator key minted", detail: "Copy it now — it is shown once."))
        } catch {
            notify(.init(kind: .error, title: "Could not mint key", detail: error.localizedDescription))
        }
    }

    private func revoke(_ key: ACIComputersClient.OperatorKey) async {
        revokingId = key.id
        defer { revokingId = nil }
        do {
            try await ACIComputersClient.shared.revokeOperatorKey(id: key.id)
            // The row stays, marked Revoked, until the list next loads.
            revokedIds.insert(key.id)
            if revealed?.id == key.id { revealed = nil }
            notify(.init(kind: .info, title: "Key revoked"))
        } catch {
            notify(.init(kind: .error, title: "Could not revoke key", detail: error.localizedDescription))
        }
    }
}
