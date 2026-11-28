import SwiftUI

/// Settings > Organization & Access parity.
///
/// Reads the backend-resolved user profile (`GET /api/v1/me`) and exposes
/// personal-organization creation (`POST /api/v1/me/organization`) for
/// self-hosted/no-Clerk-key builds, matching the web panel's fallback flow.
struct OrganizationAccessView: View {
    @State private var profile: UserProfile? = nil
    @State private var isLoading = false
    @State private var error: String? = nil
    @State private var isCreating = false

    private let client = OrganizationClient.shared

    var body: some View {
        DockingTitleList(title: "Organization & Access") {
            profileSection
            organizationSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Organization & Access")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            await load()
        }
        .refreshable {
            await load()
        }
    }

    // MARK: - Profile

    @ViewBuilder
    private var profileSection: some View {
        Section {
            if isLoading && profile == nil {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
            } else if let error, profile == nil {
                let offline = isConnectionFailure(error)
                FriendlyInlineStateView(
                    style: offline ? .offline : .error,
                    icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                    title: "Could not load profile",
                    message: FriendlyErrorMessage.from(error)
                )
            } else if let profile {
                HStack(spacing: 12) {
                    Text(initials(for: profile.displayName))
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("BgPrimary"))
                        .frame(width: 44, height: 44)
                        .background(Color("TextSecondary"))
                        .clipShape(Circle())

                    VStack(alignment: .leading, spacing: 2) {
                        Text(profile.displayName)
                            .font(.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextPrimary"))
                        Text(profile.email)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    Spacer(minLength: 8)

                    Text(profile.role)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(Color("BgSecondary"))
                        .clipShape(Capsule())
                }

                HStack {
                    Text("User ID")
                        .font(.subheadline)
                    Spacer()
                    Text(profile.id)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
        } header: {
            Text("Profile")
        }
    }

    // MARK: - Organization

    @ViewBuilder
    private var organizationSection: some View {
        Section {
            if let profile {
                if let orgId = profile.organizationId {
                    HStack {
                        Text("Organization")
                            .font(.subheadline)
                        Spacer()
                        Text(orgId)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }

                    HStack {
                        Text("Role")
                            .font(.subheadline)
                        Spacer()
                        Text(profile.organizationRole?.replacingOccurrences(of: "org:", with: "") ?? "member")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                } else {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "building.2",
                        title: "No organization",
                        message: "No organization resolved for this account."
                    )

                    Button(action: {
                        Task { await createOrganization() }
                    }) {
                        HStack {
                            Text("Create personal organization")
                                .font(.subheadline)
                            Spacer()
                            if isCreating {
                                ProgressView()
                            }
                        }
                        .foregroundColor(Color("AccentPrimary"))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(isCreating)
                }
            }

            if let error {
                let offline = isConnectionFailure(error)
                FriendlyInlineStateView(
                    style: offline ? .offline : .error,
                    icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                    title: "Could not create organization",
                    message: FriendlyErrorMessage.from(error)
                )
            }
        } header: {
            Text("Organization")
        } footer: {
            Text("Real Clerk deployments get organization scope from Clerk's org-creation flow; this panel provides the fallback path used by self-hosted builds.")
                .font(.caption)
        }
    }

    // MARK: - Actions

    private func load() async {
        isLoading = true
        error = nil
        do {
            profile = try await client.fetchProfile()
        } catch {
            self.error = error.localizedDescription
        }
        isLoading = false
    }

    private func createOrganization() async {
        isCreating = true
        error = nil
        do {
            _ = try await client.createPersonalOrganization()
            await load()
        } catch {
            self.error = error.localizedDescription
        }
        isCreating = false
    }

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private func initials(for name: String) -> String {
        let components = name.components(separatedBy: CharacterSet.whitespacesAndNewlines)
            .filter { !$0.isEmpty }
        let first = components.first?.prefix(1).uppercased() ?? "?"
        let last = components.count > 1 ? components.last?.prefix(1).uppercased() : nil
        return last.map { "\(first)\($0)" } ?? first
    }
}
