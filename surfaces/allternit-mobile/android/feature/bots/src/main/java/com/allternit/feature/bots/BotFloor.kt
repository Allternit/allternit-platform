package com.allternit.feature.bots

/**
 * One bot on the Office floor, in the facility's `allternit:workspace` roster shape (iOS OfficeFloorView.feedJSON,
 * web workspace-feed.ts parseBots). :feature:office turns these into the feed; they come from here because the
 * avatar and colour helpers are internal to this module.
 */
data class BotFloorEntry(
    val id: String,
    val name: String,
    val role: String,
    val colorHex: String,
    /** "waiting", "working" or "idle". */
    val status: String,
    val task: String,
    val chief: Boolean,
    val question: String?,
    /** The picture the facility draws (allternit.com hosts only), or null. */
    val avatarSheet: String?,
    /** True for a whole picture (`layout: "still"`), false for a sprite sheet. */
    val avatarStill: Boolean,
)

/** A project's name for the facility's Commons studios (iOS workspaceProjects: names only, newest first, max 24). */
data class BotFloorProject(val id: String, val name: String, val updatedAt: String)

private const val ORB_SHEET = "https://ai.allternit.com/brand/allternit-orb.svg"

/** The facility loads pictures from allternit.com hosts only. */
private fun allowedAsset(url: String): Boolean {
    val u = runCatching { java.net.URI(url) }.getOrNull() ?: return false
    val host = u.host?.lowercase() ?: return false
    return u.scheme == "https" && (host == "allternit.com" || host.endsWith(".allternit.com"))
}

/** The live roster for the Office floor (iOS OfficeFloorView.feedJSON `bots`). */
fun BotModeStore.floorEntries(): List<BotFloorEntry> = bots.map { bot ->
    val threads = threads(bot.id)
    val waiting = threads.firstOrNull { it.group == "waiting" }
    val working = threads.firstOrNull { it.group == "working" }
    val task = (working ?: threads.firstOrNull())?.let { it.statusLine ?: it.displayTitle }.orEmpty()
    var sheet: String? = null
    var still = false
    if (bot.isPrimaryTemplateBot) {
        sheet = ORB_SHEET; still = true
    } else when (val src = AvatarSource.resolve(bot)) {
        is AvatarSource.Sprite -> if (allowedAsset(src.url)) sheet = src.url
        is AvatarSource.Picture -> if (allowedAsset(src.url)) { sheet = src.url; still = true }
        else -> Unit
    }
    BotFloorEntry(
        id = bot.id,
        name = bot.displayName,
        role = bot.tagline,
        colorHex = bot.accentHex ?: "#9CA3AF",
        status = if (waiting != null) "waiting" else if (working != null) "working" else "idle",
        task = task,
        chief = bot.isPrimaryTemplateBot,
        question = waiting?.let { it.statusLine ?: it.displayTitle },
        avatarSheet = sheet,
        avatarStill = still,
    )
}

/** Projects for the facility (iOS workspaceProjects). */
fun BotModeStore.floorProjects(): List<BotFloorProject> =
    projects.filter { it.id.isNotEmpty() && it.title.isNotEmpty() && !it.archived }
        .sortedByDescending { it.lastActivityAt.orEmpty() }
        .take(24)
        .map { BotFloorProject(it.id, it.title, it.lastActivityAt.orEmpty()) }
