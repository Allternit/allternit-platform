import SwiftUI
import AllternitFilesKit

/// Inbox requests: files dropped in a computer's Inbox that wait for the
/// owner (platform #2445, `GET /inbox/tasks?state=needs_approval`). A drop
/// never starts the agent by itself. Approve runs the fixed pipeline (read
/// this one file, summarize with no tools, write the result beside it);
/// Reject ends the task and nothing reads the file.
struct FilesInboxRequestsView: View {
    @ObservedObject var model: FilesComputerModel

    var body: some View {
        List {
            if model.pendingTasks.isEmpty {
                FriendlyStateView(style: .empty, icon: "checkmark.shield", title: "Nothing waits for you",
                                  message: "Files dropped in Inbox show here until you approve or reject them.")
                    .listRowBackground(Color.clear)
            } else {
                Section {
                    ForEach(model.pendingTasks) { task in row(task) }
                } footer: {
                    Text("Your agent reads only the file you approve, with no other tools, and saves its summary next to it.")
                }
                .listRowBackground(Color("FillNeutral"))
            }
        }
        .filesListStyle()
        .navigationTitle("Inbox requests")
        .refreshable { await model.loadTasks() }
        .task { await model.loadTasks() }
    }

    private func row(_ task: FilesInboxTask) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            FilesRowLabel(systemImage: "doc", title: task.name,
                          subtitle: "\(task.droppedByLabel) · \(FilesFormat.date(task.createdAt))")
            HStack(spacing: 10) {
                Button { Task { await model.approve(task) } } label: {
                    Text("Ask my agent to read it")
                        .font(.system(size: 14, weight: .semibold))
                        .frame(maxWidth: .infinity, minHeight: 36)
                        .foregroundColor(Color("BgPrimary"))
                        .background(Color("TextPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
                .buttonStyle(.plain)
                Button { Task { await model.reject(task) } } label: {
                    Text("Reject")
                        .font(.system(size: 14, weight: .semibold))
                        .frame(width: 90, height: 36)
                        .foregroundColor(Color("TextPrimary"))
                        .background(Color("BgPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }
}
