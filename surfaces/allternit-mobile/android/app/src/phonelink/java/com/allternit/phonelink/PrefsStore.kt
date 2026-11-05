package com.allternit.phonelink

import android.content.SharedPreferences
import com.allternit.phonelink.core.KeyValueStore

/** Non-secret settings (device id, name, port, flags). Keys never go here. */
class PrefsStore(private val prefs: SharedPreferences) : KeyValueStore {
    override fun getString(key: String): String? = prefs.getString(key, null)
    override fun getBoolean(key: String, default: Boolean) = prefs.getBoolean(key, default)
    override fun getInt(key: String, default: Int) = prefs.getInt(key, default)
    override fun getLong(key: String, default: Long) = prefs.getLong(key, default)
    override fun edit(block: KeyValueStore.Editor.() -> Unit) {
        val e = prefs.edit()
        val editor = object : KeyValueStore.Editor {
            override fun putString(key: String, value: String?) { if (value == null) e.remove(key) else e.putString(key, value) }
            override fun putBoolean(key: String, value: Boolean) { e.putBoolean(key, value) }
            override fun putInt(key: String, value: Int) { e.putInt(key, value) }
            override fun putLong(key: String, value: Long) { e.putLong(key, value) }
        }
        editor.block()
        // commit, not apply: provisioning and Stop must be on disk before a reboot.
        e.commit()
    }
}
