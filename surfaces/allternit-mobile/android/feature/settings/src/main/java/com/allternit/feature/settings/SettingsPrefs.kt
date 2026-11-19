package com.allternit.feature.settings

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.design.ThemeMode

/*
 * On-device Settings (iOS SettingsStore, Haptics.enabledKey, AppearanceStore, WaitingGamesStore). Plain
 * SharedPreferences: these are per-phone choices, never sent to the account.
 */
object SettingsPrefs {
    private const val FILE = "allternit_settings"

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /** iOS `openLinksIn`: In app (a Custom Tab) or the browser. */
    fun openLinksInApp(context: Context): Boolean = prefs(context).getString("openLinksIn", "app") != "browser"
    fun setOpenLinksInApp(context: Context, inApp: Boolean) = prefs(context).edit().putString("openLinksIn", if (inApp) "app" else "browser").apply()

    fun hapticsOn(context: Context): Boolean = prefs(context).getBoolean("allternit.haptics", true)
    fun setHapticsOn(context: Context, on: Boolean) = prefs(context).edit().putBoolean("allternit.haptics", on).apply()

    fun reduceMotion(context: Context): Boolean = prefs(context).getBoolean("allternit.reduceMotion", false)
    fun setReduceMotion(context: Context, on: Boolean) = prefs(context).edit().putBoolean("allternit.reduceMotion", on).apply()

    /** Settings > Advanced: "Save chat drafts" (iOS `settings.autoSave`, on by default). */
    fun saveDrafts(context: Context): Boolean = prefs(context).getBoolean("allternit.saveDrafts", true)
    fun setSaveDrafts(context: Context, on: Boolean) = prefs(context).edit().putBoolean("allternit.saveDrafts", on).apply()

    fun waitingGames(context: Context): Boolean = prefs(context).getBoolean("allternit.waitingGames", true)
    fun setWaitingGames(context: Context, on: Boolean) = prefs(context).edit().putBoolean("allternit.waitingGames", on).apply()

    /** Privacy: "Improve the model for everyone" (saved on this device, like iOS). */
    fun improveModel(context: Context): Boolean = prefs(context).getBoolean("allternit.improveModel", false)
    fun setImproveModel(context: Context, on: Boolean) = prefs(context).edit().putBoolean("allternit.improveModel", on).apply()

    /** Device Access toggles (one per kind), off until the person turns them on. */
    fun deviceAccess(context: Context, kind: String): Boolean = prefs(context).getBoolean("allternit.deviceAccess.$kind", false)
    fun setDeviceAccess(context: Context, kind: String, on: Boolean) = prefs(context).edit().putBoolean("allternit.deviceAccess.$kind", on).apply()

    /** A plain on-device switch (Memory: generate memory from chat history). */
    fun flag(context: Context, key: String, default: Boolean): Boolean = prefs(context).getBoolean("allternit.$key", default)
    fun setFlag(context: Context, key: String, on: Boolean) = prefs(context).edit().putBoolean("allternit.$key", on).apply()
    fun string(context: Context, key: String): String? = prefs(context).getString("allternit.$key", null)?.takeIf { it.isNotEmpty() }
    fun setString(context: Context, key: String, value: String) = prefs(context).edit().putString("allternit.$key", value).apply()

    fun appearance(context: Context): ThemeMode = when (prefs(context).getString("allternit.appearance", "light")) {
        "dark" -> ThemeMode.Dark
        "system" -> ThemeMode.System
        else -> ThemeMode.Light
    }

    fun setAppearance(context: Context, mode: ThemeMode) =
        prefs(context).edit().putString("allternit.appearance", mode.name.lowercase()).apply()
}

/** The workspace theme the Appearance tiles drive (iOS AppearanceStore). Light is the default. MainActivity reads [mode]. */
object AppearancePrefs {
    var mode by mutableStateOf(ThemeMode.Light)
        private set
    private var loaded = false

    fun load(context: Context) {
        if (loaded) return
        loaded = true
        mode = SettingsPrefs.appearance(context)
    }

    fun set(context: Context, next: ThemeMode) {
        mode = next
        SettingsPrefs.setAppearance(context, next)
    }
}
