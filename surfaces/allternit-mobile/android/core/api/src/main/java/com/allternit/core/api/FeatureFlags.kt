package com.allternit.core.api

import android.content.Context

/**
 * Local feature flags, all ON by default (Eoj 2026-11-25: Apps + MCP ship as regular features) (iOS reads UserDefaults `allternit.flag.<name>`;
 * this is the same switch on Android). Flags in use: `mcp.tasks` (MCP task card and
 * server question form in chat), `apps.shelf` (apps shelf, share list, home-screen shortcut).
 * Turn one off for a sideloaded debug build by writing
 * `shared_prefs/allternit_flags.xml` with `adb shell run-as <package>` (a boolean named
 * after the flag), or call [set] from a debug screen.
 */
object FeatureFlags {
    private const val FILE = "allternit_flags"

    fun isOn(context: Context, name: String): Boolean = isOn(context, name, default = defaultFor(name))

    fun isOn(context: Context, name: String, default: Boolean): Boolean =
        runCatching { context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getBoolean(name, default) }
            .getOrDefault(default)

    /** Flags that ship OFF until Eoj turns them on (iOS `UserDefaults.bool` is false when unset). */
    private fun defaultFor(name: String): Boolean = name !in OFF_BY_DEFAULT

    fun set(context: Context, name: String, on: Boolean) {
        context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putBoolean(name, on).apply()
    }

    const val MCP_TASKS = "mcp.tasks"
    const val APPS_SHELF = "apps.shelf"
    /** Team map sheet (Island bar + Team map, lane 38): bot page, Inbox "Who approves this", Cowork. Off by default. */
    const val TEAM_MAP = "team.map"

    private val OFF_BY_DEFAULT = setOf(TEAM_MAP)
}
