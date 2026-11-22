package com.allternit.feature.chat

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/**
 * A typed chat or generation failure (design pick: errors B): a plain reason, the machine code when
 * the server sent one, the usage-limit reset time, and which way out to offer. The Kotlin twin of iOS
 * `ChatErrorInfo` over the chat error frame `{type:"error", error, code?, reset_at?}` (platform #2807).
 */
data class ChatErrorInfo(
    val kind: Kind,
    /** One or two plain sentences: what happened, in the server's words when it said. */
    val reason: String,
    val code: String? = null,
    /** Usage limit: when it resets (epoch milliseconds). */
    val resetAtMs: Long? = null,
    val isChatGPT: Boolean = false,
    val action: Action? = null,
) {
    enum class Kind { UsageLimit, SignedOut, Network, Server }
    enum class Action { Connect, PickModel, Retry }
    /** Which surface the failure belongs to. */
    enum class Lane { Picture, Motion, Chat }

    /** The card title for an error on a lane ("ChatGPT usage limit — resets at 3:40 PM"). */
    fun title(lane: Lane, nowMs: Long = System.currentTimeMillis(), zone: ZoneId = ZoneId.systemDefault(), locale: Locale = Locale.getDefault()): String = when (kind) {
        Kind.UsageLimit -> {
            val head = if (isChatGPT) "ChatGPT usage limit" else "Usage limit reached"
            formatReset(resetAtMs, nowMs, zone, locale)?.let { "$head — resets at $it" } ?: head
        }
        Kind.SignedOut -> if (isChatGPT) "ChatGPT is not signed in" else "Sign-in needed"
        else -> when (lane) {
            Lane.Picture -> "Picture not made"
            Lane.Motion -> "Clip not made"
            Lane.Chat -> if (kind == Kind.Network) "Couldn't reach Allternit" else "Reply not finished"
        }
    }

    /** The button label for the way out (null: none). */
    val actionLabel: String?
        get() = when (action) {
            Action.Connect -> if (isChatGPT) "Connect ChatGPT" else "Open Connections"
            Action.PickModel -> "Pick another model"
            Action.Retry -> "Try again"
            null -> null
        }

    companion object {
        val usageLimitCodes = setOf(
            "subscription_sharing_usage_limit_exceeded", "subscription_sharing_usage_unavailable",
            "usage_limit_reached", "usage_limit_exceeded", "usage_limit", "plan_rate", "vendor_limited",
        )
        val signedOutCodes = setOf(
            "signed_out", "login_expired", "not_signed_in", "signed_out_allternit",
            "subscription_sharing_invalid_user", "chatpass_v2_scope_not_authorized",
            "chatpass_v2_invalid_authorization_context",
        )
        val networkCodes = setOf("network", "stream_ended")

        private val usageLimitText = Regex("""\b(?:usage limit|limit reached|limit is used up)\b""", RegexOption.IGNORE_CASE)
        private val signedOutText = Regex("""\b(?:gizzi connect(?: chatgpt)?|sign-in expired|not signed in|did not accept your sign-in|sign in again)\b""", RegexOption.IGNORE_CASE)
        private val networkText = Regex("""^(?:failed to fetch|fetch failed|networkerror|network error|load failed)\b|\bcould not reach\b|\bcouldn't reach\b""", RegexOption.IGNORE_CASE)
        private val bareNetwork = Regex("""^(?:failed to fetch|fetch failed|load failed)$""", RegexOption.IGNORE_CASE)

        /**
         * The typed error for anything a chat turn or a generation can fail with. Unknown errors keep the
         * server's message; only an error with no words at all falls back to a sentence.
         */
        fun make(message: String?, code: String? = null, status: Int? = null, resetAtMs: Long? = null, lane: Lane = Lane.Chat): ChatErrorInfo {
            val text = message.orEmpty().trim()
            val lowered = code?.lowercase()
            val chatgpt = (lowered?.let { it.startsWith("subscription_sharing") || it.startsWith("chatpass") } ?: false) ||
                text.contains("chatgpt", ignoreCase = true)
            fun base(kind: Kind, reason: String, action: Action) = ChatErrorInfo(kind, reason, code, null, chatgpt, action)
            if ((lowered != null && lowered in usageLimitCodes) || status == 429 || usageLimitText.containsMatchIn(text)) {
                val reason = text.ifEmpty { if (chatgpt) "ChatGPT will not take more right now." else "The usage limit is reached for now." }
                return base(Kind.UsageLimit, reason, Action.PickModel).copy(resetAtMs = resetAtMs)
            }
            if ((lowered != null && lowered in signedOutCodes) || status == 401 || signedOutText.containsMatchIn(text)) {
                return base(Kind.SignedOut, signedOutReason(text, chatgpt, lane), Action.Connect)
            }
            if ((lowered != null && lowered in networkCodes) || networkText.containsMatchIn(text)) {
                val reason = if (text.isEmpty() || bareNetwork.matches(text)) "Couldn't reach Allternit. Check your connection and try again." else text
                return base(Kind.Network, reason, Action.Retry)
            }
            return base(Kind.Server, text.ifEmpty { "Something went wrong. Try again." }, Action.Retry)
        }

        /** The words for a sign-in error: the CLI command means nothing in the app. */
        private fun signedOutReason(text: String, chatgpt: Boolean, lane: Lane): String {
            if (text.isNotEmpty() && !text.contains("gizzi connect", ignoreCase = true)) return text
            if (!chatgpt) return "Sign in again to keep going."
            return if (lane == Lane.Chat) "Sign in to ChatGPT on this phone to keep going." else "Sign in to ChatGPT on this phone to make pictures with it."
        }

        /** "3:40 PM" today, "Tue 3:40 PM" on another day; null when unknown or past. */
        fun formatReset(resetAtMs: Long?, nowMs: Long = System.currentTimeMillis(), zone: ZoneId = ZoneId.systemDefault(), locale: Locale = Locale.getDefault()): String? {
            if (resetAtMs == null || resetAtMs <= nowMs) return null
            val at = Instant.ofEpochMilli(resetAtMs).atZone(zone)
            val time = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale).format(at)
            if (at.toLocalDate() == Instant.ofEpochMilli(nowMs).atZone(zone).toLocalDate()) return time
            return "${DateTimeFormatter.ofPattern("EEE", locale).format(at)} $time"
        }
    }
}
