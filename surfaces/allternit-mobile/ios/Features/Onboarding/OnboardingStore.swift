import SwiftUI

/// Work-profile options for the onboarding radio grid (Phase 10, ChatGPT
/// macOS onboarding parity — "Which best describes your work?"). The raw
/// value is the wire id sent as `metadata.persona` on session create; the
/// label is what the grid renders.
enum OnboardingPersona: String, CaseIterable, Sendable {
    case engineering
    case design
    case finance
    case legal
    case dataScience = "data-science"
    case marketing
    case operations
    case student
    case product
    case sales
    case peopleHR = "people-hr"
    case other

    var label: String {
        switch self {
        case .engineering: return "Engineering"
        case .design: return "Design"
        case .finance: return "Finance"
        case .legal: return "Legal"
        case .dataScience: return "Data science"
        case .marketing: return "Marketing"
        case .operations: return "Operations"
        case .student: return "Student"
        case .product: return "Product"
        case .sales: return "Sales"
        case .peopleHR: return "People & HR"
        case .other: return "Other"
        }
    }
}

/// First-run setup state. Persisted app-wide (UserDefaults), mirroring
/// SettingsStore's pattern:
/// - `isComplete` gates the setup in AllternitApp (root swap before the
///   workspace). It is true once setup finished on this phone
///   (`completedOnDevice`, persisted) or the setup gate let an existing user
///   through for this launch only (`skippedThisLaunch`: the server could not
///   answer and a computer is paired — asked again next launch, like the web
///   phone). `-reset-onboarding` and Settings → Replay setup set `forceRun`.
/// - `persona` / `suggestTasks` are answers from the earlier "Hey there!"
///   onboarding (retired for the web phone setup); kept so existing answers
///   still reach `metadata.persona` and the home suggestion order.
/// - `pendingPrompt` is IN-MEMORY ONLY: a prompt for the composer to fill
///   once (fill-not-send, like the suggestion rows).
@MainActor
final class OnboardingStore: ObservableObject {
    static let shared = OnboardingStore()

    /// Setup finished on this phone (persisted).
    @Published private(set) var completedOnDevice: Bool {
        didSet { defaults.set(completedOnDevice, forKey: Keys.complete) }
    }
    /// The gate skipped setup for this launch only (not persisted).
    @Published private(set) var skippedThisLaunch = false
    /// Show setup without asking the server (Replay setup, `-reset-onboarding`).
    @Published private(set) var forceRun = false

    /// Onboarding gate — true once setup is finished, or skipped this launch.
    /// Setting false replays setup.
    var isComplete: Bool {
        get { completedOnDevice || skippedThisLaunch }
        set { if newValue { complete() } else { replay() } }
    }
    /// Work-profile radio-grid selection (nil until answered).
    @Published var persona: OnboardingPersona? {
        didSet { defaults.set(persona?.rawValue, forKey: Keys.persona) }
    }
    /// "Suggest personalized tasks" checkbox (on by default).
    @Published var suggestTasks: Bool {
        didSet { defaults.set(suggestTasks, forKey: Keys.suggestTasks) }
    }
    /// Starter-task prompt waiting to be filled into the composer.
    @Published var pendingPrompt: String? = nil

    private let defaults: UserDefaults

    private enum Keys {
        static let complete = "allternit-onboarding-complete"
        static let persona = "allternit-onboarding-persona"
        static let suggestTasks = "allternit-onboarding-suggest-tasks"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.completedOnDevice = defaults.bool(forKey: Keys.complete)
        self.persona = defaults.string(forKey: Keys.persona)
            .flatMap(OnboardingPersona.init(rawValue:))
        self.suggestTasks = defaults.object(forKey: Keys.suggestTasks) as? Bool ?? true
    }

    /// Marks setup done on this phone (finished, or the server says it is done).
    func complete() {
        forceRun = false
        completedOnDevice = true
    }

    /// The gate let an existing user through without setup, for this launch only.
    func skipThisLaunch() {
        skippedThisLaunch = true
    }

    /// Settings → Replay setup: the root swaps back to the first-run setup,
    /// which runs without waiting for the server's answer.
    func replay() {
        forceRun = true
        skippedThisLaunch = false
        completedOnDevice = false
    }

    /// `-reset-onboarding` (DEBUG): clears every onboarding flag and shows the
    /// first-run setup from its first step on this launch.
    func reset() {
        defaults.removeObject(forKey: Keys.complete)
        defaults.removeObject(forKey: Keys.persona)
        defaults.removeObject(forKey: Keys.suggestTasks)
        completedOnDevice = false
        skippedThisLaunch = false
        // DEBUG reset means "show setup on this launch", whatever the server says.
        forceRun = true
        persona = nil
        suggestTasks = true
        pendingPrompt = nil
    }
}
