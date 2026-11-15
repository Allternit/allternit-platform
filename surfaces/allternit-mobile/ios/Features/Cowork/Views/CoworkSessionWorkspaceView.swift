import SwiftUI

/// Active Cowork session workspace.
///
/// Hosts the shared chat feed + composer for a specific Cowork session and
/// overlays a slide-out progress panel. Uses a local `AppModeStore` pinned to
/// `.cowork` so the workspace chrome/composer behave like the web CoworkRoot
/// without mutating the global tab surface underneath.
struct CoworkSessionWorkspaceView: View {
    let sessionId: String

    @Environment(\.dismiss) private var dismiss
    @StateObject private var viewModel = ChatViewModel()
    @StateObject private var modeStore = AppModeStore()
    @StateObject private var agentModeStore = AgentModeStore()
    @StateObject private var sessionStore = CoworkSessionStore.shared

    @State private var isProgressPanelVisible = false
    @State private var isTasksSheetPresented = false
    @State private var sessionTitle: String = "Cowork Session"

    var body: some View {
        ZStack {
            Color("BgSecondary")
                .edgesIgnoringSafeArea(.all)

            VStack(spacing: 0) {
                toolbar
                Divider().background(Color("BorderSubtle"))

                ChatContentView(
                    sessionId: sessionId,
                    viewModel: viewModel
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }

            // Slide-out progress rail.
            if isProgressPanelVisible {
                HStack {
                    Spacer()
                    CoworkProgressPanel(messages: viewModel.messages) {
                        isTasksSheetPresented = true
                    }
                    .frame(width: 320)
                    .transition(.move(edge: .trailing))
                }
                .edgesIgnoringSafeArea(.bottom)
            }
        }
        .environmentObject(modeStore)
        .environmentObject(agentModeStore)
        .sheet(isPresented: $isTasksSheetPresented) {
            CoworkTasksListView()
        }
        .task {
            // Pin workspace to Cowork mode so ChatContentView renders the
            // cowork top deck, placeholder, and stamps origin_surface=cowork.
            modeStore.mode = .cowork
            viewModel.sessionContext.originSurface = "cowork"
            viewModel.loadSession(sessionId)

            if let session = sessionStore.sessions.first(where: { $0.id == sessionId }) {
                sessionTitle = session.displayTitle
            }
        }
        .onChange(of: sessionStore.sessions) { _ in
            if let session = sessionStore.sessions.first(where: { $0.id == sessionId }) {
                sessionTitle = session.displayTitle
            }
        }
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 12) {
            Button(action: { dismiss() }) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Close")

            VStack(alignment: .leading, spacing: 2) {
                Text(sessionTitle)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)

                Text("Cowork")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            Button(action: { withAnimation { isProgressPanelVisible.toggle() } }) {
                Image(systemName: isProgressPanelVisible ? "sidebar.right" : "sidebar.right.fill")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color("BgPrimary"))
    }
}
