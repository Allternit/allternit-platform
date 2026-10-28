import SwiftUI

/// Settings → Account → Delete account (App Store guideline 5.1.1(v)).
///
/// Says what deletion does, warns about a paid plan that still bills, asks
/// the user to type DELETE, then calls Clerk's self-delete
/// (`AuthManager.deleteAccount`). When Clerk has self-delete turned off for
/// this instance, or the call fails, the page shows the reason and an email
/// link to have it deleted by hand.
struct DeleteAccountView: View {
    @EnvironmentObject private var authManager: AuthManager
    @ObservedObject private var plan = AccountPlanStore.shared
    @ObservedObject private var storefront = StorefrontGate.shared
    @Environment(\.openURL) private var openURL

    /// Called after the account is gone (Settings closes itself).
    var onDeleted: () -> Void = {}

    @State private var confirmText = ""
    @State private var isDeleting = false
    @State private var errorMessage: String? = nil
    @State private var isDeleted = false
    @FocusState private var isFieldFocused: Bool

    private var isConfirmed: Bool {
        confirmText.trimmingCharacters(in: .whitespaces) == "DELETE"
    }

    var body: some View {
        List {
            if isDeleted {
                deletedSection
            } else {
                explainSection
                if plan.subscription?.isPaid == true {
                    paidPlanSection
                }
                if authManager.canSelfDeleteAccount == false {
                    disabledSection
                } else {
                    confirmSection
                }
                if let errorMessage {
                    Section {
                        FriendlyInlineStateView(
                            style: .error,
                            icon: "exclamationmark.triangle",
                            title: "Couldn't delete your account",
                            message: errorMessage,
                            actionTitle: "Email privacy@allternit.com",
                            action: { openURL(AuthManager.accountDeletionSupportURL) }
                        )
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Delete account")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(isDeleting || isDeleted)
        .interactiveDismissDisabled(isDeleting || isDeleted)
        .task { plan.load() }
        .onDisappear {
            // Left without tapping Done: still return to the sign-in gate.
            if isDeleted { Task { await authManager.finishAccountDeletion() } }
        }
    }

    // MARK: - Sections

    private var explainSection: some View {
        Section {
            VStack(alignment: .leading, spacing: 10) {
                if let email = authManager.primaryEmail {
                    Text(email)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                }
                bullet("Your Allternit account is closed and you're signed out on every device.")
                bullet("You can't sign back in to it. Signing up again with the same email starts a new, empty account.")
                bullet("Allternit closes the account on its servers and stops using its data. To have stored chats, bots, memory and files erased right away, email privacy@allternit.com.")
                bullet("Credits left on the account are not refunded.")
                bullet("This can't be undone.")
            }
            .padding(.vertical, 6)
        } header: {
            Text("What happens")
        } footer: {
            Text("Want a copy first? Export your data from Settings → Account → Privacy before you delete.")
        }
    }

    private var paidPlanSection: some View {
        Section {
            FriendlyInlineStateView(
                style: .offline,
                icon: "creditcard",
                title: "Cancel \(plan.subscription?.label ?? "your plan") first",
                message: storefront.allowsPurchaseLinks
                    ? "Deleting your account doesn't cancel a paid plan. Cancel it in Billing on the web first so you aren't charged again."
                    : "Deleting your account doesn't cancel a paid plan. Cancel it on the web at ai.allternit.com first so you aren't charged again.",
                actionTitle: storefront.allowsPurchaseLinks ? "Open Billing" : nil,
                action: storefront.allowsPurchaseLinks ? ({ openURL(StorefrontGate.creditsURL) } as () -> Void) : nil
            )
        }
    }

    private var disabledSection: some View {
        Section {
            FriendlyInlineStateView(
                style: .empty,
                icon: "envelope",
                title: "We'll delete it for you",
                message: AuthManager.DeleteAccountError.selfDeleteDisabled.errorDescription ?? "",
                actionTitle: "Email privacy@allternit.com",
                action: { openURL(AuthManager.accountDeletionSupportURL) }
            )
        }
    }

    private var confirmSection: some View {
        Section {
            TextField("Type DELETE", text: $confirmText)
                .font(.system(.body, design: .monospaced))
                .textInputAutocapitalization(.characters)
                .autocorrectionDisabled()
                .focused($isFieldFocused)
                .disabled(isDeleting)
                .accessibilityLabel("Type DELETE to confirm")

            Button(role: .destructive, action: { Task { await delete() } }) {
                HStack {
                    if isDeleting {
                        ProgressView()
                        Text("Deleting…")
                    } else {
                        Image(systemName: "trash")
                        Text("Delete my account")
                    }
                    Spacer()
                }
                .font(.subheadline.weight(.semibold))
                .foregroundColor(isConfirmed && !isDeleting ? Theme.statusError : Color("TextSecondary"))
                .contentShape(Rectangle())
            }
            .disabled(!isConfirmed || isDeleting || !authManager.isSignedIn)
        } header: {
            Text("Confirm")
        } footer: {
            Text("Type DELETE in capitals to turn on the button.")
        }
    }

    private var deletedSection: some View {
        Section {
            FriendlyStateView(
                style: .empty,
                icon: "checkmark.circle",
                title: "Your account is deleted",
                message: "You've been signed out. Thanks for trying Allternit.",
                actionTitle: "Done",
                action: {
                    Task {
                        await authManager.finishAccountDeletion()
                        onDeleted()
                    }
                }
            )
            .padding(.vertical, 16)
        }
    }

    private func bullet(_ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("•")
            Text(text)
                .fixedSize(horizontal: false, vertical: true)
        }
        .font(.subheadline)
        .foregroundColor(Color("TextSecondary"))
    }

    // MARK: - Delete

    @MainActor
    private func delete() async {
        guard isConfirmed, !isDeleting else { return }
        isFieldFocused = false
        isDeleting = true
        errorMessage = nil
        defer { isDeleting = false }
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        do {
            try await authManager.deleteAccount()
            AccountPlanStore.shared.reset()
            isDeleted = true
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } catch {
            errorMessage = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
