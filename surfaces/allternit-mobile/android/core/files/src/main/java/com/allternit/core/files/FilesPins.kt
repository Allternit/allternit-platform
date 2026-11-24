package com.allternit.core.files

import android.content.Context

/**
 * "Keep on this phone" (iOS FilesSharedStore pins + FilesPinKeeper): the files this phone keeps a copy of, so they
 * open without the network in this app and the Files app. Item ids only (no content, no secrets). Read by :app's
 * DocumentsProvider (never evicts a kept file, serves it offline) and written by a computer's file sheet.
 */
object FilesPins {
    private const val PREFS = "files_pins"
    private const val KEY = "pins"

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private fun id(cid: String, iid: String) = "$cid\u0000$iid"

    fun all(context: Context): Set<Pair<String, String>> = (prefs(context).getStringSet(KEY, emptySet()) ?: emptySet())
        .mapNotNull { s -> s.split('\u0000', limit = 2).takeIf { it.size == 2 }?.let { it[0] to it[1] } }.toSet()

    fun isPinned(context: Context, cid: String, iid: String): Boolean =
        prefs(context).getStringSet(KEY, emptySet())?.contains(id(cid, iid)) == true

    fun set(context: Context, cid: String, iid: String, pinned: Boolean) {
        val now = (prefs(context).getStringSet(KEY, emptySet()) ?: emptySet()).toMutableSet()
        if (pinned) now += id(cid, iid) else now -= id(cid, iid)
        prefs(context).edit().putStringSet(KEY, now).apply()
    }

    fun clear(context: Context) = prefs(context).edit().clear().apply()
}
