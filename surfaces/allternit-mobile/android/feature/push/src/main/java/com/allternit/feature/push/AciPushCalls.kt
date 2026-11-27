package com.allternit.feature.push

import android.content.Context
import com.allternit.core.auth.AllternitAuth
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.chat.RuntimeCall

/** One call to the computer's ACI routes from a notification button (cold process: start auth first). */
internal object AciPushCalls {
    suspend fun post(context: Context, path: String, body: String?, runtimeId: String?): RuntimeCall {
        AllternitAuth.start(context)
        if (AllternitAuth.tokenProvider.bearerToken().isNullOrBlank()) return RuntimeCall.Http(401, null, null)
        return ChatBackends.create(context).runtimeJson("POST", path, body, targetRuntimeId = runtimeId)
    }
}
