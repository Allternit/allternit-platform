package com.allternit.core.design

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.runtime.staticCompositionLocalOf
import java.time.Instant

/** The `/me/flags` keys for first-visit intros (mobile polish contract). Bots reuses the existing key. */
object IntroKeys {
    const val HOME = "introHomeSeenAt"
    const val CODE = "introCodeSeenAt"
    const val ACI = "introAciSeenAt"
    const val PROJECTS = "introProjectsSeenAt"
    const val ARTIFACTS = "introArtifactsSeenAt"
    const val AUTOMATION = "introAutomationSeenAt"
    const val CUSTOMIZE = "introCustomizeSeenAt"
    const val FREEWEIGHTS = "introFreeweightsSeenAt"
    /** Shared with "Meet your bots" so nobody sees two Bots intros. */
    const val BOTS = "botsIntroSeenAt"

    val all = listOf(HOME, CODE, ACI, PROJECTS, ARTIFACTS, AUTOMATION, CUSTOMIZE, FREEWEIGHTS, BOTS)
}

/**
 * Whether an intro was already shown. The local cache decides first, so an intro never
 * repeats on this device even offline or while the flags PUT is refused. [fetch] reads
 * the account flags (key to iso time, null on failure) and [push] writes one key (true
 * when accepted). Wire them to core:api `MeFlagsClient.read` / `write`; core:design has
 * no network dependency on purpose.
 *
 * [accountId] keys the cache per account (one prefs file each), so a second account on the
 * same phone still sees its intros. The first account to load after the upgrade takes over
 * the old device-wide entries once, so existing users don't see intros again. Null keeps the
 * device-wide file.
 */
class IntroSeenStore(
    context: Context,
    private val fetch: suspend () -> Map<String, String>?,
    private val push: suspend (key: String, iso: String) -> Boolean,
    private val now: () -> String = { Instant.now().toString() },
    accountId: String? = null,
) {
    private val prefs: SharedPreferences = run {
        val app = context.applicationContext
        val legacy = app.getSharedPreferences(LEGACY_FILE, Context.MODE_PRIVATE)
        if (accountId.isNullOrEmpty()) return@run legacy
        val mine = app.getSharedPreferences("$LEGACY_FILE.${accountId.hashCode().toUInt().toString(16)}_${accountId.filter { it.isLetterOrDigit() }.take(40)}", Context.MODE_PRIVATE)
        // One-time move of the pre-account entries (seen times and pending writes) to this account.
        if (legacy.all.isNotEmpty()) {
            val edit = mine.edit()
            for ((k, v) in legacy.all) {
                if (mine.contains(k)) continue
                when (v) {
                    is String -> edit.putString(k, v)
                    is Boolean -> edit.putBoolean(k, v)
                }
            }
            edit.commit()
            legacy.edit().clear().commit()
        }
        mine
    }

    /** Seen on this device or on the account (as last fetched). */
    fun isSeen(key: String): Boolean = prefs.getString(key, null) != null

    /** Pulls account flags into the cache and retries writes that failed earlier. Safe to call on every launch. */
    suspend fun refresh() {
        val remote = runCatching { fetch() }.getOrNull()
        if (remote != null) {
            prefs.edit().apply { IntroKeys.all.forEach { k -> remote[k]?.let { putString(k, it) } } }.apply()
        }
        for (k in IntroKeys.all) {
            val iso = prefs.getString(k, null) ?: continue
            if (prefs.getBoolean(pending(k), false) && runCatching { push(k, iso) }.getOrDefault(false)) {
                prefs.edit().remove(pending(k)).apply()
            }
        }
    }

    /** Marks [key] seen: cache first (never blocks), then the account write; a failed write is retried by [refresh]. */
    suspend fun markSeen(key: String) {
        if (isSeen(key)) return
        val iso = now()
        prefs.edit().putString(key, iso).putBoolean(pending(key), true).apply()
        if (runCatching { push(key, iso) }.getOrDefault(false)) prefs.edit().remove(pending(key)).apply()
    }

    private fun pending(key: String) = "pending.$key"

    private companion object {
        const val LEGACY_FILE = "allternit.intro_seen"
    }
}

/**
 * The signed-in app's one [IntroSeenStore], provided by the workspace shell. Null while signed out
 * (or outside the shell), and then no first-visit intro shows.
 */
val LocalIntroSeenStore = staticCompositionLocalOf<IntroSeenStore?> { null }
