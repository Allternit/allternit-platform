import SwiftUI

/// Settings > Products parity — Gizzi Code, Cowork, and Extensions.
///
/// Phase 1 surfaces the user-facing toggles that map to the web's products
/// group. Most are local preference flags.
struct ProductsSettingsView: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var settings = SettingsStore.shared

    var body: some View {
        List {
            gizziioCodeSection
            coworkSection
            extensionsSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Products")
        .navigationBarTitleDisplayMode(.inline)
    }

    // MARK: - Gizzi Code

    private var gizziioCodeSection: some View {
        Section {
            Toggle(isOn: $settings.gizziBrowserTools) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Browser tools")
                        .font(.subheadline)
                    Text("Allow Gizzi Code to read from browser contexts.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))

            Toggle(isOn: $settings.gizziAutoCreatePRs) {
                Text("Auto-create pull requests")
                    .font(.subheadline)
            }
            .tint(Color("AccentPrimary"))

            Toggle(isOn: $settings.gizziAutofixPRs) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Autofix failing PRs")
                        .font(.subheadline)
                    Text("Retry code changes when CI checks fail.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))

            Toggle(isOn: $settings.gizziDrawAttentionNotifications) {
                Text("Draw-attention notifications")
                    .font(.subheadline)
            }
            .tint(Color("AccentPrimary"))

            #if DEBUG
            Toggle(isOn: $settings.gizziBypassPermissions) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Bypass permissions checks")
                        .font(.subheadline)
                    Text("Developer only — disables desktop permission gating.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))
            #endif
        } header: {
            Text("Gizzi Code")
        }
    }

    // MARK: - Cowork

    private var coworkSection: some View {
        Section {
            Toggle(isOn: $settings.dispatchEnabled) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Enable dispatch")
                        .font(.subheadline)
                    Text("Route cowork tasks through Allternit's dispatch layer.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))

            Button(action: {
                // Close the settings sheet and switch to the Cowork workspace
                // via the Chats tab toggle.
                modeStore.mode = .cowork
            }) {
                HStack {
                    Text("Open Cowork workspace")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Cowork")
        }
    }

    // MARK: - Extensions

    private var extensionsSection: some View {
        Section {
            Toggle(isOn: $settings.extensionsAutoUpdate) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Auto-update extensions")
                        .font(.subheadline)
                    Text("Background update marketplace and sidecar extensions.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))

            Toggle(isOn: $settings.extensionsUseBuiltinNode) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Use built-in Node runtime")
                        .font(.subheadline)
                    Text("Run extension sidecars with the bundled Node runtime.")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .tint(Color("AccentPrimary"))

            Text("Installed marketplace extensions are managed automatically.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        } header: {
            Text("Extensions")
        }
    }
}
