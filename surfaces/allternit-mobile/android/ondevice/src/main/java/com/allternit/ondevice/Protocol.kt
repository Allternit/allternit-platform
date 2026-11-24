package com.allternit.ondevice

import org.json.JSONObject

/**
 * Wire protocol between the PWA (allternit-ai src/lib/local-models/providers/android-app.ts)
 * and this engine, over the TWA postMessage channel. JSON strings, one message each.
 * Every message carries `v` (this [VERSION]). Bump it on any breaking change, in both files.
 *
 * Handshake (spike S1 rule 4): the page sends `hello` and repeats it every 250 ms until
 * `welcome`; the app also sends `ready` from onMessageChannelReady and the page answers with
 * `hello` at once. Only `welcome` / `hello` count as "connected".
 *
 * Page -> app
 *   {v,t:"hello",client,href,online}
 *   {v,t:"req",id,op,args}
 *     op = "hardware"                      -> budget, memory, device, engine, disk
 *          "list"                          -> installed models + active downloads
 *          "download" {model,quant,repo?,file,name?}  -> accepted; progress as evt "download"
 *          "cancel_download" {key}
 *          "delete" {key}
 *          "load" {key,ctx?,backend?:"auto"|"cpu"}     -> running info
 *          "unload"
 *          "chat" {key,messages:[{role,content}],max_tokens?,temperature?,seed?,thinking?}
 *                                          -> res {stream:id} then tok / end frames with id = req id
 *          "stop" {id}                     -> stops that reply (end frame finish "cancel")
 *          "resume" {id,from}              -> replays frames from seq `from`
 * App -> page
 *   {v,t:"ready"}
 *   {v,t:"welcome",proto,ok,app:{package,version},engine:{...}} (ok:false + error on a version mismatch)
 *   {v,t:"res",id,ok,data} | {v,t:"res",id,ok:false,error:{code,message}}
 *   {v,t:"evt",ev:"download"|"state",data}
 *   {v,t:"tok",id,seq,text}   {v,t:"end",id,seq,finish,stats}
 */
object Protocol {
    const val VERSION = 1

    fun msg(t: String): JSONObject = JSONObject().put("v", VERSION).put("t", t)

    fun ok(id: String, data: JSONObject = JSONObject()): String =
        msg("res").put("id", id).put("ok", true).put("data", data).toString()

    fun err(id: String, code: String, message: String): String =
        msg("res").put("id", id).put("ok", false)
            .put("error", JSONObject().put("code", code).put("message", message)).toString()

    fun evt(ev: String, data: JSONObject): String = msg("evt").put("ev", ev).put("data", data).toString()

    /** An error the page can show as-is (message is plain user-facing text). */
    class Failure(val code: String, message: String) : Exception(message)
}
