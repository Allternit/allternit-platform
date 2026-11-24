import SwiftUI

/// Browser-surface chat (web parity: OperatorBrowserView's BrowserChatPanel
/// wired to a session store stamped `originSurface: 'browser'`).
///
/// A thin wrapper around the shared ChatContentView with its own
/// ChatViewModel — the same pattern CodeThreadChatView uses for code
/// threads. The origin stamp needs NO special casing: ChatContentView's
/// sessionContext derives `originSurface` from `modeStore.mode`, which is
/// `.browser` on this surface, so session creates stamp "browser" and the
/// sidebar's ACI filter files them here.
struct BrowserChatView: View {
    let sessionId: String?
    let onBack: () -> Void

    @StateObject private var viewModel = ChatViewModel()

    private let theme = ModeTheme(mode: .browser)

    var body: some View {
        VStack(spacing: 0) {
            // Header — mirrors the ACI landing header's chrome.
            HStack {
                Button(action: {
                    Haptics.light()
                    onBack()
                }) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 44, height: 44)
                }

                Spacer()

                Text("Browser chat")
                    .font(.system(.headline, design: .serif))
                    .fontWeight(.semibold)
                    .foregroundColor(theme.accent)

                Spacer()

                // Balancer spacer matching the back button's width.
                Spacer().frame(width: 44, height: 44)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(Color("BgPrimary"))

            Divider().background(Color("BorderSubtle"))

            ChatContentView(sessionId: sessionId, viewModel: viewModel)
        }
        .background(Color("BgPrimary"))
    }
}
