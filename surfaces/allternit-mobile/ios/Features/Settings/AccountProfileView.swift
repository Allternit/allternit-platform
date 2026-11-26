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
    @StateObject private var statusModel = UserStatusModel()

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
                Section("Status") {
                    Picker("Status", selection: Binding(get: { statusModel.status }, set: { statusModel.choose($0) })) {
                        ForEach(UserStatus.allCases) { item in
                            VStack(alignment: .leading) {
                                Text(item.title)
                                if !item.subline.isEmpty {
                                    Text(item.subline).font(.caption)
                                }
                            }
                            .tag(item)
                        }
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                }
                .task { await statusModel.load() }
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
                    .accessibilityLabel("Delete account")
                    .accessibilityIdentifier("delete-account-row")
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
            activeOrgId = try await AccountSwitch.activate(sessionId: sessionId, orgId: orgId, authManager: authManager)
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

/// Switches the active account (personal or one Clerk organization). Used by
/// this page and the Settings root's Accounts card. Plan contract C7, in this
/// order (`AccountSwitchOrder`): `AccountSwitchCleanup.run` while the leaving
/// account's session still works, then Clerk `setActive`, then
/// `AccountSwitchCleanup.rebuild()` for whichever account is active. The
/// rebuild runs even when Clerk refuses, because the cleanup already dropped
/// the cached data.
@MainActor
enum AccountSwitch {
    /// Returns the organization id now active (nil = personal).
    static func activate(sessionId: String, orgId: String?, authManager: AuthManager) async throws -> String? {
        let steps = AccountSwitchSteps(
            cleanup: { reason in await AccountSwitchCleanup.run(reason: reason) },
            setActive: { sessionId, orgId in
                try await Clerk.shared.auth.setActive(sessionId: sessionId, organizationId: orgId)
            },
            rebuild: {
                authManager.refreshAuthState()
                await AccountSwitchCleanup.rebuild()
            }
        )
        // Organizations of the signed-in person only; another person's
        // session (multi-session) is not offered yet (gated, see Accounts card).
        try await AccountSwitchOrder.run(sessionId: sessionId, orgId: orgId,
                                         reason: .organizationChanged, steps: steps)
        Haptics.selection()
        return Clerk.shared.session?.lastActiveOrganizationId ?? orgId
    }

    /// "Switch account" to another person, one Clerk session at a time
    /// (phone calm I14b, option B; web `signOutToSignIn`). In this order
    /// (`PersonSwitchOrder`): `AccountSwitchCleanup.run(reason:
    /// .personChanged)` while the leaving session still works, then Clerk
    /// sign-out, then the sign-in screen with `email` filled in (nil = "Add
    /// another account", an empty sign-in). Throws when sign-out is refused;
    /// the current person stays signed in and their data is rebuilt.
    static func switchPerson(email: String?, authManager: AuthManager) async throws {
        let steps = PersonSwitchSteps(
            cleanup: { await AccountSwitchCleanup.run(reason: .personChanged) },
            signOut: {
                try await authManager.signOut()
                AccountPlanStore.shared.reset()
            },
            afterSignOut: { AccountSwitchCleanup.resetLocalState(reason: .personChanged) },
            rebuild: {
                authManager.refreshAuthState()
                await AccountSwitchCleanup.rebuild()
                // signOut() revoked the device credential before Clerk
                // refused; pair again for the person still signed in.
                _ = try? await RuntimePairing.shared.ensurePaired()
            },
            showSignIn: { email in authManager.pendingSignInEmail = email }
        )
        try await PersonSwitchOrder.run(email: email, steps: steps)
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
        // The web's avatar letter: the account name's first letter ("A" for "Account").
        Text(authManager.isSignedIn ? String(authManager.accountName.prefix(1)).uppercased() : "A")
            .font(.system(size: size * 0.4, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .frame(width: size, height: size)
            .background(Color("TextPrimary"))
    }
}

extension AuthManager {
    /// "First Last" when Clerk has both names; nil otherwise.
    /// The sample workspace's person ("You") stands in for Clerk, so a
    /// Clerk user cached from an earlier real sign-in never shows through.
    var fullName: String? {
        if SampleWorkspace.isEnabled { return SampleWorkspace.userName }
        guard isClerkConfigured, let user = Clerk.shared.user else { return nil }
        let parts = [user.firstName, user.lastName].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// Clerk profile photo, when the user has one.
    var avatarURL: URL? {
        if SampleWorkspace.isEnabled { return nil }
        guard isClerkConfigured, let user = Clerk.shared.user, user.hasImage else { return nil }
        return URL(string: user.imageUrl)
    }

    /// The account's email for Settings (the sample workspace's in sample mode).
    var accountEmail: String? {
        SampleWorkspace.isEnabled ? SampleWorkspace.userEmail : primaryEmail
    }

    /// The web's account name: first + last, else the email's local part, else "Account".
    var accountName: String {
        if let fullName { return fullName }
        if let email = accountEmail, let local = email.split(separator: "@").first { return String(local) }
        return "Account"
    }
}
