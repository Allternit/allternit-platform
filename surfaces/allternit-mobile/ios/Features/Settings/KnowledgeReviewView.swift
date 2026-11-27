import SwiftUI

// MARK: - Rules

/// Plain rules for the Knowledge review screen (kept out of the views).
enum KnowledgeReview {
    static let emptyLine = "Nothing to review. Agents flag missing knowledge while they work; the curator turns flags into proposals overnight."
    static let skillApproveNote = "Approving a skill turns it on the next time Gizzi runs on your computer. Gizzi asks first."
    static let staleLine = "That item changed since you loaded this screen. It has been refreshed."

    static func isStale(_ error: Error) -> Bool {
        if case .httpError(let status, _)? = error as? APIError { return status == 409 }
        return false
    }

    /// A line diff of `base` against `body`: `+` added, `-` removed, ` ` kept.
    /// Longest-common-subsequence over lines; fine for guide-sized text.
    static func lineDiff(base: String, body: String) -> [DiffLine] {
        let a = base.components(separatedBy: "\n")
        let b = body.components(separatedBy: "\n")
        var lcs = Array(repeating: Array(repeating: 0, count: b.count + 1), count: a.count + 1)
        for i in stride(from: a.count - 1, through: 0, by: -1) {
            for j in stride(from: b.count - 1, through: 0, by: -1) {
                lcs[i][j] = a[i] == b[j] ? lcs[i + 1][j + 1] + 1 : max(lcs[i + 1][j], lcs[i][j + 1])
            }
        }
        var out: [DiffLine] = []
        var i = 0, j = 0
        while i < a.count && j < b.count {
            if a[i] == b[j] {
                out.append(DiffLine(op: .same, text: a[i])); i += 1; j += 1
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                out.append(DiffLine(op: .removed, text: a[i])); i += 1
            } else {
                out.append(DiffLine(op: .added, text: b[j])); j += 1
            }
        }
        while i < a.count { out.append(DiffLine(op: .removed, text: a[i])); i += 1 }
        while j < b.count { out.append(DiffLine(op: .added, text: b[j])); j += 1 }
        return out
    }

    struct DiffLine: Identifiable {
        enum Op { case same, added, removed }
        let id = UUID()
        let op: Op
        let text: String
    }
}

// MARK: - Settings row

/// The "Knowledge" row in Settings → Memory: a count of open flags plus
/// proposals waiting for review, pushing `KnowledgeReviewView`.
struct MemoryKnowledgeRow: View {
    let refreshToken: Int
    @State private var count: Int? = nil

    private let client = MemoryClient()

    var body: some View {
        NavigationLink {
            KnowledgeReviewView()
        } label: {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Color("AccentPrimary").opacity(0.12))
                    Image(systemName: "books.vertical")
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                }
                .frame(width: 40, height: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Knowledge")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Review what agents want to learn")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 8)
                if let count, count > 0 {
                    Text("\(count)")
                        .font(.caption.weight(.semibold))
                        .foregroundColor(.white)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Color("AccentPrimary")))
                        .accessibilityLabel("\(count) to review")
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .simultaneousGesture(TapGesture().onEnded { Haptics.light() })
        .accessibilityIdentifier("memoryKnowledgeRow")
        .task(id: refreshToken) {
            // The badge is a convenience: a failed count shows no badge.
            async let flags = try? client.knowledgeFlags()
            async let proposals = try? client.knowledgeProposals()
            let f = await flags, p = await proposals
            count = (f == nil && p == nil) ? nil : (f?.count ?? 0) + (p?.count ?? 0)
        }
    }
}

// MARK: - Review screen

/// Settings → Memory → Knowledge: proposals waiting for review and flags
/// agents raised. Approve and reject act on the cloud record only.
struct KnowledgeReviewView: View {
    @State private var proposals: [KnowledgeProposal] = []
    @State private var flags: [KnowledgeFlag] = []
    @State private var isLoading = false
    @State private var loaded = false
    @State private var loadError: String? = nil
    @State private var notice: String? = nil
    @State private var busyFlagId: String? = nil

    private let client = MemoryClient()

    var body: some View {
        List {
            if let notice {
                Section {
                    Text(notice)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            if isLoading && !loaded && loadError == nil {
                HStack { Spacer(); ProgressView(); Spacer() }
                    .listRowBackground(Color.clear)
            } else if let loadError, !loaded {
                FriendlyStateView(
                    style: .offline,
                    icon: "books.vertical",
                    title: "Couldn't load knowledge",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { Task { await load() } }
                )
                .frame(maxWidth: .infinity)
                .listRowBackground(Color.clear)
                .accessibilityIdentifier("knowledgeLoadError")
            } else if proposals.isEmpty && flags.isEmpty {
                Text(KnowledgeReview.emptyLine)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .listRowBackground(Color.clear)
                    .accessibilityIdentifier("knowledgeEmpty")
            } else {
                if let loadError {
                    Section {
                        FriendlyInlineStateView(
                            style: .error,
                            icon: "exclamationmark.triangle",
                            title: "Refresh failed",
                            message: FriendlyErrorMessage.from(loadError),
                            actionTitle: "Retry",
                            action: { Task { await load() } }
                        )
                    }
                }
                if !proposals.isEmpty {
                    Section("Waiting for review") {
                        ForEach(proposals) { proposal in
                            NavigationLink {
                                KnowledgeProposalDetailView(proposal: proposal, client: client) { message in
                                    notice = message
                                    Task { await load() }
                                }
                            } label: { proposalRow(proposal) }
                        }
                    }
                }
                if !flags.isEmpty {
                    Section("Flagged by agents") {
                        ForEach(flags) { flag in flagRow(flag) }
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Knowledge")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task { await load() }
    }

    private func proposalRow(_ p: KnowledgeProposal) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(p.title)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
            Text("\(KnowledgeTargetKind.label(p.targetKind)) · \(p.targetRef)")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
            Text(p.evidence.isEmpty ? "No evidence listed" : "Based on \(p.evidence.count) \(p.evidence.count == 1 ? "item" : "items")")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    private func flagRow(_ f: KnowledgeFlag) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(f.kindLabel) · \(KnowledgeTargetKind.label(f.targetKind))\(f.targetRef.map { " · \($0)" } ?? "")")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text(f.evidence)
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
            if let fix = f.suggestedFix, !fix.isEmpty {
                Text("Suggested fix: \(fix)")
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
            }
            Button {
                Haptics.light()
                Task { await dismiss(f) }
            } label: {
                if busyFlagId == f.id { ProgressView() } else { Text("Dismiss") }
            }
            .buttonStyle(.bordered)
            .disabled(busyFlagId != nil)
            .accessibilityIdentifier("knowledgeDismiss")
        }
        .padding(.vertical, 4)
    }

    private func dismiss(_ flag: KnowledgeFlag) async {
        busyFlagId = flag.id
        defer { busyFlagId = nil }
        do {
            try await client.dismissFlag(id: flag.id)
            Haptics.success()
            notice = nil
        } catch {
            Haptics.error()
            notice = KnowledgeReview.isStale(error) ? KnowledgeReview.staleLine : FriendlyErrorMessage.from(error.localizedDescription)
        }
        await load()
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            async let p = client.knowledgeProposals()
            async let f = client.knowledgeFlags()
            let (np, nf) = try await (p, f)
            proposals = np
            flags = nf
            loadError = nil
            loaded = true
        } catch is CancellationError {
            return
        } catch {
            loadError = error.localizedDescription
        }
    }
}

// MARK: - Proposal detail

/// One proposal: body, a line diff against `base`, evidence, and the
/// Approve and Reject buttons.
struct KnowledgeProposalDetailView: View {
    let proposal: KnowledgeProposal
    let client: MemoryClient
    /// Called after a decision (or a 409) with a line to show on the list.
    let onDecided: (String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var confirmsApprove = false
    @State private var isBusy = false
    @State private var errorText: String? = nil

    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 4) {
                    Text(proposal.title)
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                    Text("\(KnowledgeTargetKind.label(proposal.targetKind)) · \(proposal.targetRef)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            if let base = proposal.base, !base.isEmpty {
                Section("What changes") {
                    ForEach(KnowledgeReview.lineDiff(base: base, body: proposal.body)) { line in
                        diffRow(line)
                    }
                }
            } else {
                Section("Proposed text") {
                    Text(proposal.body)
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                }
            }
            if !proposal.evidence.isEmpty {
                Section("Evidence") {
                    ForEach(Array(proposal.evidence.enumerated()), id: \.offset) { _, item in
                        Text(item.text ?? (item.flagId != nil ? "An agent flag" : "A past chat"))
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            Section {
                if let errorText {
                    Text(errorText)
                        .font(.footnote)
                        .foregroundColor(.red)
                }
                Button {
                    Haptics.medium()
                    confirmsApprove = true
                } label: {
                    Text("Approve").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .tint(Color("AccentPrimary"))
                .disabled(isBusy)
                .accessibilityIdentifier("knowledgeApprove")
                if proposal.targetKind == "skill" {
                    Text(KnowledgeReview.skillApproveNote)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                Button(role: .destructive) {
                    Haptics.light()
                    Task { await decide(approve: false) }
                } label: {
                    Text("Reject").frame(maxWidth: .infinity)
                }
                .buttonStyle(.bordered)
                .disabled(isBusy)
                .accessibilityIdentifier("knowledgeReject")
            }
            .listRowBackground(Color.clear)
        }
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Review")
        .navigationBarTitleDisplayMode(.inline)
        .confirmationDialog("Approve this change?", isPresented: $confirmsApprove, titleVisibility: .visible) {
            Button("Approve") { Task { await decide(approve: true) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(proposal.targetKind == "skill"
                 ? KnowledgeReview.skillApproveNote
                 : "It is saved to your knowledge files.")
        }
    }

    private func diffRow(_ line: KnowledgeReview.DiffLine) -> some View {
        let prefix: String, tint: Color
        switch line.op {
        case .added: prefix = "+ "; tint = .green
        case .removed: prefix = "- "; tint = .red
        case .same: prefix = "  "; tint = .clear
        }
        return Text(prefix + line.text)
            .font(.system(.footnote, design: .monospaced))
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity, alignment: .leading)
            .listRowBackground(tint.opacity(0.12))
    }

    private func decide(approve: Bool) async {
        isBusy = true
        errorText = nil
        defer { isBusy = false }
        do {
            if approve {
                try await client.approveProposal(id: proposal.id)
            } else {
                try await client.rejectProposal(id: proposal.id)
            }
            Haptics.success()
            onDecided(nil)
            dismiss()
        } catch {
            Haptics.error()
            if KnowledgeReview.isStale(error) {
                onDecided(KnowledgeReview.staleLine)
                dismiss()
            } else {
                errorText = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}
