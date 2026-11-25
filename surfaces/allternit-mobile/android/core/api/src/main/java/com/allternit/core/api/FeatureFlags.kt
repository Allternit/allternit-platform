package com.allternit.core.api

import android.content.Context

/**
 * Local feature flags, all off by default (iOS reads UserDefaults `allternit.flag.<name>`;
 * this is the same switch on Android). Flags in use: `mcp.tasks` (MCP task card and
 * server question form in chat), `apps.shelf` (apps shelf, share list, home-screen shortcut).
 * Turn one on for a sideloaded debug build by writing
 * `shared_prefs/allternit_flags.xml` with `adb shell run-as <package>` (a boolean named
 * after the flag), or call [set] from a debug screen.
 */
object FeatureFlags {
    private const val FILE = "allternit_flags"

    fun isOn(context: Context, name: String): Boolean =
        runCatching { context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getBoolean(name, false) }
            .getOrDefault(false)

    fun set(context: Context, name: String, on: Boolean) {
        context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putBoolean(name, on).apply()
    }

    const val MCP_TASKS = "mcp.tasks"
    const val APPS_SHELF = "apps.shelf"
}
