import SwiftUI

/// "Used memory (n)" under an assistant reply: the memories the reply
/// recalled, saved, updated or deleted (`metadata.memoryRefs`). Tapping it
/// lists them, with a link to Settings → Memory.
struct MemoryRefsChip: View {
    let refs: [MemoryRef]

    @State private var showsList = false

    var body: some View {
        Button { showsList = true } label: {
            HStack(spacing: 5) {
                Image(systemName: "brain.head.profile")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Used memory (\(refs.count))")
            }
            .font(.system(size: 12, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 9)
            .padding(.vertical, 4)
            .background(Capsule().fill(Color("FillNeutral")))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Used memory, \(refs.count) \(refs.count == 1 ? "entry" : "entries")")
        .accessibilityHint("Shows the memories this reply used")
        .sheet(isPresented: $showsList) {
            MemoryRefsSheet(refs: refs)
                .presentationDetents([.medium, .large])
        }
    }
}

private struct MemoryRefsSheet: View {
    let refs: [MemoryRef]
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(Array(refs.enumerated()), id: \.offset) { _, ref in
                        row(ref)
                    }
                }
                Section {
                    NavigationLink {
                        MemorySettingsView()
                            .environmentObject(AuthManager.shared)
                    } label: {
                        Label("Manage memory", systemImage: "gearshape")
                    }
                }
            }
            .navigationTitle("Memory used")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private func row(_ ref: MemoryRef) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol(ref.kind))
                .font(.body)
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(ref.title?.isEmpty == false ? ref.title! : "Memory")
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text(verb(ref.kind))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 0)
        }
        .accessibilityElement(children: .combine)
    }

    private func symbol(_ action: MemoryRef.Action) -> String {
        switch action {
        case .recalled: return "brain.head.profile"
        case .saved: return "plus.circle"
        case .updated: return "pencil.circle"
        case .deleted: return "trash.circle"
        }
    }

    private func verb(_ action: MemoryRef.Action) -> String {
        switch action {
        case .recalled: return "Recalled"
        case .saved: return "Saved"
        case .updated: return "Updated"
        case .deleted: return "Deleted"
        }
    }
}
