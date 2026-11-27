import SwiftUI

/// A chat thread bound to a CLI session on a picked computer (Start anywhere,
/// plan D2 terminal kind): the Code terminal and its agent composer
/// (`PhoneCodeTerminal` → `AgentComposerBar`), attached to the terminal the
/// start opened. Nothing new starts on the computer; a failed attach says so
/// and offers a retry.
struct BoundTerminalThread: View {
    let binding: StartBinding

    @StateObject private var store = CodeTerminalStore()
    /// nil while attaching; then whether the terminal was reached.
    @State private var attached: Bool? = nil

    private var title: String {
        RuntimeRegistryStore.shared.runtime(binding.runtime)?.name ?? binding.runtime.capitalized
    }

    private var folderName: String {
        binding.folder.split(separator: "/").last.map(String.init) ?? binding.folder
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "apple.terminal")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
                Text("\(title) on \(binding.deviceName)")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(folderName)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .frame(minHeight: 36)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("boundTerminal-header")

            switch attached {
            case .none:
                VStack(spacing: 10) {
                    ProgressView()
                    Text("Connecting to \(binding.deviceName)…")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .some(true):
                // Attached first, so the terminal's own first-open does not start a new shell.
                PhoneCodeTerminal(store: store, shellOnly: true)
            case .some(false):
                VStack(spacing: 12) {
                    Text("Couldn't reach \(binding.deviceName). The session keeps running there.")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Try again") {
                        Haptics.light()
                        Task { await attach() }
                    }
                    .font(.system(size: 14, weight: .semibold))
                    .frame(minHeight: 44)
                }
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .accessibilityIdentifier("boundTerminal-error")
            }
        }
        .task(id: binding.terminalId) { await attach() }
    }

    private func attach() async {
        guard let terminalId = binding.terminalId else {
            attached = false
            return
        }
        attached = nil
        RuntimeRelayStore.shared.select(binding.runtimeId)
        attached = await store.attach(sessionId: terminalId, title: title)
    }
}
