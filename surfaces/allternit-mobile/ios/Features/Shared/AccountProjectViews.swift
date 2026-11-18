import SwiftUI

/// One neutral line for a screen whose computer parts are empty because
/// there is no computer: "<text> · Connect". Connect opens the connect kit
/// (the same sheet as the composer's computer chip). Never red.
struct NeedsComputerLine: View {
    let text: String

    @State private var isConnectPresented = false

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            Text(text)
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
            Text("·")
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            Button {
                Haptics.light()
                isConnectPresented = true
            } label: {
                Text("Connect")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 36)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .sheet(isPresented: $isConnectPresented) { ConnectComputerSheet() }
    }
}

/// The Bots home's account bots: bots saved to the account that run once a
/// computer connects, plus the no-computer line and a New bot that saves to
/// the account. Opening one shows the compute gate, as running any bot does
/// with no computer. A real load failure stays a calm grey line.
struct AccountBotsStrip: View {
    /// `AgentHubStore.loadError` (the computer's bot list).
    let hubError: String?
    /// The computer's roster has bots.
    let hasBots: Bool

    @ObservedObject private var library = ProjectsLibraryStore.shared
    @State private var isNewPresented = false
    @State private var newKind: LibraryProject.Mode = .bot
    @State private var createError: String?

    private var noComputer: Bool {
        library.noComputer || ProjectsLibraryStore.isNoComputerMessage(hubError)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(library.accountBots) { bot in row(bot) }
            if noComputer {
                HStack(spacing: 12) {
                    NeedsComputerLine(text: "Bots run when a computer connects")
                    Button {
                        Haptics.light()
                        newKind = .bot
                        isNewPresented = true
                    } label: {
                        Label("New bot", systemImage: "plus")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(minHeight: 36)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .fixedSize()
                }
            } else if let hubError, !hasBots {
                Text("Couldn't load your bots. \(FriendlyErrorMessage.from(hubError))")
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextTertiary"))
            }
            if let createError {
                Text(createError)
                    .font(.system(size: 11.5))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .sheet(isPresented: $isNewPresented) {
            NewProjectSheet(onCreate: create, kind: $newKind)
        }
    }

    private func row(_ bot: AccountProject) -> some View {
        Button {
            Haptics.light()
            if noComputer { ComputeStatusStore.shared.noteComputeRequired() } else { Task { await library.load() } }
        } label: {
            HStack(spacing: 10) {
                Text(String(bot.title.prefix(1)).uppercased())
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color("BgSecondary")))
                    .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                VStack(alignment: .leading, spacing: 1) {
                    Text(bot.title)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(ProjectsLibraryStore.accountCountLine(bot))
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(bot.title), runs when a computer connects")
    }

    private func create(title: String, description: String?) {
        let kind = AccountProject.Kind(rawValue: newKind.rawValue) ?? .bot
        Task {
            do {
                createError = nil
                if kind == .bot {
                    try await library.createAccountProject(kind: .bot, title: title, description: nil, brief: description)
                } else {
                    try await library.createAccountProject(kind: kind, title: title, description: description)
                }
            } catch {
                createError = "Couldn't save the bot. \(FriendlyErrorMessage.from(error))"
            }
        }
    }
}
