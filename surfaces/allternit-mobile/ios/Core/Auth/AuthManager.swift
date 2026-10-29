import Foundation
import Combine
import AuthenticationServices
import ClerkKit

/// Thin wrapper over the official Clerk iOS SDK v1.3.x (ClerkKit / ClerkKitUI).
///
/// Dashboard prerequisites before this works against a real Clerk application:
/// 1. Enable the **Native API** (Clerk Dashboard → Native applications).
/// 2. Register this app's App ID Prefix + Bundle ID on that page.
/// 3. Add the Associated Domains capability to the target with
///    `webcredentials:{YOUR_FRONTEND_API_URL}` (required for seamless
///    OAuth / Apple Sign-In hand-off).
///
/// The publishable key comes from `AppConfig.clerkPublishableKey` (Info.plist).
/// While it is empty the SDK is deliberately left unconfigured and the login
/// gate shows a warning instead of crashing — accessing `Clerk.shared` before
/// `Clerk.configure` assertion-fails in debug builds.
@MainActor
final class AuthManager: ObservableObject {
    enum AuthEntryMode {
        case signIn
        case signUp
    }

    static let shared = AuthManager()

    /// Whether `Clerk.configure` ran with a non-empty publishable key.
    @Published private(set) var isClerkConfigured = false
    @Published private(set) var isSignedIn = false
    /// DEBUG / preview bypass: when true the workspace is shown without a
    /// Clerk session. API calls go out without a Bearer token.
    @Published private(set) var isSkippingAuth = false
    @Published private(set) var currentSession: Session?

    /// Set true to present ClerkKitUI's `AuthView` sheet (see AllternitApp).
    @Published var isPresentingAuth = false
    @Published private(set) var authEntryMode: AuthEntryMode = .signIn

    private init() {}

    #if DEBUG
    private func debugLog(_ message: String) {
        NSLog("%@", "[AuthManager] " + message)
    }
    #endif

    /// Configures the Clerk SDK. Call once at app start.
    func configure(publishableKey: String) {
        guard !publishableKey.isEmpty else {
            #if DEBUG
            debugLog("CLERK_PUBLISHABLE_KEY is empty — Clerk SDK not configured.")
            #endif
            return
        }
        Clerk.configure(publishableKey: publishableKey)
        isClerkConfigured = true
        refreshAuthState()
    }

    /// Mirrors `Clerk.shared.session` into the published gate state. Called
    /// after configure, sign-out, auth-sheet dismissal, and foregrounding.
    /// When the user is signed in, kick off runtime pairing in the background
    /// so API calls can upgrade from a short-lived Clerk JWT to a long-lived
    /// cloud-issued device token.
    func refreshAuthState() {
        guard isClerkConfigured else { return }
        currentSession = Clerk.shared.session
        let wasSignedIn = isSignedIn
        isSignedIn = currentSession != nil

        if isSignedIn, !wasSignedIn {
            // Push: send this phone's APNs token for the account now signed in.
            Task { await PushNotificationManager.shared.didSignIn() }
            Task {
                do {
                    _ = try await RuntimePairing.shared.ensurePaired()
                } catch {
                    #if DEBUG
                    debugLog("Runtime pairing failed: \(error.localizedDescription)")
                    #endif
                }
            }
        }
    }

    /// Clerk session token for the API layer (the SDK caches and refreshes it).
    func getToken() async throws -> String? {
        guard isClerkConfigured else { return nil }
        let token = try await Clerk.shared.session?.getToken(.init(skipCache: true))
        #if DEBUG
        if let token {
            let parts = token.split(separator: ".")
            if parts.count == 3,
               let payloadData = base64URLDecode(String(parts[1])),
               let json = try? JSONSerialization.jsonObject(with: payloadData) as? [String: Any] {
                debugLog("Clerk JWT claims — iss: \(json["iss"] ?? "-"), aud: \(json["aud"] ?? "-"), exp: \(json["exp"] ?? "-"), sub: \(json["sub"] ?? "-"), template: \(json["template"] ?? "-"), sts: \(json["sts"] ?? "-")")
            }
            if ProcessInfo.processInfo.environment["DUMP_CLERK_TOKEN"] == "1" {
                let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first
                let path = docs?.appendingPathComponent("ios_clerk_token.txt").path ?? "/tmp/ios_clerk_token.txt"
                try? token.write(toFile: path, atomically: true, encoding: .utf8)
                debugLog("Wrote Clerk token to \(path)")
            }
        } else {
            debugLog("Clerk session token is nil")
        }
        #endif
        return token
    }

    /// Returns the runtime device token when paired and valid; otherwise falls
    /// back to the Clerk session token. This is what `APIClient` sends as the
    /// Bearer token, matching desktop's use of `allternit_runtime_…` credentials.
    func effectiveToken() async throws -> String? {
        if RuntimePairing.shared.isPaired {
            await RuntimePairing.shared.rotateIfNeeded()
        }
        if let token = RuntimePairing.shared.deviceToken() {
            #if DEBUG
            debugLog("effectiveToken using runtime device token")
            #endif
            return token
        }
        #if DEBUG
        debugLog("effectiveToken falling back to Clerk session token")
        #endif
        return try await getToken()
    }

    #if DEBUG
    private func base64URLDecode(_ value: String) -> Data? {
        var base64 = value
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while base64.count % 4 != 0 {
            base64.append("=")
        }
        return Data(base64Encoded: base64)
    }
    #endif

    /// Bridges to ClerkKitUI: presenting the `AuthView` sheet is the sign-in flow.
    func signIn() {
        guard isClerkConfigured else { return }
        authEntryMode = .signIn
        isPresentingAuth = true
    }

    /// Last Sign in with Apple failure, shown under the button. Nil when the
    /// person cancelled the Apple sheet (not an error).
    @Published var appleSignInError: String?
    @Published private(set) var isSigningInWithApple = false

    /// Native Sign in with Apple through Clerk (ClerkKit 1.1.0
    /// `Auth.signInWithApple`, which signs up a new Apple ID too). App Store
    /// guideline 4.8: offered next to Google on the sign-in screen. Needs the
    /// Sign in with Apple capability on the App ID and Apple enabled as a
    /// social connection in the Clerk dashboard (native, bundle id
    /// com.allternit.mobile).
    func signInWithApple() {
        guard isClerkConfigured, !isSigningInWithApple else { return }
        isSigningInWithApple = true
        appleSignInError = nil
        Task {
            defer { isSigningInWithApple = false }
            do {
                try await Clerk.shared.auth.signInWithApple()
                refreshAuthState()
                // A session still pending a step (e.g. choose an organization)
                // finishes in Clerk's own sheet.
                if !isSignedIn {
                    authEntryMode = .signIn
                    isPresentingAuth = true
                }
            } catch {
                if (error as NSError).domain == "com.apple.AuthenticationServices.AuthorizationError",
                   (error as NSError).code == 1001 {
                    return // cancelled
                }
                appleSignInError = error.localizedDescription
            }
        }
    }

    /// Social providers offered under Apple on the sign-in screen, matching
    /// the web sign-in (GitHub, then Google).
    enum SocialProvider: String, CaseIterable, Identifiable {
        case github
        case google

        var id: String { rawValue }

        var title: String {
            switch self {
            case .github: return "GitHub"
            case .google: return "Google"
            }
        }

        fileprivate var clerkProvider: OAuthProvider {
            switch self {
            case .github: return .github
            case .google: return .google
            }
        }
    }

    /// Last GitHub / Google sign-in failure, shown under those buttons. Nil
    /// when the person closed the browser sheet (not an error).
    @Published var socialSignInError: String?
    @Published private(set) var signingInWith: SocialProvider?

    /// GitHub / Google through Clerk's native OAuth (ClerkKit
    /// `Auth.signInWithOAuth`, an ASWebAuthenticationSession that signs up a
    /// new account too). Needs the provider enabled as a social connection in
    /// the Clerk dashboard and the app's redirect URL allowed for native.
    func signIn(with provider: SocialProvider) {
        guard isClerkConfigured, signingInWith == nil else { return }
        signingInWith = provider
        socialSignInError = nil
        Task {
            defer { signingInWith = nil }
            do {
                try await Clerk.shared.auth.signInWithOAuth(provider: provider.clerkProvider)
                refreshAuthState()
                // A sign-up missing fields, or a session still pending a step,
                // finishes in Clerk's own sheet.
                if !isSignedIn {
                    authEntryMode = .signIn
                    isPresentingAuth = true
                }
            } catch {
                if case ASWebAuthenticationSessionError.canceledLogin = error { return }
                socialSignInError = error.localizedDescription
            }
        }
    }

    #if DEBUG
    /// `-seed-auth` (DEBUG only): automatically sign into Clerk with the
    /// credentials supplied via `CLERK_SEED_EMAIL` / `CLERK_SEED_PASSWORD`.
    /// Used for end-to-end chat/conversation regression testing without
    /// manual sign-in on every simulator launch.
    func seedSignInIfNeeded() {
        guard CommandLine.arguments.contains("-seed-auth") else { return }
        guard let email = ProcessInfo.processInfo.environment["CLERK_SEED_EMAIL"],
              let password = ProcessInfo.processInfo.environment["CLERK_SEED_PASSWORD"],
              !email.isEmpty, !password.isEmpty else {
            debugLog("-seed-auth requested but CLERK_SEED_EMAIL and/or CLERK_SEED_PASSWORD are missing/empty")
            return
        }
        Task {
            do {
                // Clear any persisted Clerk session so the seeded credentials
                // always produce a fresh sign-in.
                try? await Clerk.shared.auth.signOut()
                let signIn = try await Clerk.shared.auth.signInWithPassword(identifier: email, password: password)
                debugLog("Seed sign-in status: \(String(describing: signIn.status)), createdSessionId: \(signIn.createdSessionId ?? "nil")")
                if let sessionId = signIn.createdSessionId, !sessionId.isEmpty {
                    // Organizations are enabled on this Clerk instance. A brand-new
                    // seeded account has no organization memberships, so the session
                    // stays in the pending "choose-organization" task. Pick an
                    // existing membership or auto-create a personal seed organization
                    // and make it active so the token becomes active.
                    var seedOrgId: String? = Clerk.shared.user?.organizationMemberships?.first?.organization.id
                    if signIn.status == .complete, seedOrgId == nil {
                        do {
                            let org = try await Clerk.shared.organizations.create(
                                name: "Allternit Seed"
                            )
                            seedOrgId = org.id
                            debugLog("Created seed organization \(org.id)")
                        } catch {
                            debugLog("Failed to create seed organization: \(error)")
                        }
                    }
                    try await Clerk.shared.auth.setActive(sessionId: sessionId, organizationId: seedOrgId)
                    debugLog("setActive completed for session \(sessionId), org: \(seedOrgId ?? "none")")
                    // Prime the Clerk session object so subsequent getToken() calls
                    // (including RuntimePairing) don't see a transient signed-out state.
                    _ = try? await getToken()
                }
                // Drop any stale runtime-pairing session from the Keychain so the
                // API client falls back to the Clerk JWT instead of an invalid
                // device token.
                await RuntimePairing.shared.revoke()
                await MainActor.run { refreshAuthState() }
                // Pair this simulator as an iOS runtime so the API client can use
                // a long-lived cloud device token instead of the short Clerk JWT.
                do {
                    let deviceSession = try await RuntimePairing.shared.ensurePaired()
                    debugLog("Runtime pairing succeeded; device token expires at \(deviceSession.expiresAt)")
                } catch {
                    debugLog("Runtime pairing failed after seed sign-in: \(error.localizedDescription)")
                }
                // Seed auth is for exercising the workspace, not onboarding.
                OnboardingStore.shared.complete()
                debugLog("Seed sign-in succeeded for \(email)")
                // Force a token fetch so the DUMP_CLERK_TOKEN file is written
                // and we can verify session status without waiting for an API call.
                _ = try? await getToken()
            } catch {
                debugLog("Seed sign-in failed: \(error)")
            }
        }
    }
    #endif

    /// Opens Clerk's dedicated account-creation flow.
    func signUp() {
        guard isClerkConfigured else { return }
        authEntryMode = .signUp
        isPresentingAuth = true
    }

    /// Lets a user enter the workspace without signing in (ChatGPT/Claude
    /// iOS parity). API calls go out without a Bearer token, so backend-fed
    /// views show their error/empty states. Onboarding is also completed so
    /// the workspace opens immediately.
    func skipAuth() {
        isSkippingAuth = true
        OnboardingStore.shared.complete()
    }

    func signOut() async throws {
        guard isClerkConfigured else {
            isSkippingAuth = false
            return
        }
        // Stop pushes to this phone for the account leaving (needs its Bearer).
        await PushNotificationManager.shared.willSignOut()
        // Revoke the iOS runtime device credential before signing out of Clerk.
        // The revoke call needs the current device token, so it must run first.
        await RuntimePairing.shared.revoke()
        try await Clerk.shared.auth.signOut()
        refreshAuthState()
    }

    /// Signed-in user's first name for the Phase-10 onboarding welcome
    /// ("Hey, Joe!"); nil when signed out / skip-auth / no first name, so
    /// the caller falls back to "Hey there!".
    var firstName: String? {
        guard isClerkConfigured else { return nil }
        guard let firstName = Clerk.shared.user?.firstName,
              !firstName.isEmpty else { return nil }
        return firstName
    }

    /// Display name for the sidebar footer: first name, full name, or the
    /// primary email's local part — whichever resolves first.
    var displayName: String {
        guard isClerkConfigured, let user = Clerk.shared.user else { return "Guest" }
        if let firstName = user.firstName, !firstName.isEmpty {
            return firstName
        }
        if let email = user.emailAddresses.first?.emailAddress {
            return email.split(separator: "@").first.map(String.init) ?? email
        }
        return "Account"
    }

    /// Sidebar avatar initial — the first character of `displayName`,
    /// uppercased (matches the reference app's single-letter avatar).
    var avatarInitial: String {
        guard isClerkConfigured else { return "A" }
        return displayName.first.map { String($0).uppercased() } ?? "?"
    }

    /// The signed-in user's primary email address (Settings account row);
    /// nil when signed out or the account has no email.
    var primaryEmail: String? {
        guard isClerkConfigured else { return nil }
        return Clerk.shared.user?.emailAddresses.first?.emailAddress
    }

    /// Clerk user id for ownership filtering (e.g., Projects "Created by you").
    /// nil when signed out / skip-auth.
    var currentUserId: String? {
        Clerk.shared.user?.id
    }
}
