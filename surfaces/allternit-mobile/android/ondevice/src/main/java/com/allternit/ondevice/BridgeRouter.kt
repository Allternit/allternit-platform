package com.allternit.ondevice

import android.app.NotificationManager
import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * Turns page messages ([Protocol]) into engine calls. Runs on the bridge worker; anything
 * slow goes to the engine thread (load, chat) or the io pool (manifest fetch).
 */
object BridgeRouter {
    private const val TAG = "BridgeRouter"
    private val io = Executors.newFixedThreadPool(2) { r -> Thread(r, "ondevice-io") }

    fun handle(raw: String) {
        val ctx = TwaBridge.appContext ?: return
        val msg = try { JSONObject(raw) } catch (e: Exception) {
            Log.w(TAG, "non-JSON message ignored"); return
        }
        val engine = OnDeviceEngine.get(ctx)
        engine.sink = TwaBridge::post
        when (msg.optString("t")) {
            "hello" -> hello(ctx, engine, msg)
            "req" -> request(ctx, engine, msg)
            else -> Log.w(TAG, "unknown message type ${msg.optString("t")}")
        }
    }

    private fun hello(ctx: Context, engine: OnDeviceEngine, msg: JSONObject) {
        val v = msg.optInt("v", 0)
        val welcome = Protocol.msg("welcome").put("proto", Protocol.VERSION)
        if (v != Protocol.VERSION) {
            welcome.put("ok", false).put("error", JSONObject().put("code", "protocol_version")
                .put("message", "This page speaks bridge protocol $v; the app speaks ${Protocol.VERSION}. Update the app."))
            TwaBridge.post(welcome.toString())
            return
        }
        TwaBridge.markConnected()
        val pkg = ctx.packageManager.getPackageInfo(ctx.packageName, 0)
        welcome.put("ok", true)
            .put("app", JSONObject().put("package", ctx.packageName).put("version", pkg.versionName))
            .put("engine", engine.engineInfo())
            .put("running", engine.runningJson() ?: JSONObject.NULL)
        TwaBridge.post(welcome.toString())
    }

    private fun request(ctx: Context, engine: OnDeviceEngine, msg: JSONObject) {
        val id = msg.optString("id")
        val args = msg.optJSONObject("args") ?: JSONObject()
        if (id.isEmpty()) return
        fun reply(block: () -> JSONObject) {
            val out = try {
                Protocol.ok(id, block())
            } catch (f: Protocol.Failure) {
                Protocol.err(id, f.code, f.message ?: f.code)
            } catch (e: Exception) {
                Log.e(TAG, "op ${msg.optString("op")} failed", e)
                Protocol.err(id, "internal", e.message ?: e.javaClass.simpleName)
            }
            TwaBridge.post(out)
        }
        when (val op = msg.optString("op")) {
            "hardware" -> reply { engine.hardware() }
            "list" -> reply { engine.list() }
            "download" -> io.execute {
                reply {
                    val model = args.getString("model")
                    val quant = args.getString("quant")
                    val job = engine.downloads.start(
                        model, quant, args.optString("repo").ifEmpty { model }, args.getString("file"),
                        args.optString("name").ifEmpty { model.substringAfter('/') },
                    )
                    JSONObject().put("key", job.key).put("size", job.size).put("sha256", job.sha256)
                        .put("notifications", notificationsOn(ctx))
                }
            }
            "cancel_download" -> reply { JSONObject().put("cancelled", engine.downloads.cancel(args.getString("key"))) }
            "delete" -> engine.post {
                reply {
                    val key = args.getString("key")
                    if (engine.runningJson()?.optString("key") == key) engine.unloadNow("delete")
                    JSONObject().put("deleted", engine.store.delete(key))
                }
            }
            "load" -> engine.post {
                reply { engine.loadNow(args.getString("key"), args.optInt("ctx", OnDeviceEngine.DEFAULT_CTX), args.optString("backend", "auto")) }
            }
            "unload" -> {
                engine.requestUnload("page")
                reply { JSONObject().put("unloaded", true) }
            }
            "chat" -> {
                val key = args.optString("key")
                if (key.isEmpty()) { TwaBridge.post(Protocol.err(id, "bad_request", "chat needs a model key")); return }
                reply { JSONObject().put("stream", id) }
                engine.post { engine.chatNow(id, key, args) }
            }
            "stop" -> reply { JSONObject().put("stopped", engine.stop(args.optString("id"))) }
            "resume" -> {
                val s = engine.stream(args.optString("id"))
                if (s == null) TwaBridge.post(Protocol.err(id, "no_stream", "That reply is no longer kept."))
                else {
                    TwaBridge.post(Protocol.ok(id, JSONObject().put("frames", s.frameCount())))
                    s.resume(args.optInt("from", 0))
                }
            }
            else -> TwaBridge.post(Protocol.err(id, "unknown_op", "Unknown op $op"))
        }
    }

    private fun notificationsOn(ctx: Context): Boolean =
        ctx.getSystemService(NotificationManager::class.java).areNotificationsEnabled()
}
