import SwiftUI

/// The `settings` tool's card (Primitives Wave D, D2;
/// contracts/settings/README.md): one row per change.
/// - applied: "Changed", with Undo when the old value is known and this app
///   can write the key.
/// - apply_on_device: this iPhone applies it once per tool-call id while the
///   reply streams here ("Applied on this iPhone", Undo); on a reloaded chat
///   it offers Apply instead. Runtime keys (scope `runtime`, gizzi.*) are
///   Gizzi Code's: the row says so and offers nothing here.
/// - needs_confirmation: Confirm writes it under the person's own sign-in.
/// - locked: Open Settings only.
/// - invalid: the reason.
/// Keys this app has no store or client for fall back to Open Settings.
struct SettingsChangeCardView: View {
    let result: AgentSettingsResult
    let toolCallId: String
    /// True while the reply that made the call streams on this iPhone, so
    /// this is the device that sent the turn.
    let autoApply: Bool

    private enum RowState: Equatable {
        case appliedHere(previous: AgentSettingValue?)
        case confirmed
        case undone
    }

    @State private var rowState: [String: RowState] = [:]
    @State private var busy: Set<String> = []
    @State private var problem: [String: String] = [:]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "gearshape")
                    .foregroundColor(Color("AccentPrimary"))
                    .accessibilityHidden(true)
                Text(result.changes.count == 1 ? "Settings change" : "Settings changes")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))

            VStack(spacing: 0) {
                ForEach(Array(result.changes.enumerated()), id: \.element.id) { index, change in
                    if index > 0 {
                        Divider().background(Color("BorderSubtle"))
                    }
                    row(change)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Settings changes")
        .onAppear(perform: applyDeviceChanges)
    }

    // MARK: Row

    private func row(_ change: AgentSettingsResult.Change) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(change.label)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(statusLabel(change))
                    .font(.caption.weight(.semibold))
                    .foregroundColor(change.status == .invalid ? Theme.statusError : Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Color("FillNeutral")))
            }

            if let line = valueLine(change) {
                Text(line)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let note = note(change) {
                Text(note)
                    .font(.caption)
                    .foregroundColor(change.status == .invalid ? Theme.statusError : Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let message = problem[change.key] {
                Text(message)
                    .font(.caption)
                    .foregroundColor(Theme.statusError)
                    .fixedSize(horizontal: false, vertical: true)
            }
            actions(change)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .contain)
    }

    private func statusLabel(_ change: AgentSettingsResult.Change) -> String {
        switch rowState[change.key] {
        case .undone: return "Undone"
        case .confirmed: return "Changed"
        case .appliedHere: return "Applied on this iPhone"
        case nil: break
        }
        switch change.status {
        case .applied: return "Changed"
        case .applyOnDevice where change.scope == .runtime: return "In Gizzi Code"
        case .applyOnDevice: return "For each device"
        case .needsConfirmation: return "Needs your OK"
        case .locked: return "In Settings only"
        case .invalid: return "Not changed"
        }
    }

    private func valueLine(_ change: AgentSettingsResult.Change) -> String? {
        guard let value = change.value else { return nil }
        if rowState[change.key] == .undone {
            return change.previous.map { "Back to \($0.display)" } ?? "Back to before"
        }
        if case .appliedHere(let previous?) = rowState[change.key], previous != value {
            return "\(previous.display) → \(value.display)"
        }
        if let previous = change.previous, previous != value {
            return "\(previous.display) → \(value.display)"
        }
        return value.display
    }

    private func note(_ change: AgentSettingsResult.Change) -> String? {
        switch change.status {
        case .invalid:
            return change.reason ?? "This change couldn't be made."
        case .locked:
            return change.reason ?? "Change this in Settings."
        case .needsConfirmation where !AgentSettingsApplier.canWriteAccount(change.key):
            return change.reason ?? "Change this in Settings."
        case .needsConfirmation where rowState[change.key] == nil:
            return change.reason
        case .applyOnDevice where change.scope == .runtime:
            return "Gizzi Code applies this on the computer it runs on."
        case .applyOnDevice where !AgentSettingsApplier.canApplyOnDevice(change.key):
            return "This iPhone doesn't keep this setting. Change it in Settings."
        default:
            return nil
        }
    }

    @ViewBuilder
    private func actions(_ change: AgentSettingsResult.Change) -> some View {
        let isBusy = busy.contains(change.key)
        let state = rowState[change.key]
        HStack(spacing: 8) {
            switch change.status {
            case .applied:
                if state == nil, let previous = change.previous, canUndo(change.key) {
                    actionButton("Undo", icon: "arrow.uturn.backward", label: "Undo \(change.label)", busy: isBusy) {
                        await undo(change, to: previous)
                    }
                } else if state == nil, !canUndo(change.key), change.link != nil {
                    settingsButton(change)
                }
            case .applyOnDevice:
                if change.scope == .runtime {
                    EmptyView()
                } else if !AgentSettingsApplier.canApplyOnDevice(change.key) {
                    settingsButton(change)
                } else if case .appliedHere(let previous) = state {
                    if let previous, previous != change.value {
                        actionButton("Undo", icon: "arrow.uturn.backward", label: "Undo \(change.label)", busy: isBusy) {
                            await undo(change, to: previous)
                        }
                    }
                } else if state == nil {
                    actionButton("Apply on this iPhone", icon: "iphone", label: "Apply \(change.label) on this iPhone", busy: isBusy) {
                        applyHere(change)
                    }
                }
            case .needsConfirmation:
                if !AgentSettingsApplier.canWriteAccount(change.key) {
                    settingsButton(change)
                } else if state == nil, let value = change.value {
                    actionButton("Confirm", icon: "checkmark", label: "Confirm \(change.label)", busy: isBusy, prominent: true) {
                        await confirm(change, value: value)
                    }
                }
            case .locked:
                settingsButton(change)
            case .invalid:
                EmptyView()
            }
        }
    }

    private func actionButton(
        _ title: String, icon: String, label: String, busy: Bool, prominent: Bool = false,
        action: @escaping @MainActor () async -> Void
    ) -> some View {
        Group {
            if prominent {
                Button { Task { await action() } } label: { buttonLabel(title, icon: icon, busy: busy) }
                    .buttonStyle(.borderedProminent)
            } else {
                Button { Task { await action() } } label: { buttonLabel(title, icon: icon, busy: busy) }
                    .buttonStyle(.bordered)
            }
        }
        .disabled(busy)
        .frame(minHeight: 44)
        .accessibilityLabel(label)
    }

    @ViewBuilder
    private func buttonLabel(_ title: String, icon: String, busy: Bool) -> some View {
        if busy {
            ProgressView().controlSize(.small)
        } else {
            Label(title, systemImage: icon).font(.caption.weight(.semibold))
        }
    }

    private func settingsButton(_ change: AgentSettingsResult.Change) -> some View {
        Button { AgentSettingsApplier.openSettings(link: change.link) } label: {
            Label("Open Settings", systemImage: "gearshape").font(.caption.weight(.semibold))
        }
        .buttonStyle(.bordered)
        .frame(minHeight: 44)
        .accessibilityLabel("Open Settings for \(change.label)")
    }

    // MARK: Actions

    private func canUndo(_ key: String) -> Bool {
        AgentSettingsApplier.canApplyOnDevice(key) || AgentSettingsApplier.canWriteAccount(key)
    }

    /// Applies this call's device changes once, on the iPhone that sent the
    /// turn; shows the ones already applied by an earlier render.
    private func applyDeviceChanges() {
        for change in result.changes where change.status == .applyOnDevice && AgentSettingsApplier.canApplyOnDevice(change.key) {
            if AgentSettingsApplier.wasApplied(toolCallId: toolCallId, key: change.key) || autoApply {
                applyHere(change)
            }
        }
    }

    private func applyHere(_ change: AgentSettingsResult.Change) {
        switch AgentSettingsApplier.applyOnce(toolCallId: toolCallId, change: change) {
        case .applied(let previous):
            rowState[change.key] = .appliedHere(previous: previous)
            problem[change.key] = nil
        case .unsupported:
            problem[change.key] = "This iPhone couldn't apply that value. Change it in Settings."
        }
    }

    private func undo(_ change: AgentSettingsResult.Change, to previous: AgentSettingValue) async {
        problem[change.key] = nil
        if AgentSettingsApplier.canApplyOnDevice(change.key) {
            if AgentSettingsApplier.setDevice(change.key, previous) {
                rowState[change.key] = .undone
            } else {
                problem[change.key] = "Couldn't undo this. Change it in Settings."
            }
            return
        }
        busy.insert(change.key)
        defer { busy.remove(change.key) }
        do {
            try await AgentSettingsApplier.writeAccount(change.key, previous)
            rowState[change.key] = .undone
        } catch {
            problem[change.key] = "Couldn't undo this. \(error.localizedDescription)"
        }
    }

    private func confirm(_ change: AgentSettingsResult.Change, value: AgentSettingValue) async {
        problem[change.key] = nil
        busy.insert(change.key)
        defer { busy.remove(change.key) }
        do {
            try await AgentSettingsApplier.writeAccount(change.key, value)
            rowState[change.key] = .confirmed
        } catch {
            problem[change.key] = "Couldn't change this. \(error.localizedDescription)"
        }
    }
}
