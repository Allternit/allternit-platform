package com.allternit.feature.shell.firstrun

import com.allternit.core.design.ThinkingOrbState

/*
 * First-run setup: the pure model behind Gizzi + A:// (iOS Features/Onboarding/Setup/SetupModel.swift,
 * itself a port of allternit-ai `setup/setupModel.ts`). No Compose and no network, so the step
 * machine, the result cards and the progress blocks stay plain data. Every card line is built
 * from what ACTUALLY happened (an outcome reported by the real connect calls), never from the
 * choice alone.
 *
 * Day-one order is connect kit option A (Eoj 2026-11-13): Your AI, Your phone, Main bot,
 * Connected. iOS keeps `where` and `style` in its model for older layouts but never asks them;
 * Android leaves them out (they have no screen on either platform).
 */

internal enum class SetupStep(val label: String) {
    Ai("Your AI"), Phone("Your phone"), MainBot("Main bot"), Connected("Connected"),
}

/**
 * iOS `setupStepOrder(mainBot:)`. The Main bot step is "Meet your bots"; Android passes false
 * until MeetYourBotsView is ported (PARITY-PROGRAM.md wave B), which is the order iOS runs
 * when `setupMainBotStepEnabled` is off.
 */
internal fun setupStepOrder(mainBot: Boolean): List<SetupStep> =
    if (mainBot) listOf(SetupStep.Ai, SetupStep.Phone, SetupStep.MainBot, SetupStep.Connected)
    else listOf(SetupStep.Ai, SetupStep.Phone, SetupStep.Connected)

/** Android has no Meet-your-bots screen yet, so the Main bot step is not in its order. */
internal const val SETUP_MAIN_BOT_STEP_ENABLED = false

internal enum class SetupField(val title: String) { Ai("Your AI"), Phone("Your phone") }

/** iOS BrainChoice (the rows the graph and the connected view know by name). */
internal enum class BrainChoice(val raw: String, val label: String) {
    SubsChatgpt("subs-chatgpt", "ChatGPT (your subscription)"),
    SubsClaude("subs-claude", "Claude (your subscription)"),
    SubsKimi("subs-kimi", "Kimi (your subscription)"),
    PhoneApps("phone-apps", "Apps on your phone"),
    Allternit("allternit", "Allternit Cloud"),
    Claude("claude", "Claude"),
    Chatgpt("chatgpt", "ChatGPT"),
    Gemini("gemini", "Gemini"),
    Local("local", "Local model"),
    Other("other", "Other brain");

    val isSubscription: Boolean get() = raw.startsWith("subs-")
    val subscriptionProvider: String? get() = if (isSubscription) raw.removePrefix("subs-") else null
    val shortName: String get() = label.replace(" (your subscription)", "")

    companion object {
        val subscriptions = listOf(SubsChatgpt, SubsClaude, SubsKimi)

        /** Which brain row a saved provider id belongs to (iOS forProvider). */
        fun forProvider(provider: String?): BrainChoice? {
            if (provider.isNullOrEmpty()) return null
            val p = provider.lowercase()
            if (p.startsWith("subs-")) return entries.firstOrNull { it.raw == p } ?: Other
            if (p == "anthropic" || p == "claude-cli" || p.startsWith("claude")) return Claude
            if (p == "openai" || p == "codex-cli" || p.startsWith("codex") || p.startsWith("gpt")) return Chatgpt
            if (p == "google" || p == "antigravity" || p.startsWith("gemini")) return Gemini
            if (p in setOf("ollama", "lmstudio", "sidecar", "local")) return Local
            if (p.startsWith("allternit")) return Allternit
            return Other
        }
    }
}

/** Status of the real action behind a choice. */
internal enum class OutcomeStatus {
    Idle, Working, Detected, Paired, Requested, Waiting, Connected, Set, NotFound, Error;

    val isFinished: Boolean get() = this == Paired || this == Connected || this == Set
}

internal data class StepOutcome(val status: OutcomeStatus, val label: String? = null, val detail: List<String> = emptyList())

internal data class SetupState(
    val order: List<SetupStep> = setupStepOrder(SETUP_MAIN_BOT_STEP_ENABLED),
    val step: SetupStep = order.first(),
    val ai: BrainChoice? = null,
    val outcomes: Map<SetupField, StepOutcome> = emptyMap(),
    val skipped: Map<SetupField, Boolean> = emptyMap(),
    /** Main bot made/picked in the Meet-your-bots step, when one really was. */
    val mainBotId: String? = null,
    /** Incremented on every choice: keys the haptic. */
    val ripple: Int = 0,
) {
    fun index(of: SetupStep): Int = order.indexOf(of)

    val field: SetupField? get() = if (step == SetupStep.Ai) SetupField.Ai else null

    /** Whether Next is allowed from the current step. */
    val canAdvance: Boolean
        get() = when (step) {
            SetupStep.MainBot, SetupStep.Phone -> true
            SetupStep.Ai -> ai != null
            SetupStep.Connected -> false
        }

    /** The step after the current one is the connected view. */
    val nextIsConnected: Boolean
        get() {
            val i = index(step)
            return i >= 0 && i + 1 < order.size && order[i + 1] == SetupStep.Connected
        }
}

internal sealed interface SetupAction {
    data class ChooseAI(val brain: BrainChoice) : SetupAction
    /** Sets the brain without a ripple; null clears it (a cancelled connect leaves no brain). */
    data class SetAIQuietly(val brain: BrainChoice?) : SetupAction
    data class Outcome(val field: SetupField, val outcome: StepOutcome) : SetupAction
    data class MainBot(val id: String?) : SetupAction
    data object Next : SetupAction
    data object Back : SetupAction
    data object Skip : SetupAction

    /** The haptic fires ONLY when a setup choice is picked, never on Next, Back or Skip. */
    val firesHaptic: Boolean get() = this is ChooseAI
}

internal object SetupReducer {
    fun reduce(s: SetupState, action: SetupAction): SetupState = when (action) {
        is SetupAction.ChooseAI -> s.copy(
            ai = action.brain,
            // A new choice discards the previous choice's outcome.
            outcomes = if (s.ai != action.brain) s.outcomes - SetupField.Ai else s.outcomes,
            skipped = s.skipped + (SetupField.Ai to false),
            ripple = s.ripple + 1,
        )
        is SetupAction.SetAIQuietly -> s.copy(
            ai = action.brain,
            outcomes = if (s.ai != action.brain) s.outcomes - SetupField.Ai else s.outcomes,
        )
        is SetupAction.Outcome -> if (s.outcomes[action.field] == action.outcome) s
        else s.copy(outcomes = s.outcomes + (action.field to action.outcome))
        is SetupAction.MainBot -> s.copy(mainBotId = action.id)
        SetupAction.Next -> if (!s.canAdvance) s
        else s.copy(step = s.order[minOf(s.index(s.step) + 1, s.order.size - 1)])
        SetupAction.Skip -> {
            val i = s.index(s.step)
            // Skipping keeps a real outcome that already happened but marks the step answered.
            val skipped = s.field?.let { s.skipped + (it to (s.ai == null)) } ?: s.skipped
            s.copy(step = s.order[minOf(i + 1, s.order.size - 1)], skipped = skipped)
        }
        SetupAction.Back -> {
            val i = s.index(s.step)
            if (i > 0) s.copy(step = s.order[i - 1]) else s
        }
    }
}

// Progress

internal const val SETUP_PROGRESS_BLOCKS = 30

internal enum class ProgressBlock { Off, On, Lead }

internal fun progressBlocks(step: SetupStep, order: List<SetupStep>): List<ProgressBlock> {
    val i = order.indexOf(step)
    val lit = if (i < 0) 0 else ((i + 1) * SETUP_PROGRESS_BLOCKS) / order.size
    return (0 until SETUP_PROGRESS_BLOCKS).map { b ->
        when {
            b < lit - 1 -> ProgressBlock.On
            b == lit - 1 -> if (lit == SETUP_PROGRESS_BLOCKS) ProgressBlock.On else ProgressBlock.Lead
            else -> ProgressBlock.Off
        }
    }
}

/** A://'s state while Gizzi asks (iOS orbState). iOS `.thinking` is the Android orb's Breathing. */
internal fun orbState(step: SetupStep, busy: Boolean): ThinkingOrbState = when {
    busy -> ThinkingOrbState.Working
    step == SetupStep.Ai -> ThinkingOrbState.Breathing
    step == SetupStep.MainBot -> ThinkingOrbState.Working
    else -> ThinkingOrbState.Connecting
}

/** The word next to A:// (iOS `orb.rawValue`). */
internal fun orbWord(step: SetupStep, busy: Boolean): String = when {
    busy -> "working"
    step == SetupStep.Ai -> "thinking"
    step == SetupStep.MainBot -> "working"
    else -> "idle"
}

internal object SetupCopy {
    fun question(field: SetupField): String = when (field) {
        SetupField.Ai -> "Hi, I'm Gizzi. Which AI should I think with? If you already pay for one, use it here."
        SetupField.Phone -> "Now put me on your phone, so you can talk to me from anywhere."
    }
}

// Result cards

internal enum class ChipTone { Ok, Warn, Soft, Error }

internal data class ResultCard(
    val field: SetupField,
    val title: String,
    val summary: String,
    val chip: String,
    val tone: ChipTone,
    val lines: List<String>,
    /** Mark the last line done (✓): only when the action really finished. */
    val done: Boolean,
)

private fun chip(status: OutcomeStatus): Pair<String, ChipTone> = when (status) {
    OutcomeStatus.Idle -> "Chosen" to ChipTone.Soft
    OutcomeStatus.Working -> "Working" to ChipTone.Warn
    OutcomeStatus.Detected -> "Detected" to ChipTone.Warn
    OutcomeStatus.Paired -> "Paired" to ChipTone.Ok
    OutcomeStatus.Requested -> "Requested" to ChipTone.Warn
    OutcomeStatus.Waiting -> "Waiting" to ChipTone.Warn
    OutcomeStatus.Connected -> "Connected" to ChipTone.Ok
    OutcomeStatus.Set -> "Set" to ChipTone.Ok
    OutcomeStatus.NotFound -> "Not found" to ChipTone.Error
    OutcomeStatus.Error -> "Error" to ChipTone.Error
}

internal fun resultCard(state: SetupState, field: SetupField): ResultCard? {
    val outcome = state.outcomes[field]
    val chosen = field == SetupField.Ai && state.ai != null
    if (!chosen && outcome == null) {
        if (state.skipped[field] != true) return null
        return ResultCard(field, field.title, "Skipped", "Skipped", ChipTone.Soft,
            listOf("Skipped — change it any time in Settings"), done = false)
    }
    val name = outcome?.label ?: when (field) {
        SetupField.Ai -> state.ai?.label ?: "—"
        SetupField.Phone -> "Your phone"
    }
    val status = outcome?.status ?: OutcomeStatus.Idle
    val (chipText, tone) = chip(status)
    val lines = outcome?.detail?.takeIf { it.isNotEmpty() } ?: defaultLines(field, name, status)
    return ResultCard(field, field.title, name, chipText, tone, lines, done = status.isFinished)
}

private fun defaultLines(field: SetupField, name: String, status: OutcomeStatus): List<String> = when (field) {
    SetupField.Ai -> when (status) {
        OutcomeStatus.Connected -> listOf("Default brain: $name", "Switch any time from the model picker")
        OutcomeStatus.Working -> listOf("Connecting $name …")
        OutcomeStatus.Error -> listOf("Could not connect $name")
        else -> listOf("Chose $name", "Connect it below, or later in Settings › Connections")
    }
    SetupField.Phone -> if (status == OutcomeStatus.Connected) listOf("$name · signed in")
    else listOf("Add your phone any time in Settings › Connections")
}

/** The stacked log of completed steps shown above the current question. */
internal fun completedCards(state: SetupState): List<ResultCard> {
    val now = state.index(state.step)
    return SetupField.entries.mapNotNull { f ->
        val step = if (f == SetupField.Ai) SetupStep.Ai else SetupStep.Phone
        val i = state.index(step)
        if (i < 0 || i >= now) return@mapNotNull null
        resultCard(state, f) ?: ResultCard(f, f.title, "Skipped", "Skipped", ChipTone.Soft, emptyList(), false)
    }
}

// Gate (allternit-ai PhoneFirstRun.tsx decidePhoneSetup)

internal enum class PhoneSetupDecision { Checking, Run, Skip }

internal enum class SetupServerAnswer { Pending, Run, Done, Failed }

/**
 * The server's setup config decides; when it can't answer (no computer online to relay it),
 * setup only shows for an account with no computer paired at all, so an existing user whose
 * computer is just asleep is never stopped by setup on their phone.
 */
internal fun decidePhoneSetup(
    completedLocally: Boolean,
    usedAppBefore: Boolean,
    server: SetupServerAnswer,
    runtimesLoaded: Boolean,
    runtimeCount: Int,
): PhoneSetupDecision = when (server) {
    SetupServerAnswer.Pending -> PhoneSetupDecision.Checking
    SetupServerAnswer.Done -> PhoneSetupDecision.Skip
    SetupServerAnswer.Run -> if (completedLocally) PhoneSetupDecision.Skip else PhoneSetupDecision.Run
    SetupServerAnswer.Failed -> when {
        completedLocally || usedAppBefore -> PhoneSetupDecision.Skip
        !runtimesLoaded -> PhoneSetupDecision.Checking
        runtimeCount == 0 -> PhoneSetupDecision.Run
        else -> PhoneSetupDecision.Skip
    }
}

/** What the real flows set while setup runs (iOS SetupWizardData). */
internal data class SetupWizardData(
    /** `local` (a computer you own) or `cloud`: recorded as the wizard's run mode. */
    val infraType: String = "local",
    val defaultProvider: String? = null,
    val defaultModelId: String? = null,
) {
    /** `provider/model`, or the provider alone (web completeFirstRun). */
    val defaultModel: String?
        get() {
            val p = defaultProvider ?: return null
            return defaultModelId?.let { "$p/$it" } ?: p
        }
}
