import SwiftUI
import ClerkKit

/// Settings → Account: who's signed in, which account is active (personal or
/// one of the Clerk organizations, the PWA's useAccountSummary switcher),
/// Log out and Delete account.
struct AccountProfileView: View {
    @EnvironmentObject private var authManager: AuthManager
    @Environment(\.dismiss) private var dismiss
    /// Closes the whole Settings sheet (after log out / delete).
    var onClose: () -> Void = {}

    @State private var memberships: [OrganizationMembership] = []
    @State private var activeOrgId: String? = nil
    @State private var isLoadingOrgs = false
    @State private var switchingTo: String?? = nil
    @State private var errorMessage: String? = nil
    @State private var isDeletePresented = false

    var body: some View {
        List {
            Section {
                HStack(spacing: 12) {
                    AccountAvatar(size: 52)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(authManager.fullName ?? authManager.displayName)
                            .font(.headline)
                            .foregroundColor(Color("TextPrimary"))
                        if let email = authManager.primaryEmail {
                            Text(email)
                                .font(.subheadline)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
                .padding(.vertical, 4)
            }

            if authManager.isSignedIn {
                accountsSection
            }

            Section {
                if authManager.isSignedIn {
                    Button(role: .destructive, action: { Task { await signOut() } }) {
                        Label("Log out", systemImage: "rectangle.portrait.and.arrow.right")
                            .font(.subheadline)
                    }
                    Button(action: { isDeletePresented = true }) {
                        Label("Delete account", systemImage: "person.crop.circle.badge.xmark")
                            .font(.subheadline)
                            .foregroundColor(Theme.statusError)
                    }
                } else {
                    Button(action: {
                        onClose()
                        authManager.signIn()
                    }) {
                        Label("Sign in", systemImage: "person.crop.circle")
                            .font(.subheadline)
                    }
                }
            } footer: {
                if let errorMessage {
                    Text(errorMessage).foregroundColor(Theme.statusError)
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Account")
        .navigationBarTitleDisplayMode(.inline)
        .navigationDestination(isPresented: $isDeletePresented) {
            DeleteAccountView(onDeleted: onClose)
        }
        .task { await loadOrganizations() }
    }

    // MARK: - Account switcher

    @ViewBuilder
    private var accountsSection: some View {
        Section {
            accountRow(orgId: nil, name: "Personal", detail: authManager.primaryEmail)
            ForEach(memberships, id: \.id) { membership in
                accountRow(
                    orgId: membership.organization.id,
                    name: membership.organization.name,
                    detail: membership.roleName
                )
            }
            if isLoadingOrgs && memberships.isEmpty {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Loading organizations…")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        } header: {
            Text("Accounts")
        } footer: {
            Text("Bots, chats and billing follow the active account.")
        }
    }

    private func accountRow(orgId: String?, name: String, detail: String?) -> some View {
        Button(action: { Task { await switchAccount(to: orgId) } }) {
            HStack(spacing: 12) {
                Image(systemName: orgId == nil ? "person.fill" : "building.2.fill")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 1) {
                    Text(name)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    if let detail, !detail.isEmpty {
                        Text(detail)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer()
                if switchingTo == .some(orgId) {
                    ProgressView()
                } else if activeOrgId == orgId {
                    Image(systemName: "checkmark")
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(switchingTo != nil)
        .accessibilityAddTraits(activeOrgId == orgId ? .isSelected : [])
    }

    // MARK: - Actions

    @MainActor
    private func loadOrganizations() async {
        guard authManager.isClerkConfigured, let user = Clerk.shared.user else { return }
        activeOrgId = Clerk.shared.session?.lastActiveOrganizationId
        if let cached = user.organizationMemberships, !cached.isEmpty {
            memberships = cached
        }
        isLoadingOrgs = true
        defer { isLoadingOrgs = false }
        do {
            memberships = try await user.getOrganizationMemberships(page: 1, pageSize: 50).data
        } catch {
            if memberships.isEmpty {
                errorMessage = "Couldn't load your organizations. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    @MainActor
    private func switchAccount(to orgId: String?) async {
        guard orgId != activeOrgId, let sessionId = Clerk.shared.session?.id else { return }
        switchingTo = .some(orgId)
        errorMessage = nil
        defer { switchingTo = nil }
        do {
            try await Clerk.shared.auth.setActive(sessionId: sessionId, organizationId: orgId)
            activeOrgId = Clerk.shared.session?.lastActiveOrganizationId ?? orgId
            authManager.refreshAuthState()
            // Everything account-scoped reloads for the new account.
            AccountPlanStore.shared.reset()
            AccountPlanStore.shared.load(force: true)
            UsageStore.shared.refresh()
            NotificationCenter.default.post(name: .historyMutated, object: nil)
            UISelectionFeedbackGenerator().selectionChanged()
        } catch {
            errorMessage = "Couldn't switch accounts. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    @MainActor
    private func signOut() async {
        do {
            try await authManager.signOut()
            AccountPlanStore.shared.reset()
            onClose()
        } catch {
            errorMessage = "Couldn't log out. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}

/// The signed-in person's photo (Clerk imageUrl), else their initial.
struct AccountAvatar: View {
    @EnvironmentObject private var authManager: AuthManager
    var size: CGFloat = 44

    var body: some View {
        Group {
            if let url = authManager.avatarURL {
                AsyncImage(url: url) { image in
                    image.resizable().scaledToFill()
                } placeholder: {
                    initial
                }
            } else {
                initial
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }

    private var initial: some View {
        Text(authManager.isSignedIn ? authManager.avatarInitial : "A")
            .font(.system(size: size * 0.4, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .frame(width: size, height: size)
            .background(Color("TextSecondary"))
    }
}

extension AuthManager {
    /// "First Last" when Clerk has both names; nil otherwise.
    var fullName: String? {
        guard isClerkConfigured, let user = Clerk.shared.user else { return nil }
        let parts = [user.firstName, user.lastName].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// Clerk profile photo, when the user has one.
    var avatarURL: URL? {
        guard isClerkConfigured, let user = Clerk.shared.user, user.hasImage else { return nil }
        return URL(string: user.imageUrl)
    }
}
