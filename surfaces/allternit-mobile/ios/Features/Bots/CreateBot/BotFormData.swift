import Foundation

// Copied from allternit-ai src/views/agent-view/components/create-bot:
// starter-prompt-suggestions.ts, bot-tool-registry.ts, vm-operator.ts, bot-profile.ts,
// bot-brain.ts. Same lists, same wording.
enum BotFormData {
    static let starterPromptMax = 5

    static let starterPrompts: [String: [String]] = [
        "research": [
            "Summarize the latest news on…",
            "Compare these three sources and list what they disagree on",
            "Research the market for… and give me a one-page brief",
            "Find five credible sources about…",
            "Track this topic weekly and flag what changed",
            "Turn these notes into an annotated reading list",
            "What are the strongest arguments against…?",
            "Build a timeline of… with sources",
        ],
        "code": [
            "Review this diff and flag anything risky",
            "Write a failing test for this function, then make it pass",
            "Explain this stack trace and suggest a fix",
            "Refactor this file for readability, keep behavior identical",
            "Generate a migration script for…",
            "Find the bug: here's the code and the error…",
            "Draft a README section for…",
            "Set up a CI check that runs…",
        ],
        "writing": [
            "Rewrite this email to be shorter and direct",
            "Draft a blog post outline about…",
            "Edit this paragraph for Register 1: plain, no hype",
            "Write three headline options for…",
            "Turn these bullet points into a one-page brief",
            "Proofread this and list every factual claim to verify",
            "Continue this draft in the same voice",
            "Summarize this article in five sentences",
        ],
        "data": [
            "Chart this CSV and call out the outliers",
            "Clean this dataset and report what you dropped",
            "Join these two tables on… and summarize the result",
            "Build a weekly report from these numbers",
            "What trends do you see in this data?",
            "Convert this spreadsheet to a formatted table",
            "Run a sanity check on these figures",
            "Explain this SQL query line by line",
        ],
        "sales": [
            "Draft a follow-up email for this lead",
            "Score these leads and tell me who to call first",
            "Research this company before my call — bullet points",
            "Write a one-paragraph pitch for…",
            "Summarize this call transcript into CRM notes",
            "Find 10 prospects that match our ICP",
            "Draft outreach for… in a plain, direct tone",
            "Review this proposal for anything that overpromises",
        ],
        "design": [
            "Critique this landing page layout",
            "Suggest a color palette for…",
            "Rewrite this UI copy to be clearer",
            "Turn this wireframe description into a spec",
            "Audit this page for accessibility issues",
            "Generate three layout options for…",
            "What would make this form feel shorter?",
            "Compare our homepage to these two competitors",
        ],
        "ops": [
            "Check this process for steps we can automate",
            "Draft a runbook for…",
            "Summarize today's failed jobs and what to retry",
            "Watch this inbox and route messages to the right owner",
            "Prepare the weekly status report from these updates",
            "Reconcile this inventory list against the system of record",
            "Flag anything in this log that needs action today",
            "Write the onboarding checklist for…",
        ],
        "custom": [
            "What can you help me with?",
            "Summarize my last session",
            "Walk me through how you work",
            "Remember this for next time…",
            "What should I set up first?",
            "Give me a quick status update",
        ],
        "default": [
            "What can you help me with?",
            "Summarize the last conversation",
            "Walk me through how you work",
            "Remember this for next time…",
            "Draft a starting point for…",
            "Review this and list what needs attention",
        ],
    ]

    static func suggestions(for category: String) -> [String] {
        starterPrompts[category] ?? starterPrompts["default"] ?? []
    }

    /// QuickCreateStep ACCENTS (Create bot on a phone).
    static let createAccents = ["#B08D6E", "#6366f1", "#8b5cf6", "#ec4899", "#f97316", "#10b981", "#06b6d4", "#3b82f6", "#64748b"]
    /// IdentityStep ACCENT_COLORS (Edit bot).
    static let editAccents = ["#B08D6E", "#6366f1", "#8b5cf6", "#ec4899", "#f43f5e", "#f97316", "#f59e0b", "#10b981", "#06b6d4", "#0ea5e9", "#3b82f6", "#64748b"]

    /// BOT_CATEGORIES.
    static let categories: [(id: String, label: String)] = [
        ("research", "Research"), ("code", "Code"), ("writing", "Writing"), ("data", "Data"),
        ("sales", "Sales"), ("design", "Design"), ("ops", "Operations"), ("custom", "Custom"),
    ]

    /// BOT_NATIVE_TOOLS.
    static let tools: [(id: String, label: String)] = [
        ("web_search", "Web search"), ("web_fetch", "Web fetch"), ("file_read", "File read"),
        ("file_write", "File write"), ("code_execution", "Code execution"), ("memory", "Memory"),
        ("str_replace_editor", "Editor"), ("computer", "Computer"),
    ]

    /// BOT_DESKTOP_PRESETS.
    struct DesktopPreset: Identifiable {
        let id: String, label: String, cpu: String, memory: String, disk: String
        /// describeDesktopResources.
        var detail: String { "\(cpu) vCPU · \((Int(memory) ?? 4096) / 1024) GB RAM · \((Int(disk) ?? 20480) / 1024) GB disk" }
    }
    static let desktopPresets = [
        DesktopPreset(id: "small", label: "Small", cpu: "2", memory: "4096", disk: "20480"),
        DesktopPreset(id: "medium", label: "Medium", cpu: "4", memory: "8192", disk: "40960"),
        DesktopPreset(id: "large", label: "Large", cpu: "8", memory: "16384", disk: "81920"),
    ]

    /// HARNESS_MODES and PROVIDER_OPTIONS (ComputerRuntimeStep).
    static let harnessModes: [(id: String, label: String)] = [("cloud", "Cloud"), ("byok", "BYOK"), ("local", "Local"), ("subprocess", "Subprocess")]
    static let providers: [(id: String, label: String)] = [("openai", "OpenAI"), ("anthropic", "Anthropic"), ("google", "Google"), ("local", "Local"), ("custom", "Custom")]

    /// NATIVE_HARNESS_IDS with NATIVE_HARNESS_LABEL.
    static let nativeHarnesses: [(id: String, label: String)] = [
        ("codex", "Codex"), ("claude", "Claude"), ("kimi", "Kimi"), ("grok", "Grok"), ("hermes", "Hermes"),
        ("kiro-cli", "Kiro"), ("mcode", "MiniMax Code"), ("qodercli", "Qoder"), ("qoderclicn", "Qoder CN"),
        ("qwenpaw", "QwenPaw"), ("reasonix", "Reasonix"), ("traecli", "Trae"),
    ]

    /// BOT_BRAIN_MODE_LABEL with BotBrainBindFields MODE_HINT.
    static let brainModes: [(id: String, label: String, hint: String)] = [
        ("allternit_cloud", "Allternit cloud", "Runs on Allternit cloud. Model is the one you pick below."),
        ("native_harness", "Native harness", "Resumes a local Codex, Claude, or Kimi session. Does not fall back to cloud."),
        ("uhp_harness", "UHP harness", "Binds a UHP harness id. Does not fall back to cloud."),
    ]
}
