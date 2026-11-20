package com.allternit.feature.system

import org.json.JSONObject

/**
 * The runtime-owned `status.snapshot` as cloud-api `GET /api/v1/status/summary` returns it (iOS
 * `SystemSurfaces/StatusSnapshot`). Every field is optional so an older or partial payload still parses.
 */
data class StatusSnapshot(
    val activeBots: Int = 0,
    val attentionBots: Int = 0,
    val pendingApprovals: Int = 0,
    val paused: Boolean = false,
    val focus: Focus? = null,
    val bots: List<Bot> = emptyList(),
    val needsYou: Int = 0,
    val lastRoutine: LastRoutine? = null,
    /** Set locally when the snapshot is stored; not part of the wire payload. */
    val asOfMillis: Long = 0L,
) {
    data class Focus(
        val botId: String = "",
        val botName: String = "",
        val status: String = "",
        val step: String = "",
        val progress: Double? = null,
        val inboxId: String? = null,
        val sessionId: String? = null,
        val question: String? = null,
        val contentHash: String? = null,
        val approveInline: Boolean = false,
        val isPrivate: Boolean = false,
    )

    data class Bot(val botId: String, val name: String, val status: String, val step: String)

    data class LastRoutine(val title: String, val status: String)

    /** iOS `Copy.needsCount`. */
    val needsCount: Int get() = maxOf(needsYou, pendingApprovals)

    companion object {
        fun parse(json: String, nowMillis: Long = System.currentTimeMillis()): StatusSnapshot? = runCatching {
            // The route may wrap the snapshot (`{ snapshot: {...} }`); take either shape.
            val root = JSONObject(json)
            val o = root.optJSONObject("snapshot") ?: root
            val focus = o.optJSONObject("focus")?.let { f ->
                Focus(
                    botId = f.optString("botId"),
                    botName = f.optString("botName"),
                    status = f.optString("status"),
                    step = f.optString("step"),
                    progress = if (f.has("progress") && !f.isNull("progress")) f.optDouble("progress") else null,
                    inboxId = f.optString("inboxId").takeIf { it.isNotBlank() },
                    sessionId = f.optString("sessionId").takeIf { it.isNotBlank() },
                    question = f.optString("question").takeIf { it.isNotBlank() },
                    contentHash = f.optString("contentHash").takeIf { it.isNotBlank() },
                    approveInline = f.optBoolean("approveInline", false),
                    isPrivate = f.optBoolean("private", false),
                )
            }?.takeIf { it.botName.isNotBlank() || it.step.isNotBlank() }
            val bots = o.optJSONArray("bots")?.let { a ->
                (0 until a.length()).mapNotNull { i ->
                    val b = a.optJSONObject(i) ?: return@mapNotNull null
                    val id = b.optString("botId").ifBlank { return@mapNotNull null }
                    Bot(id, b.optString("name"), b.optString("status"), b.optString("step"))
                }
            }.orEmpty()
            val routine = o.optJSONObject("lastRoutine")?.let { LastRoutine(it.optString("title"), it.optString("status")) }
            StatusSnapshot(
                activeBots = o.optInt("activeBots", 0),
                attentionBots = o.optInt("attentionBots", 0),
                pendingApprovals = o.optInt("pendingApprovals", 0),
                paused = o.optBoolean("paused", false),
                focus = focus,
                bots = bots,
                needsYou = o.optJSONArray("needsYou")?.length() ?: 0,
                lastRoutine = routine,
                asOfMillis = nowMillis,
            )
        }.getOrNull()
    }
}

/** The widget's face, the same five states as iOS `StatusContent`. */
sealed interface StatusContent {
    data object SignedOut : StatusContent
    data object NoComputer : StatusContent
    data class Paused(val snapshot: StatusSnapshot) : StatusContent
    data class Quiet(val snapshot: StatusSnapshot) : StatusContent
    data class Live(val snapshot: StatusSnapshot) : StatusContent

    /** iOS `Copy.state`: title and detail for the four non-live states. */
    fun stateCopy(): Pair<String, String>? = when (this) {
        SignedOut -> "Sign in to Allternit" to "Open the app to sign in."
        NoComputer -> "Pair a computer" to "Your bots run on a computer."
        is Paused -> "Bots paused" to "Resume them in the app."
        is Quiet -> "All quiet" to "Nothing needs you."
        is Live -> null
    }

    val snapshotOrNull: StatusSnapshot?
        get() = when (this) {
            is Paused -> snapshot
            is Quiet -> snapshot
            is Live -> snapshot
            else -> null
        }
}

/** iOS `StatusWidget.resolveContent`'s last step. */
fun StatusSnapshot.toContent(): StatusContent = when {
    paused -> StatusContent.Paused(this)
    activeBots == 0 && needsCount == 0 && attentionBots == 0 && focus == null -> StatusContent.Quiet(this)
    else -> StatusContent.Live(this)
}
