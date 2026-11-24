import SwiftUI

/// New orb (contract "New orb" + "Visibility"; mockup `NewPill`).
/// Rest: a 36 pt glass orb with a compose icon, above the composer's top-right.
/// Tap (light haptic): it opens into a pill. The mode's main item sits at the
/// right end where the orb was; "Quick task" sits to its left and only when
/// `showQuick` is true (a session is open). Picking an item (medium haptic)
/// runs it and closes the pill. Reduce Motion: states change at once.
struct NewOrbView: View {
    let mode: ShellMode
    var showQuick: Bool = true
    var onNew: () -> Void
    var onQuickTask: (() -> Void)? = nil

    @ReducedMotion private var reduced
    @State private var open = false
    @State private var collapseTask: Task<Void, Never>?

    private var quickShown: Bool { showQuick && onQuickTask != nil }

    var body: some View {
        HStack(spacing: 2) {
            if open {
                if quickShown {
                    pillItem(title: "Quick task", icon: "bolt", id: "newOrbQuickTask") {
                        onQuickTask?()
                    }
                }
                pillItem(title: mode.newMainLabel, icon: "square.and.pencil", id: "newOrbMain") {
                    onNew()
                }
            } else {
                Image(systemName: "square.and.pencil")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(Color("TextPrimary"))
                    .frame(width: 36, height: 36)
                    .contentShape(Circle())
                    .onTapGesture { expand() }
                    .accessibilityElement()
                    .accessibilityLabel(mode.newMainLabel)
                    .accessibilityAddTraits(.isButton)
                    .accessibilityIdentifier("newOrb")
                    .accessibilityAction { expand() }
            }
        }
        .padding(open ? 4 : 0)
        .frame(minWidth: 36, minHeight: 36)
        .glassSurface(cornerRadius: 999)
        .animation(reduced ? nil : ModeHubSpec.pillSpring, value: open)
        .onDisappear { collapseTask?.cancel() }
        .accessibilityAction(.escape) { collapse() }
    }

    private func pillItem(title: String, icon: String, id: String, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.medium()
            collapse()
            action()
        } label: {
            HStack(spacing: 6) {
                Image(systemName: icon).font(.system(size: 14, weight: .semibold))
                Text(title).font(.system(size: 15, weight: .medium))
            }
            .foregroundStyle(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(height: 36)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier(id)
    }

    private func expand() {
        Haptics.light()
        open = true
        collapseTask?.cancel()
        collapseTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(4))
            if !Task.isCancelled { open = false }
        }
    }

    private func collapse() {
        collapseTask?.cancel()
        open = false
    }
}

/// Tap expands the pills; picks close them. The hub hides New globally;
/// Tools hides it locally. Layout follows the composer as it grows.
struct ComposerOrbRow: View {
    let hasActiveSession: Bool
    var toolsOpen = false
    /// Optional content right of the mode orb (the Bots home "To:" chip).
    var trailing: AnyView? = nil
    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.modeHubPresented) private var hubPresented
    @State private var quickTaskPresented = false

    var body: some View {
        HStack {
            ModeOrbView(mode: modeStore.shellMode,
                        onSelect: { mode in
                            let from = modeStore.shellMode
                            modeStore.setShellMode(mode)
                            NotificationCenter.default.post(name: ModeHubSpec.modeArrive, object: nil,
                                userInfo: ["from": from.hubId, "to": mode.hubId])
                        },
                        onHold: { NotificationCenter.default.post(name: ModeHubSpec.openModeHub, object: nil) })
            if let trailing {
                trailing.layoutPriority(-1)
            }
            Spacer(minLength: 8)
            if hasActiveSession && !hubPresented && !toolsOpen {
                NewOrbView(mode: modeStore.shellMode, onNew: {
                    NotificationCenter.default.post(name: .composerNewItem, object: nil)
                }, onQuickTask: { quickTaskPresented = true })
            }
        }
        .sheet(isPresented: $quickTaskPresented) { QuickTaskSheet() }
    }
}

private struct ModeHubPresentedKey: EnvironmentKey {
    static let defaultValue = false
}
extension EnvironmentValues {
    var modeHubPresented: Bool {
        get { self[ModeHubPresentedKey.self] }
        set { self[ModeHubPresentedKey.self] = newValue }
    }
}
extension Notification.Name {
    static let composerNewItem = Notification.Name("allternit.composer.new-item")
}
