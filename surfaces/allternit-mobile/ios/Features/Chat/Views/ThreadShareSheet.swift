import SwiftUI
import UIKit

// MARK: - Share link sheet
//
// "Share link…" from the thread header or a reply's action bar. Creates (or
// finds) the thread's share link, then offers Share, Update link (re-snapshot
// the chat now) and Stop sharing (confirmed).

@MainActor
final class ThreadShareModel: ObservableObject {
    enum Phase: Equatable { case loading, ready(ThreadShareEntry), failed(ThreadShareError) }

    @Published var phase: Phase = .loading
    @Published var busy = false
    @Published var notice: String? = nil
    @Published var stopped = false

    let sessionId: String?
    let title: String
    let messages: [MessageRecord]
    private let client = ThreadShareClient.shared

    init(sessionId: String?, title: String, messages: [MessageRecord]) {
        self.sessionId = sessionId
        self.title = title
        self.messages = messages
    }

    private func snapshot() -> [ThreadShareSnapshotItem] { ThreadShareClient.buildSnapshot(from: messages) }

    private func fail(_ error: Error) -> ThreadShareError {
        (error as? ThreadShareError) ?? .other(error.localizedDescription)
    }

    func start() async {
        guard let sessionId else { phase = .failed(.empty); return }
        phase = .loading
        do {
            if let existing = try await client.lookup(sessionId: sessionId) {
                phase = .ready(existing)
            } else {
                let snap = snapshot()
                if snap.isEmpty { phase = .failed(.empty); return }
                phase = .ready(try await client.createOrUpdate(sessionId: sessionId, title: title, snapshot: snap))
            }
        } catch { phase = .failed(fail(error)) }
    }

    func refresh(_ entry: ThreadShareEntry) async {
        let snap = snapshot()
        if snap.isEmpty { notice = ThreadShareError.empty.errorDescription; return }
        busy = true; defer { busy = false }
        do {
            let updated = try await client.update(token: entry.token, title: title, snapshot: snap)
            phase = .ready(updated)
            notice = "Link updated with the latest messages."
        } catch { notice = fail(error).errorDescription }
    }

    func stopSharing(_ entry: ThreadShareEntry) async {
        busy = true; defer { busy = false }
        do {
            try await client.delete(token: entry.token)
            stopped = true
        } catch { notice = fail(error).errorDescription }
    }
}

struct ThreadShareSheet: View {
    @StateObject private var model: ThreadShareModel
    @Environment(\.dismiss) private var dismiss
    @State private var confirmStop = false
    @State private var showActivity = false

    init(sessionId: String?, title: String, messages: [MessageRecord]) {
        _model = StateObject(wrappedValue: ThreadShareModel(sessionId: sessionId, title: title, messages: messages))
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 16) {
                content
                Spacer(minLength: 0)
            }
            .padding(20)
            .background(Color("BgPrimary"))
            .navigationTitle("Share link")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium])
        .task { await model.start() }
        .onChange(of: model.stopped) { stopped in if stopped { dismiss() } }
    }

    @ViewBuilder private var content: some View {
        switch model.phase {
        case .loading:
            HStack(spacing: 10) { ProgressView(); Text("Creating link…").foregroundColor(Color("TextSecondary")) }
        case .failed(let error):
            VStack(alignment: .leading, spacing: 12) {
                Label(error.errorDescription ?? "Something went wrong.", systemImage: "exclamationmark.triangle")
                    .foregroundColor(Color("TextPrimary"))
                if error != .signedOut && error != .empty && error != .tooLarge {
                    Button("Try again") { Task { await model.start() } }.buttonStyle(.borderedProminent)
                }
            }
        case .ready(let entry):
            VStack(alignment: .leading, spacing: 14) {
                Text(entry.url)
                    .font(.system(size: 14, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .textSelection(.enabled)
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 10).fill(Color("TextSecondary").opacity(0.08)))
                Text("Anyone with this link can see a snapshot of this chat: the messages and the names of tools used, as of the last update. New messages are not added until you update the link.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                Button { showActivity = true } label: {
                    Label("Share link", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                HStack {
                    Button { Task { await model.refresh(entry) } } label: {
                        Label("Update link", systemImage: "arrow.triangle.2.circlepath")
                    }
                    Spacer()
                    Button(role: .destructive) { confirmStop = true } label: {
                        Label("Stop sharing", systemImage: "link.badge.plus").labelStyle(.titleOnly)
                    }
                }
                .disabled(model.busy)
                if let notice = model.notice {
                    Text(notice).font(.footnote).foregroundColor(Color("TextSecondary"))
                }
            }
            .sheet(isPresented: $showActivity) { ThreadShareActivityView(items: [entry.url]) }
            .confirmationDialog("Stop sharing this chat?", isPresented: $confirmStop, titleVisibility: .visible) {
                Button("Stop sharing", role: .destructive) { Task { await model.stopSharing(entry) } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("The link will stop working for everyone who has it.")
            }
        }
    }
}

private struct ThreadShareActivityView: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}
