import SwiftUI

/// One design project on the phone: the web's Design canvas
/// (`/editor/design/:projectId`), the page an artifact session's Design pane
/// uses, under a Close that saves first.
struct DesignProjectCanvasView: View {
    let project: WorkspaceLibraryProject
    /// Just made here, so there's no canvas elsewhere to wait for.
    var createdHere = false

    @Environment(\.dismiss) private var dismiss
    @State private var saveHandle = EditorSaveHandle()
    @State private var isSaving = false
    @State private var saveFailed = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Button(action: close) {
                    if isSaving {
                        ProgressView()
                    } else {
                        Text("Close")
                            .font(.system(size: 15, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                    }
                }
                .buttonStyle(.plain)
                .disabled(isSaving)
                .frame(minWidth: 44, minHeight: 44)
                Text(project.title)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            ArtifactLiveWebView(
                route: .design(projectId: project.nativeId, title: project.title, fromProject: !createdHere),
                onClose: { close() },
                saveHandle: saveHandle
            )
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .alert("Couldn't save the canvas", isPresented: $saveFailed) {
            Button("Keep editing", role: .cancel) {}
            Button("Close anyway", role: .destructive) { dismiss() }
        } message: {
            Text("Your latest changes may not be saved. Check the connection and try again.")
        }
    }

    private func close() {
        guard !isSaving else { return }
        isSaving = true
        Task {
            let saved = await saveHandle.save()
            isSaving = false
            if saved { dismiss() } else { saveFailed = true }
        }
    }
}
