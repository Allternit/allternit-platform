import SwiftUI

/// The artifact viewer's Comments sheet (phone calm I12; web
/// collab/CommentsPanel.tsx). Lists threads (first comment + replies), adds
/// a comment or reply when the caller can comment, resolves / reopens
/// (author or edit), edits your own text, deletes (author or owner). Every
/// action is a visible button; `ArtifactCommentRules` decides which show.
struct ArtifactCommentsSheet: View {
    let artifactId: String
    /// The artifact's `my_access`.
    let myAccess: String
    /// Called with the open-thread count after every load, for the badge.
    var onCountChange: ((Int) -> Void)? = nil

    @Environment(\.dismiss) private var dismiss
    @ReducedMotion private var reducedMotion

    @State private var comments: [ArtifactComment]?
    @State private var loadError: String?
    @State private var actionError: String?
    @State private var draft = ""
    @State private var replyTo: String?
    @State private var replyText = ""
    @State private var editing: String?
    @State private var editText = ""
    @State private var busy = false
    @State private var showResolved = false
    @State private var confirmDelete: ArtifactComment?
    @FocusState private var focus: Field?

    private enum Field: Hashable { case draft, reply, edit }

    private var access: ArtifactAccessLevel { ArtifactAccessLevel(myAccess: myAccess) }
    private var me: String? { AuthManager.shared.currentUserId }
    private var mayComment: Bool { ArtifactCommentRules.canComment(access: access) }
    private var threads: [ArtifactCommentThread] { ArtifactCommentRules.threads(comments ?? []) }
    private var listAnimation: Animation { Motion.pick(Motion.deck, reduced: reducedMotion) }
    private var rowTransition: AnyTransition {
        reducedMotion ? .opacity : .opacity.combined(with: .move(edge: .bottom))
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                content
                if let actionError {
                    Text(actionError)
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
                        .padding(.horizontal, 12)
                        .padding(.bottom, 6)
                        .transition(.opacity)
                }
                if mayComment, comments != nil { composer }
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Comments")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityLabel("Close comments")
                }
                if ArtifactCommentRules.resolvedCount(threads) > 0 {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button(showResolved ? "Hide resolved" : "Resolved (\(ArtifactCommentRules.resolvedCount(threads)))") {
                            Haptics.selection()
                            withAnimation(listAnimation) { showResolved.toggle() }
                        }
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    }
                }
            }
            .confirmationDialog(
                confirmDelete?.isRoot == true ? "Delete this thread?" : "Delete this comment?",
                isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } }),
                titleVisibility: .visible
            ) {
                Button("Delete", role: .destructive) {
                    if let comment = confirmDelete { Task { await remove(comment) } }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                if confirmDelete?.isRoot == true { Text("Its replies are deleted too.") }
            }
        }
        .task { await load() }
    }

    // MARK: - States

    @ViewBuilder
    private var content: some View {
        if let loadError, comments == nil {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load comments",
                message: loadError,
                actionTitle: "Retry",
                action: { Task { await load() } }
            )
            .frame(maxHeight: .infinity)
        } else if comments == nil {
            ProgressView("Loading comments…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if ArtifactCommentRules.visibleThreads(threads, showResolved: showResolved).isEmpty {
            FriendlyStateView(
                title: threads.isEmpty ? "No comments yet" : "No open comments",
                message: mayComment ? "" : "Only people who can comment can add one.",
                art: .envelope,
                size: .panel
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(ArtifactCommentRules.visibleThreads(threads, showResolved: showResolved)) { thread in
                        threadCard(thread)
                            .transition(rowTransition)
                    }
                }
                .padding(16)
                .animation(listAnimation, value: comments)
            }
        }
    }

    // MARK: - Thread

    private func threadCard(_ thread: ArtifactCommentThread) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            commentRow(thread.root)
            ForEach(thread.replies) { reply in
                commentRow(reply)
                    .padding(.leading, 14)
                    .overlay(alignment: .leading) {
                        Rectangle().fill(Color("BorderSubtle")).frame(width: 2)
                    }
            }
            if replyTo == thread.id {
                HStack(spacing: 8) {
                    TextField("Reply…", text: $replyText, axis: .vertical)
                        .lineLimit(1...4)
                        .focused($focus, equals: .reply)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                    Button("Send") { Task { await post(replyText, parent: thread.id) } }
                        .font(.subheadline.weight(.semibold))
                        .disabled(ArtifactCommentRules.postableBody(replyText) == nil || busy)
                }
            }
            HStack(spacing: 16) {
                if mayComment, replyTo != thread.id {
                    Button("Reply") {
                        Haptics.light()
                        replyTo = thread.id
                        replyText = ""
                        focus = .reply
                    }
                }
                Spacer(minLength: 0)
                if ArtifactCommentRules.canResolve(thread.root, me: me, access: access) {
                    Button {
                        Task { await resolve(thread.root) }
                    } label: {
                        Label(thread.isOpen ? "Resolve" : "Reopen", systemImage: thread.isOpen ? "checkmark" : "arrow.uturn.backward")
                    }
                    .disabled(busy)
                }
            }
            .font(.footnote.weight(.medium))
            .foregroundColor(Color("TextSecondary"))
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .opacity(thread.isOpen ? 1 : 0.6)
    }

    private func commentRow(_ comment: ArtifactComment) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                if comment.authorId == ArtifactCommentRules.assistantId {
                    Image(systemName: "sparkle").font(.caption2)
                }
                Text(ArtifactCommentRules.authorLabel(comment, me: me))
                    .font(.footnote.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                if let date = comment.createdDate {
                    Text(date, format: .relative(presentation: .named, unitsStyle: .abbreviated))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 0)
                if ArtifactCommentRules.canEditBody(comment, me: me), editing != comment.id {
                    Button {
                        Haptics.light()
                        editing = comment.id
                        editText = comment.body
                        focus = .edit
                    } label: { Image(systemName: "pencil") }
                    .accessibilityLabel("Edit comment")
                }
                if ArtifactCommentRules.canDelete(comment, me: me, access: access) {
                    Button { confirmDelete = comment } label: { Image(systemName: "trash") }
                        .accessibilityLabel("Delete comment")
                }
            }
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
            .buttonStyle(.plain)

            if editing == comment.id {
                TextField("Comment", text: $editText, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($focus, equals: .edit)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                HStack(spacing: 16) {
                    Spacer()
                    Button("Cancel") { editing = nil }
                    Button("Save") { Task { await saveEdit(comment) } }
                        .fontWeight(.semibold)
                        .disabled(ArtifactCommentRules.postableBody(editText) == nil || busy)
                }
                .font(.footnote)
                .buttonStyle(.plain)
                .foregroundColor(Color("TextPrimary"))
            } else {
                Text(comment.body)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    // MARK: - Composer

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(access >= .edit ? "Add a comment, or @gizzi to ask for a change" : "Add a comment", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .focused($focus, equals: .draft)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("FillNeutral")))
                .accessibilityLabel("New comment")
            Button {
                Task { await post(draft, parent: nil) }
            } label: {
                Image(systemName: busy ? "ellipsis" : "arrow.up")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("BgPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color("TextPrimary")))
            }
            .buttonStyle(.plain)
            .disabled(ArtifactCommentRules.postableBody(draft) == nil || busy)
            .opacity(ArtifactCommentRules.postableBody(draft) == nil ? 0.4 : 1)
            .accessibilityLabel("Comment")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Color("BgPrimary"))
        .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 0.5) }
    }

    // MARK: - Actions

    private func load() async {
        do {
            let items = try await ArtifactsV2Client.shared.comments(id: artifactId)
            withAnimation(listAnimation) { comments = items }
            loadError = nil
            onCountChange?(ArtifactCommentRules.openThreadCount(items))
        } catch is CancellationError {
            return
        } catch {
            loadError = plainErrorText(error)
        }
    }

    private func post(_ raw: String, parent: String?) async {
        guard let text = ArtifactCommentRules.postableBody(raw), !busy else { return }
        busy = true
        defer { busy = false }
        do {
            _ = try await ArtifactsV2Client.shared.addComment(id: artifactId, body: text, parentId: parent)
            Haptics.success()
            if parent == nil { draft = "" } else { replyTo = nil; replyText = "" }
            actionError = nil
            await load()
        } catch {
            actionError = plainErrorText(error)
            Haptics.warning()
        }
    }

    private func resolve(_ root: ArtifactComment) async {
        busy = true
        defer { busy = false }
        do {
            _ = try await ArtifactsV2Client.shared.resolveComment(id: artifactId, commentId: root.id, resolved: !root.isResolved)
            Haptics.selection()
            actionError = nil
            await load()
        } catch {
            actionError = plainErrorText(error)
        }
    }

    private func saveEdit(_ comment: ArtifactComment) async {
        guard let text = ArtifactCommentRules.postableBody(editText) else { return }
        busy = true
        defer { busy = false }
        do {
            _ = try await ArtifactsV2Client.shared.editComment(id: artifactId, commentId: comment.id, body: text)
            editing = nil
            actionError = nil
            await load()
        } catch {
            actionError = plainErrorText(error)
        }
    }

    private func remove(_ comment: ArtifactComment) async {
        busy = true
        defer { busy = false }
        do {
            try await ArtifactsV2Client.shared.deleteComment(id: artifactId, commentId: comment.id)
            actionError = nil
            await load()
        } catch {
            actionError = plainErrorText(error)
        }
    }
}
