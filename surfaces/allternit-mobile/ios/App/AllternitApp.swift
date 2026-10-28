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
                } else if newPhase == .background {
                    BackgroundRefreshManager.scheduleNextRefresh()
                }
            }
            .onOpenURL { url in
                openedDocumentURL = url
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

struct LoginGateView: View {
    @EnvironmentObject var auth: AuthManager

    var onSkipAuth: (() -> Void)? = nil

    @State private var logoGlowing = false

    var body: some View {
        VStack(spacing: 32) {
            Spacer()

            // Logo with ambient glow matching LaunchHeader
            ZStack {
                Circle()
                    .fill(Color("AccentPrimary").opacity(logoGlowing ? 0.12 : 0.04))
                    .frame(width: 140, height: 140)
                    .blur(radius: 35)
                    .animation(.easeInOut(duration: 2.5).repeatForever(autoreverses: true), value: logoGlowing)

                WordmarkView(height: 28)
            }

            Text("Your native workspace for autonomous AI execution.")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)

            Spacer()

            if !auth.isClerkConfigured {
                Text("Set CLERK_PUBLISHABLE_KEY in Info.plist to enable sign-in.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
            }

            VStack(spacing: 12) {
                Button(action: auth.signIn) {
                    Text("Sign In")
                        .font(.headline)
                        .foregroundColor(.white)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 16)
                        .background(Color("AccentPrimary"))
                        .cornerRadius(12)
                }

                Button(action: auth.signUp) {
                    Text("Create Account")
                        .font(.headline)
                        .foregroundColor(Color("AccentPrimary"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 16)
                        .background(
                            RoundedRectangle(cornerRadius: 12)
                                .stroke(Color("AccentPrimary"), lineWidth: 1)
                        )
                }
            }
            .disabled(!auth.isClerkConfigured)
            .opacity(auth.isClerkConfigured ? 1 : 0.5)
            .padding(.horizontal, 32)

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
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                }
                .padding(.horizontal, 32)
                .padding(.bottom, 64)
            }
            #endif
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .onAppear { logoGlowing = true }
    }
}
