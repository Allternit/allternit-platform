import SwiftUI
import ClerkKit
import ClerkKitUI

@main
struct AllternitApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var authManager = AuthManager.shared
    /// Platform mode state (chat/cowork/code/browser) + composer agent
    /// state, available environment-wide so every surface reads the same
    /// mode accent and agent pill context.
    @StateObject private var modeStore = AppModeStore()
    @StateObject private var agentModeStore = AgentModeStore()
    /// First-launch onboarding gate (Phase 10) — `isComplete` flips drive
    /// the root swap from OnboardingView to the workspace.
    @StateObject private var onboardingStore = OnboardingStore.shared
    @Environment(\.scenePhase) private var scenePhase
    @State private var openedDocumentURL: URL?
    /// DEBUG-only manual bypass from the login gate so the app can be opened
    /// without signing in (addresses the "open app without auth" gap).
    @State private var skipAuthRequested = false

    init() {
        AuthManager.shared.configure(publishableKey: AppConfig.clerkPublishableKey)
        // Must register before this initializer returns — BGTaskScheduler
        // requires the handler in place before app launch completes.
        BackgroundRefreshManager.register()
        // Registers the scene observer that applies the saved Light/Dark/System
        // choice as soon as the first window activates.
        _ = AppearanceStore.shared
        #if DEBUG
        // `-reset-onboarding` (DEBUG only): also clears the Phase-10
        // onboarding gate (the ChatView site clears the dictation/priming
        // flags). Clearing here means THIS launch lands on page 1.
        if launchArgumentEnabled("reset-onboarding") {
            OnboardingStore.shared.reset()
        }
        #endif
    }

    var body: some Scene {
        WindowGroup {
            Group {
                if authManager.isClerkConfigured || shouldSkipAuth {
                    // Only inject Clerk.shared when the SDK was actually
                    // configured; otherwise `Clerk.shared` traps on access.
                    if authManager.isClerkConfigured {
                        gatedContent
                            // ClerkKitUI views (AuthView) read Clerk from the environment.
                            .environment(Clerk.shared)
                    } else {
                        gatedContent
                    }
                } else {
                    // Publishable key placeholder not filled in — LoginGateView
                    // shows the setup hint instead of crashing on Clerk.shared.
                    // DEBUG: still expose the skip-auth affordance so the app can
                    // be opened without a configured Clerk app.
                    LoginGateView(onSkipAuth: {
                        skipAuthRequested = true
                    })
                }
            }
            .environmentObject(authManager)
            .sheet(isPresented: $authManager.isPresentingAuth, onDismiss: {
                authManager.refreshAuthState()
            }) {
                // The sheet is only presented after Clerk is configured, but
                // SwiftUI may evaluate the content view eagerly. Avoid touching
                // Clerk.shared unless the SDK is actually configured.
                if authManager.isClerkConfigured {
                    AuthView(
                        mode: authManager.authEntryMode == .signUp ? .signUp : .signIn
                    )
                        .environment(Clerk.shared)
                } else {
                    EmptyView()
                }
            }
            .onChange(of: scenePhase) { _, newPhase in
                if newPhase == .active {
                    authManager.refreshAuthState()
                    PushNotificationManager.shared.clearBadge()
                } else if newPhase == .background {
                    BackgroundRefreshManager.scheduleNextRefresh()
                }
            }
            .onOpenURL { url in
                // Universal Links (ai./m.allternit.com) and allternit:// open
                // in the workspace; anything else is a document to view.
                if DeepLinkRouter.shared.open(url: url) { return }
                openedDocumentURL = url
            }
            .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                if let url = activity.webpageURL {
                    DeepLinkRouter.shared.open(url: url)
                }
            }
            .sheet(isPresented: Binding(
                get: { openedDocumentURL != nil },
                set: { if !$0 { openedDocumentURL = nil } }
            )) {
                if let url = openedDocumentURL {
                    LocalDocumentView(fileURL: url)
                }
            }
        }
    }

    @ViewBuilder
    private var gatedContent: some View {
        if authManager.isSignedIn || shouldSkipAuth {
            // Phase 10: first-launch onboarding runs BEFORE the workspace
            // (root swap — it never covers LoginGateView). Completing or
            // skipping flips `isComplete` and swaps in the workspace.
            if (!onboardingStore.isComplete && !shouldBypassOnboarding)
                || Self.forceOnboardingForTesting {
                OnboardingView()
            } else {
                MainWorkspaceView()
                    // iPhone link: screen-sharing banner, share prompt,
                    // control socket lifecycle (Features/Phones).
                    .appPhoneOverlays()
                    .environmentObject(modeStore)
                    .environmentObject(agentModeStore)
            }
        } else {
            LoginGateView(onSkipAuth: {
                skipAuthRequested = true
            })
        }
    }

    /// DEBUG-only: true when the `-skip-auth` launch argument is present or
    /// the user tapped the login gate's skip button.
    private var shouldSkipAuth: Bool {
        #if DEBUG
        Self.skipAuthForTesting || skipAuthRequested
        #else
        false
        #endif
    }

    /// DEBUG-only: bypass onboarding when skipping auth so the workspace is
    /// reachable immediately.
    private var shouldBypassOnboarding: Bool {
        #if DEBUG
        Self.bypassOnboardingForTesting || skipAuthRequested
        #else
        false
        #endif
    }

    /// `-skip-auth` (DEBUG builds only): bypasses the Clerk gate to exercise
    /// the workspace UI without an account. API calls go out without a
    /// Bearer token, so backend-fed views show their error/empty states.
    private static var skipAuthForTesting: Bool {
        #if DEBUG
        launchArgumentEnabled("skip-auth")
        #else
        false
        #endif
    }

    /// `-open-onboarding` (DEBUG builds only): force-show the Phase-10
    /// onboarding flow regardless of the complete flag, for screenshots.
    private static var forceOnboardingForTesting: Bool {
        #if DEBUG
        launchArgumentEnabled("open-onboarding")
        #else
        false
        #endif
    }

    /// Workspace-targeting DEBUG args imply "past onboarding" — simctl has
    /// no tap injection, so without this the Phase-10 gate would swallow
    /// the regression args (`-autosend`, `-open-settings`, …) on a fresh
    /// install before they ever reach the workspace.
    private static var bypassOnboardingForTesting: Bool {
        #if DEBUG
        launchArgumentEnabled("autosend")
            || launchArgumentEnabled("open-settings")
            || launchArgumentEnabled("open-settings-memory")
            || launchArgumentEnabled("open-settings-platform")
            || launchArgumentEnabled("open-settings-data")
            || launchArgumentEnabled("open-settings-brain-spike")
            || launchArgumentEnabled("open-plus-sheet")
            || launchArgumentEnabled("open-incognito")
            || launchArgumentEnabled("enable-agent-mode")
            || launchArgumentEnabled("open-agent-sheet")
            || launchArgumentEnabled("open-agent-hub")
            || launchArgumentEnabled("open-agent-detail")
            || launchArgumentEnabled("open-avatar-editor")
            || launchArgumentEnabled("open-new-workspace-file")
            || launchArgumentEnabled("open-workspace-file")
            || launchArgumentEnabled("open-voice-mode")
            || launchArgumentEnabled("open-voice-settings")
            || launchArgumentEnabled("open-code-thread")
            || launchArgumentEnabled("open-code-thread-id")
            || launchArgumentEnabled("open-code-filter")
            || launchArgumentEnabled("open-projects")
            || launchArgumentEnabled("open-project-detail")
            || launchArgumentEnabled("open-new-project")
            || launchArgumentEnabled("open-artifacts")
            || launchArgumentEnabled("open-browser-chat")
            || launchArgumentEnabled("open-connectors")
            || launchArgumentEnabled("open-automation")
            || launchArgumentEnabled("open-models")
            || launchArgumentEnabled("open-aci")
            || launchArgumentEnabled("brain-spike-auto")
            || launchArgumentEnabled("chat")
            || launchArgumentEnabled("code")
            || launchArgumentEnabled("browser")
        #else
        false
        #endif
    }
}

/// The sign-in screen: the native port of m.allternit.com's phone sign-in
/// (allternit-ai SignInLayout with the phone SignInStage and compact rail).
/// Top to bottom: A://TERNIT lockup and counter, the self-building preview
/// of Home / Bots / Code / ACI, its caption, then "Sign in to Allternit",
/// the sign-in buttons and the legal line. The page scrolls on small phones.
struct LoginGateView: View {
    @EnvironmentObject var auth: AuthManager
    @Environment(\.colorScheme) private var colorScheme

    var onSkipAuth: (() -> Void)? = nil

    private var palette: SignInPalette { SignInPalette(colorScheme) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                SignInStageView()

                rail
                    .padding(.top, 8)
            }
            .padding(.horizontal, 16)
            .padding(.top, 16)
            .padding(.bottom, 24)
            .frame(maxWidth: 480)
            .frame(maxWidth: .infinity)
        }
        .scrollBounceBehavior(.basedOnSize)
        .background(palette.page.ignoresSafeArea())
    }

    /// The compact sign-in rail (web .ast-rail-compact).
    private var rail: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Sign in to Allternit")
                .font(.system(size: 21, weight: .bold))
                .tracking(-0.42)
                .foregroundStyle(palette.ink)
                .padding(.bottom, 12)
                .accessibilityAddTraits(.isHeader)

            if !auth.isClerkConfigured {
                Text("Set CLERK_PUBLISHABLE_KEY in Info.plist to enable sign-in.")
                    .font(.caption)
                    .foregroundStyle(palette.sub)
                    .padding(.bottom, 10)
            }

            VStack(spacing: 10) {
                SignInWithAppleRow()

                ForEach(AuthManager.SocialProvider.allCases) { provider in
                    SignInSocialButton(provider: provider)
                }

                if let error = auth.socialSignInError {
                    Text(error)
                        .font(.system(size: 12.5))
                        .foregroundStyle(colorScheme == .dark ? Color(hex: "#F08A7C") : Color(hex: "#B42318"))
                        .multilineTextAlignment(.center)
                }

                orDivider

                Button(action: auth.signIn) {
                    Text("Continue with email")
                }
                .buttonStyle(SignInButtonStyle(kind: .ink))
            }
            .disabled(!auth.isClerkConfigured)
            .opacity(auth.isClerkConfigured ? 1 : 0.5)

            HStack(spacing: 4) {
                Text("New here?")
                    .foregroundStyle(palette.sub)
                Button(action: auth.signUp) {
                    Text("Create an account")
                        .fontWeight(.semibold)
                        .underline()
                        .foregroundStyle(palette.ink)
                }
                .buttonStyle(.plain)
                .disabled(!auth.isClerkConfigured)
            }
            .font(.system(size: 13))
            .frame(maxWidth: .infinity)
            .padding(.top, 12)

            #if DEBUG
            if let onSkipAuth {
                Button(action: {
                    let generator = UIImpactFeedbackGenerator(style: .light)
                    generator.impactOccurred()
                    onSkipAuth()
                }) {
                    Text("Continue without signing in")
                        .font(.subheadline)
                        .fontWeight(.medium)
                        .foregroundStyle(palette.sub)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                }
                .padding(.top, 4)
            }
            #endif

            Text("By continuing you agree to the [Terms](https://allternit.com/terms) and [Privacy Policy](https://allternit.com/privacy).")
                .font(.system(size: 11.5))
                .lineSpacing(2)
                .foregroundStyle(palette.mut)
                .tint(palette.ink)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.top, 18)
        }
    }

    /// "—— or ——" between the social buttons and email.
    private var orDivider: some View {
        HStack(spacing: 12) {
            Rectangle().fill(palette.line).frame(height: 1)
            Text("or")
                .font(.system(size: 12))
                .foregroundStyle(palette.mut)
            Rectangle().fill(palette.line).frame(height: 1)
        }
        .padding(.vertical, 2)
        .accessibilityHidden(true)
    }
}

/// The sign-in buttons' look (web .ast-btn): 44 pt, 11 pt corners, 14 pt
/// semibold. `outline` is the raised white button, `ink` the black one.
struct SignInButtonStyle: ButtonStyle {
    enum Kind { case outline, ink }
    let kind: Kind

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        let palette = SignInPalette(colorScheme)
        let pressed = configuration.isPressed
        let background: Color = switch kind {
        case .outline: pressed ? palette.fill : palette.raise
        case .ink: pressed ? palette.ink.opacity(0.88) : palette.ink
        }
        return configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(kind == .ink ? palette.page : palette.ink)
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(background))
            .overlay(
                RoundedRectangle(cornerRadius: 11, style: .continuous)
                    .strokeBorder(kind == .ink ? palette.ink : palette.line, lineWidth: 1)
            )
            .scaleEffect(pressed ? 0.985 : 1)
            .contentShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
    }
}

/// "Continue with GitHub" / "Continue with Google" through Clerk OAuth
/// (`AuthManager.signIn(with:)`).
private struct SignInSocialButton: View {
    let provider: AuthManager.SocialProvider
    @EnvironmentObject private var auth: AuthManager
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Button {
            auth.signIn(with: provider)
        } label: {
            HStack(spacing: 10) {
                if auth.signingInWith == provider {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    logo
                }
                Text("Continue with \(provider.title)")
            }
        }
        .buttonStyle(SignInButtonStyle(kind: .outline))
        .disabled(auth.signingInWith != nil)
        .accessibilityLabel("Continue with \(provider.title)")
    }

    @ViewBuilder
    private var logo: some View {
        switch provider {
        case .github:
            Image("LogoGitHub")
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(width: 18, height: 18)
                .foregroundStyle(SignInPalette(colorScheme).ink)
        case .google:
            Image("LogoGoogle")
                .resizable()
                .scaledToFit()
                .frame(width: 18, height: 18)
        }
    }
}

/// "Sign in with Apple" on the sign-in screen (App Store 4.8: the web offers
/// Google, so Apple sits beside it). Apple's HIG look: black in light mode,
/// white in dark, Apple logo + "Sign in with Apple". Clerk does the exchange
/// (`AuthManager.signInWithApple`).
private struct SignInWithAppleRow: View {
    @EnvironmentObject private var auth: AuthManager
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(spacing: 6) {
            Button(action: auth.signInWithApple) {
                HStack(spacing: 8) {
                    if auth.isSigningInWithApple {
                        ProgressView()
                            .tint(colorScheme == .dark ? .black : .white)
                    } else {
                        Image(systemName: "apple.logo")
                            .font(.system(size: 16, weight: .semibold))
                    }
                    Text("Sign in with Apple")
                        .font(.system(size: 15, weight: .semibold))
                }
                .foregroundColor(colorScheme == .dark ? .black : .white)
                .frame(maxWidth: .infinity)
                .frame(height: 44)
                .background(colorScheme == .dark ? Color.white : Color.black)
                .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
            }
            .accessibilityLabel("Sign in with Apple")

            if let error = auth.appleSignInError {
                Text(error)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
            }
        }
    }
}
