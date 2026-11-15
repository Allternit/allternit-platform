package com.allternit.ondevice

import org.json.JSONArray
import org.json.JSONObject

/**
 * The JNI surface of libs allternit_llama (src/main/cpp/engine_jni.cpp). Only
 * [EngineService]'s worker thread calls these, except [stop].
 */
internal object NativeLlama {
    /** Streaming sink for [chat]. Return false to stop generating. */
    fun interface TokenSink {
        fun onToken(utf8: ByteArray): Boolean
    }

    @Volatile private var devicesJson: String? = null

    /** Loads libggml + every backend once. Returns the devices that came up. */
    @Synchronized
    fun init(nativeLibDir: String): JSONArray {
        devicesJson?.let { return JSONArray(it) }
        System.loadLibrary("allternit_llama")
        val json = nativeInit(nativeLibDir)
        devicesJson = json
        return JSONArray(json)
    }

    fun load(path: String, nCtx: Int, nThreads: Int, useGpu: Boolean): Long =
        nativeLoad(path, nCtx, nThreads, useGpu)

    fun info(handle: Long): JSONObject = JSONObject(nativeInfo(handle))

    fun chat(
        handle: Long,
        roles: Array<String>,
        contents: Array<String>,
        maxTokens: Int,
        temperature: Float,
        seed: Int,
        thinking: Boolean,
        sink: TokenSink,
    ): JSONObject = JSONObject(nativeChat(handle, roles, contents, maxTokens, temperature, seed, thinking, sink))

    fun stop(handle: Long) = nativeStop(handle)

    fun free(handle: Long) = nativeFree(handle)

    @JvmStatic private external fun nativeInit(libDir: String): String
    @JvmStatic private external fun nativeLoad(path: String, nCtx: Int, nThreads: Int, useGpu: Boolean): Long
    @JvmStatic private external fun nativeInfo(handle: Long): String
    @JvmStatic private external fun nativeChat(
        handle: Long, roles: Array<String>, contents: Array<String>, maxTokens: Int,
        temperature: Float, seed: Int, thinking: Boolean, sink: TokenSink,
    ): String
    @JvmStatic private external fun nativeStop(handle: Long)
    @JvmStatic private external fun nativeFree(handle: Long)
}
