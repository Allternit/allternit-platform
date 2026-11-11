import SwiftUI
import UIKit

/// Settings → Phones: "Let your bots use this iPhone". Pick a Desktop, see
/// the same 6-digit code here and on the Desktop, approve there. Once
/// paired: share the screen, see whether bots can reach the app, unpair.
struct AppPhonesSettingsView: View {
    @ObservedObject private var phone = AppPhoneService.shared

    @State private var runtimes: [RuntimeDevice] = []
    @State private var isLoading = false
    @State private var loadError: String?
    @State private var phoneName = UIDevice.current.name
    @State private var isUnpairConfirmPresented = false
    @State private var boardsState: BoardsState = .loading
    @State private var stopping: Set<String> = []
    @State private var stopNotes: [String: String] = [:]

    private enum BoardsState: Equatable {
        case loading
        case loaded([LinkBoard])
        case unreachable
        /// An older runtime answered 404: the section is hidden.
        case unsupported
    }

    var body: some View {
        List {
            if let credentials = phone.credentials {
                pairedSection(credentials)
                boardsSection(credentials)
                screenSection
                unpairSection
            } else {
                switch phone.pairStep {
                case .registering, .waitingForDesktop:
                    pairingSection
                default:
                    introSection
                    desktopsSection
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Phones")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            if phone.credentials == nil { await loadRuntimes() } else { await loadBoards() }
        }
        .refreshable {
            await loadRuntimes()
            if phone.credentials != nil { await loadBoards() }
        }
        .confirmationDialog("Unpair this iPhone?", isPresented: $isUnpairConfirmPresented, titleVisibility: .visible) {
            Button("Unpair", role: .destructive) {
                Task {
                    await phone.unpair()
                    await loadRuntimes()
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Bots on \(phone.credentials?.runtimeName ?? "your Desktop") won't be able to see this screen or use the ACI browser until you pair again.")
        }
    }

    // MARK: - Not paired

    private var introSection: some View {
        Section {
            TextField("Name", text: $phoneName)
                .font(.subheadline)
                .textInputAutocapitalization(.words)
        } header: {
            Text("Let your bots use this iPhone")
        } footer: {
            Text("Pair this iPhone with one of your Desktops. Your bots can then see the screen while you share it and use the ACI browser in this app. Taps on other apps aren't supported.")
        }
    }

    @ViewBuilder
    private var desktopsSection: some View {
        Section {
            if isLoading && runtimes.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading your Desktops…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else if let loadError {
                Text(loadError)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                Button("Try again") { Task { await loadRuntimes() } }
                    .font(.subheadline)
            } else if desktops.isEmpty {
                Text("No Desktop is paired with your account yet. Open Allternit Desktop and sign in, then pull to refresh.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(desktops) { runtime in
                    Button(action: { phone.pair(with: runtime, name: trimmedName) }) {
                        HStack(spacing: 10) {
                            Image(systemName: "desktopcomputer")
                                .font(.subheadline)
                                .frame(width: 20)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(runtime.name)
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextPrimary"))
                                Text(runtime.isOnline ? runtime.typeLabel : "\(runtime.typeLabel) · offline")
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            Spacer()
                            Image(systemName: "chevron.right")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!runtime.isOnline)
                }
            }

            if case .failed(let message) = phone.pairStep {
                Text(message)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        } header: {
            Text("Choose a Desktop")
        } footer: {
            Text("The Desktop must be open. You'll approve this iPhone there.")
        }
    }

    private var pairingSection: some View {
        Section {
            VStack(spacing: 12) {
                if case .waitingForDesktop(let code?) = phone.pairStep {
                    Text(Self.spaced(code))
                        .font(.system(size: 34, weight: .semibold, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityLabel("Code \(code.map(String.init).joined(separator: " "))")
                    Text("Check that your Desktop shows the same code, then approve there.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                } else {
                    ProgressView()
                    Text(phone.pairStep == .registering ? "Connecting to your Desktop…" : "Waiting for your Desktop…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
                Button("Cancel") { phone.cancelPairing() }
                    .font(.subheadline)
                    .foregroundColor(Theme.statusError)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
        } header: {
            Text("Pairing")
        }
    }

    // MARK: - Paired

    private func pairedSection(_ c: AppPhoneCredentials) -> some View {
        Section {
            row("Name", c.name)
            row("Desktop", c.runtimeName)
            HStack {
                Text("Bots can reach this app")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Circle()
                    .fill(phone.isControlConnected ? Theme.statusSuccess : Color("TextSecondary").opacity(0.5))
                    .frame(width: 8, height: 8)
                Text(phone.isControlConnected ? "Connected" : "Connecting…")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
        } header: {
            Text("This iPhone")
        } footer: {
            Text("Bots reach this app only while it's open. They can use the ACI browser here; a banner shows when they do, with Stop.")
        }
    }

    // MARK: - Allternit Link boards

    @ViewBuilder
    private func boardsSection(_ c: AppPhoneCredentials) -> some View {
        switch boardsState {
        case .unsupported:
            EmptyView()
        case .loading:
            Section {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Looking for Allternit Link boards…")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } header: {
                Text("Allternit Link")
            }
        case .unreachable:
            Section {
                Text("Couldn't reach \(c.runtimeName) to check your boards.")
                    .font(.subheadline)
                    .foregroundColor(Theme.statusError)
                Button("Try again") { Task { await loadBoards() } }
                    .font(.subheadline)
            } header: {
                Text("Allternit Link")
            }
        case .loaded(let boards):
            if !boards.isEmpty {
                Section {
                    ForEach(boards) { board in
                        boardRow(board, isThisPhone: board.phone == c.serial)
                    }
                } header: {
                    Text("Allternit Link")
                } footer: {
                    Text("Setup and pairing are done in Allternit Desktop. A board is armed only by pressing its own button; this app can only stop it.")
                }
            }
        }
    }

    private func boardRow(_ board: LinkBoard, isThisPhone: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Circle()
                    .fill(board.online ? Theme.statusSuccess : Color("TextSecondary").opacity(0.5))
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
                Text(board.name)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(board.status.label)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.trailing)
            }
            Text(boardDetail(board, isThisPhone: isThisPhone))
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            if isThisPhone { assistiveSteps }
            if board.armed {
                let busy = stopping.contains(board.boardId)
                Button(action: { Task { await stopBoard(board) } }) {
                    HStack(spacing: 6) {
                        if busy { ProgressView() }
                        Text(busy ? "Stopping…" : "Stop")
                            .font(.subheadline.weight(.semibold))
                    }
                    .frame(maxWidth: .infinity, minHeight: 36)
                }
                .buttonStyle(.bordered)
                .tint(Theme.statusError)
                .disabled(busy)
                .accessibilityLabel("Stop \(board.name)")
            }
            if let note = stopNotes[board.boardId] {
                Text(note)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }

    private func boardDetail(_ board: LinkBoard, isThisPhone: Bool) -> String {
        var parts: [String] = []
        parts.append(board.link?.label ?? "Not linked")
        parts.append(board.armed ? "Armed" : "Not armed")
        if isThisPhone { parts.append("Paired with this iPhone") }
        else if let other = board.phone, !other.isEmpty { parts.append("Paired with another phone") }
        else { parts.append("Not paired with a phone") }
        return parts.joined(separator: " · ")
    }

    /// Only a person can do these; none can be done remotely.
    private var assistiveSteps: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("To use this board with this iPhone, do these yourself:")
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("1. Turn on AssistiveTouch (Settings → Accessibility → Touch).")
            Text("2. Start the screen broadcast below.")
            Text("3. Press the button on the board.")
        }
        .font(.caption)
        .foregroundColor(Color("TextSecondary"))
        .fixedSize(horizontal: false, vertical: true)
    }

    private func loadBoards() async {
        if case .loaded = boardsState {} else { boardsState = .loading }
        switch await LinkBoardsClient.shared.boards() {
        case .boards(let boards): boardsState = .loaded(boards)
        case .unsupported: boardsState = .unsupported
        case .unreachable: boardsState = .unreachable
        }
    }

    private func stopBoard(_ board: LinkBoard) async {
        stopping.insert(board.boardId)
        stopNotes[board.boardId] = nil
        let result = await LinkBoardsClient.shared.stop(boardId: board.boardId)
        stopping.remove(board.boardId)
        switch result {
        case .stopped: break
        case .uncertain:
            stopNotes[board.boardId] = "Couldn't confirm it stopped. Press the button on the board to be sure."
        case .boardOffline:
            stopNotes[board.boardId] = "The board is offline. Press its button if it is still moving."
        case .desktopOffline:
            stopNotes[board.boardId] = "Allternit Desktop isn't reachable. Press the board's button to stop it."
        case .failed(let message):
            stopNotes[board.boardId] = message
        }
        await loadBoards()
    }

    private var screenSection: some View {
        Section {
            ScreenShareButton()
        } header: {
            Text("Screen")
        } footer: {
            Text("Sends this iPhone's screen and app sound to your Desktop until you stop. Nothing is encoded until a bot or person is watching. The microphone is never sent.")
        }
    }

    private var unpairSection: some View {
        Section {
            Button(action: { isUnpairConfirmPresented = true }) {
                HStack(spacing: 10) {
                    Image(systemName: "xmark.circle")
                        .font(.subheadline)
                        .frame(width: 20)
                    Text("Unpair this iPhone")
                        .font(.subheadline)
                    Spacer()
                }
                .foregroundColor(Theme.statusError)
                .contentShape(Rectangle())
            }
        }
    }

    // MARK: - Helpers

    private func row(_ title: String, _ value: String) -> some View {
        HStack {
            Text(title)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
        }
    }

    /// Desktops and VPS runtimes the phone can pair with — not other phones.
    private var desktops: [RuntimeDevice] {
        runtimes.filter { $0.runtimeType != "ios" && $0.runtimeType != "mobile" }
    }

    private var trimmedName: String {
        let name = phoneName.trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? "iPhone" : name
    }

    /// "123456" → "123 456".
    private static func spaced(_ code: String) -> String {
        guard code.count == 6 else { return code }
        return "\(code.prefix(3)) \(code.suffix(3))"
    }

    private func loadRuntimes() async {
        isLoading = true
        defer { isLoading = false }
        do {
            runtimes = try await RuntimeDevicesClient().fetchRuntimes()
            loadError = nil
        } catch APIError.httpError(statusCode: 401, _) {
            loadError = "Sign in with your Allternit account to pair this iPhone."
        } catch {
            loadError = error.localizedDescription
        }
    }
}
