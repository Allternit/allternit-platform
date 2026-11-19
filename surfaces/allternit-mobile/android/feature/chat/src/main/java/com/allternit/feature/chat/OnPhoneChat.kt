package com.allternit.feature.chat

import com.allternit.core.stream.ChatStreamEvent
import com.allternit.ondevice.PhoneModels
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Gap run GS-D: a chat turn that runs on this phone (iOS OnDeviceChat). A brain id `ondevice/<freeweights id>#<quant>`
 * never goes to a computer or Allternit Cloud: the reply comes from the :ondevice llama.cpp engine, offline too.
 * Cancelling the collector (the chat's Stop) stops the reply.
 */
object OnPhoneChat {
    fun keyOf(runtimeModelId: String?): String? = PhoneModels.keyOfChatId(runtimeModelId)

    fun stream(key: String, roles: List<String>, contents: List<String>): Flow<ChatStreamEvent> = callbackFlow {
        val open = AtomicBoolean(true)
        try {
            PhoneModels.chat(key, roles, contents, onText = { t ->
                trySend(ChatStreamEvent.TextDelta(null, null, t))
                open.get()
            }) { err ->
                open.set(false)
                trySend(if (err != null) ChatStreamEvent.StreamError(err) else ChatStreamEvent.Done)
                close()
            }
        } catch (e: Exception) {
            open.set(false)
            trySend(ChatStreamEvent.StreamError("The engine on this phone isn't ready. Open Models › On this phone and try again."))
            close()
        }
        awaitClose { if (open.getAndSet(false)) PhoneModels.stopReply() }
    }.buffer(Channel.UNLIMITED)
}
