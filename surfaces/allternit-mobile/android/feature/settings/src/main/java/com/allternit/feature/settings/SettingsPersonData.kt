package com.allternit.feature.settings

import android.content.Context
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.files.FilesSession
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicBoolean

/**
 * What leaves this phone at sign-out, on AllternitAuth's sign-out hook (iOS AuthManager, `reason.clearsPersonData`):
 * the Files drive and Files key material (`FilesDriveBridge.clearLocal()` → [FilesSession.clearPerson]) and the
 * voice keys (`VoiceKeySync.forgetLocal()`). Nothing is deleted from the leaving person's account.
 */
object SettingsPersonData {
    private val started = AtomicBoolean(false)

    /** MainActivity.onCreate, once per process. */
    fun start(context: Context) {
        val app = context.applicationContext
        if (!started.compareAndSet(false, true)) return
        AllternitAuth.addSignOutHook { forget(app) }
    }

    suspend fun forget(context: Context) = withContext(Dispatchers.IO) {
        runCatching { FilesSession.clearPerson(context) }
        runCatching { VoiceKeySync.forgetLocal(context) }
        Unit
    }
}
