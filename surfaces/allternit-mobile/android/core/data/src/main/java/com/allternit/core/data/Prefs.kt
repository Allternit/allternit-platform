package com.allternit.core.data

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.preferencesDataStore
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.booleanPreferencesKey
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

private val Context.allternitPrefs: DataStore<Preferences> by preferencesDataStore(name = "allternit_prefs")

/**
 * Small typed wrapper over Preferences DataStore, the Android stand-in for the iOS
 * app's UserDefaults stores. One file (`allternit_prefs`) per app process; screens
 * read [Flow]s and write with suspend setters.
 */
class Prefs(private val store: DataStore<Preferences>) {

    constructor(context: Context) : this(context.applicationContext.allternitPrefs)

    fun string(key: String): Flow<String?> =
        store.data.map { it[stringPreferencesKey(key)] }

    fun boolean(key: String, default: Boolean = false): Flow<Boolean> =
        store.data.map { it[booleanPreferencesKey(key)] ?: default }

    /** Stores [value]; null removes the key. */
    suspend fun setString(key: String, value: String?) {
        store.edit { prefs ->
            val k = stringPreferencesKey(key)
            if (value == null) prefs.remove(k) else prefs[k] = value
        }
    }

    suspend fun setBoolean(key: String, value: Boolean) {
        store.edit { it[booleanPreferencesKey(key)] = value }
    }

    suspend fun clear() {
        store.edit { it.clear() }
    }
}
