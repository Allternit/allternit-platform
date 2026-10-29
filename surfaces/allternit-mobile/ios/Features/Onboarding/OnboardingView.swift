import SwiftUI

/// First-run setup gate on the iPhone — the m.allternit.com phone setup
/// (allternit-ai `fabric-session/phone/PhoneFirstRun.tsx` +
/// `OnboardingFlow.tsx` PhoneOnboardingFlow), shown after sign-in while
/// `OnboardingStore.isComplete` is false (root swap in AllternitApp — never
/// over LoginGateView). The setup itself is `FirstRunSetupView`
/// (Features/Onboarding/Setup): Where it runs · Style · Your AI · Meet your
/// bots · Everything connected.
///
/// Gate: the server's setup config decides (`user.onboardingComplete`). The
/// phone is careful when that check can't answer (no computer online to
/// relay it): it only shows setup for an account that has no computer
/// paired at all — an existing user whose computer is just asleep is never
/// stopped by setup on their phone. The check gives up after 8 s, so the
/// phone is never held on the splash.
///
/// DEBUG args:
/// - `-reset-onboarding` — clear the flags and run setup on this launch
///   (handled in AllternitApp via `OnboardingStore.reset()`).
/// - `-open-onboarding` — force-show setup regardless of the flags.
struct OnboardingView: View {
    @ObservedObject private var store = OnboardingStore.shared

    @State private var server: SetupServerAnswer = .pending
    @State private var runtimesLoaded = false
    @State private var runtimeCount = 0

    private var forced: Bool {
        #if DEBUG
        if launchArgumentEnabled("open-onboarding") { return true }
        #endif
        return store.forceRun
    }

    /// Someone has used the app on this phone before (the mode store's key,
    /// the same key the web reads in localStorage).
    private var usedAppBefore: Bool {
        let d = UserDefaults.standard
        return d.string(forKey: AppModeStore.storageKey) != nil || d.string(forKey: AppModeStore.shellStorageKey) != nil
    }

    private var decision: PhoneSetupDecision {
        if forced { return .run }
        return decidePhoneSetup(completedLocally: store.completedOnDevice, usedAppBefore: usedAppBefore,
                                server: server, runtimesLoaded: runtimesLoaded, runtimeCount: runtimeCount)
    }

    var body: some View {
        ZStack {
            switch decision {
            case .run:
                FirstRunSetupView(onFinish: finish)
                    .transition(.opacity)
            case .checking, .skip:
                splash
            }
        }
        .animation(.easeOut(duration: 0.25), value: decision == .run)
        .task { await check() }
        .task { await loadRuntimes() }
        .onChange(of: decision) { _, d in skipIfDecided(d) }
    }

    private var splash: some View {
        ZStack {
            SetupPalette.bg.ignoresSafeArea()
            AllternitOrbView(state: .idle, size: 56)
        }
        .accessibilityElement()
        .accessibilityLabel("Loading")
        .accessibilityAddTraits(.updatesFrequently)
    }

    // MARK: - Gate

    private func check() async {
        guard !forced else { return }
        // Never hold the phone on a splash: if the config check hangs, skip setup.
        let answer: SetupServerAnswer
        do {
            let complete = try await setupWithTimeout(8) { try await SetupAPI.onboardingComplete() }
            answer = complete ? .done : .run
        } catch {
            answer = .failed
        }
        server = answer
        if answer == .done { store.complete() }
    }

    /// The paired-computer list, settled after 6 s whatever it has.
    private func loadRuntimes() async {
        let relay = RuntimeRelayStore.shared
        let deadline = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 6_000_000_000)
            runtimesLoaded = true
        }
        await relay.refresh()
        runtimeCount = relay.devices.count
        runtimesLoaded = true
        deadline.cancel()
    }

    private func skipIfDecided(_ d: PhoneSetupDecision) {
        // An existing user goes straight in; the server is asked again next launch.
        if d == .skip && !store.isComplete { store.skipThisLaunch() }
    }

    // MARK: - Finish (web completeFirstRun, phone)

    /// Saves the setup config with the wizard-completion flag (best-effort —
    /// offline must not trap the user), applies the default brain to the
    /// model picker when it is in the catalog, completes setup on this phone,
    /// and queues a Settings fix picked on the checklist (or Billing, when a
    /// cloud computer was chosen on the free plan) for the workspace to open.
    private func finish(_ state: SetupState, _ data: SetupWizardData, _ fix: String?) async {
        try? await SetupAPI.saveConfig(defaultModel: data.defaultModel, mode: data.infraType)
        if let wanted = data.defaultModel {
            let models = ModelStore.shared.models
            let match = models.first { $0.id == wanted }
                ?? data.defaultModelId.flatMap { id in models.first { $0.id == id || $0.id.hasSuffix("/\(id)") } }
            if let match { ModelStore.shared.userSelectedModel(match.id) }
        }
        store.complete()
        if let fix {
            // The Settings screens exist once the workspace is up; DeepLinkHost opens it.
            DeepLinkRouter.shared.open(path: fix == "computers" ? "/computers" : "/settings/\(fix)")
        }
    }
}
