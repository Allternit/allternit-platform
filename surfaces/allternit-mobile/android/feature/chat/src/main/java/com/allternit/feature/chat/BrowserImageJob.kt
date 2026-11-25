package com.allternit.feature.chat

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Base64
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.io.ByteArrayOutputStream
import java.util.UUID
import kotlinx.coroutines.*
import org.json.JSONObject
import kotlin.coroutines.resume

/** Only this fixed adapter can operate this browser. No page bridge or remotely supplied JS. */
internal class BrowserImageJob(private val context: Context, accountId: String) {
    data class Request(val id: String, val session: String, val call: String, val prompt: String, val deadline: Long, val remote: Boolean) {
        companion object {
            fun parse(o: JSONObject, displayedSession: String): Request? {
                // Unknown kinds never reach a code renderer. Binding comes from authenticated sync, not the page.
                if (o.optString("kind") != "browser-image" || o.optString("engine") != "chatgpt-aci" ||
                    o.optString("format") != "html" || o.optString("code") != "") return null
                val session = o.optString("sessionID", o.optString("session_id"))
                val id = o.optString("id", o.optString("request_id"))
                val prompt = o.opt("prompt") as? String ?: return null
                if (o.opt("code") !is String || o.opt("time") !is Number || o.opt("timeoutMs") !is Number) return null
                val timeout = o.optLong("timeoutMs", 0)
                val time = o.optLong("time", 0)
                val call = o.optJSONObject("toolCall")?.optString("callID").orEmpty()
                if (session != displayedSession || id.isBlank() || call.isBlank() || prompt.isBlank() || prompt.length > 16000 ||
                    timeout !in 1L..600000L || time > System.currentTimeMillis() + 30000 || time + timeout <= System.currentTimeMillis()) return null
                return Request(id, session, call, prompt, time + timeout, true)
            }
        }
    }
    var request by mutableStateOf<Request?>(null); private set
    var label by mutableStateOf(""); private set
    var visible by mutableStateOf(false)
    var confirmed by mutableStateOf(false); private set
    var foreground = true
    private var needsUser = false
    private val application = context.applicationContext as android.app.Application
    private val lifecycle = object : android.app.Application.ActivityLifecycleCallbacks {
        override fun onActivityResumed(activity: android.app.Activity) { foreground = true; browser?.onResume() }
        override fun onActivityPaused(activity: android.app.Activity) { foreground = false; if (request != null && confirmed) report("queued", "Paused — return to Allternit to reconnect"); browser?.onPause() }
        override fun onActivityCreated(a: android.app.Activity, b: android.os.Bundle?) = Unit
        override fun onActivityStarted(a: android.app.Activity) = Unit
        override fun onActivityStopped(a: android.app.Activity) = Unit
        override fun onActivitySaveInstanceState(a: android.app.Activity, b: android.os.Bundle) = Unit
        override fun onActivityDestroyed(a: android.app.Activity) = Unit
    }
    init { application.registerActivityLifecycleCallbacks(lifecycle) }
    private var browser: WebView? = null
    val webView: WebView? get() = browser
    private val prefs = context.getSharedPreferences("aci_image_jobs_" + UUID.nameUUIDFromBytes(accountId.toByteArray()), Context.MODE_PRIVATE)
    private var task: Job? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var complete: ((JSONObject) -> Unit)? = null
    private var progress: ((JSONObject) -> Unit)? = null
    private var generation = 0
    private var nonce = ""
    private var submitted = false
    private val seen = mutableSetOf<String>()

    init { active = this }

    fun offer(r: Request, onProgress: (JSONObject) -> Unit, onComplete: (JSONObject) -> Unit): Boolean {
        if (request != null || r.id in seen) return false
        if (prefs.contains(r.id)) {
            seen += r.id
            onComplete(error("ambiguous_submission", "This image request was already handled or interrupted on this phone. Check the browser before starting a new request; it will not be sent again."))
            return true
        }
        request = r; confirmed = false; submitted = false; needsUser = false
        progress = onProgress; complete = onComplete
        label = "Making the picture"
        return true
    }

    /** True while the sign-in screen (no image job) is open. */
    var signingIn by mutableStateOf(false); private set

    /** The "Sign in to ChatGPT" action on a failed image result: opens the sign-in page, with no image request. */
    @SuppressLint("SetJavaScriptEnabled")
    fun signIn() {
        if (request != null || browser != null) return
        signingIn = true; visible = true; label = "Sign in, then return to chat"
        browser = WebView(context).apply {
            settings.javaScriptEnabled = true; settings.domStorageEnabled = true
            settings.allowFileAccess = false; settings.allowContentAccess = false
            settings.mixedContentMode = android.webkit.WebSettings.MIXED_CONTENT_NEVER_ALLOW
            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    val u = request.url
                    return u.scheme != "https" || u.host !in setOf("chatgpt.com", "auth.openai.com", "auth0.openai.com", "accounts.google.com", "login.live.com", "appleid.apple.com")
                }
            }
            loadUrl("https://chatgpt.com/")
        }
    }

    /** Closes the sign-in screen and drops its page. */
    fun endSignIn() { if (signingIn) { signingIn = false; visible = false; destroyBrowser() } }

    @SuppressLint("SetJavaScriptEnabled")
    fun confirm() {
        val r = request ?: return
        if (confirmed || !foreground) return
        endSignIn()
        // Durable before any provider send. Replayed IDs cannot submit, even after process death.
        if (!prefs.edit().putString(r.id, "confirmed:${r.session}").commit()) {
            finish(error("storage", "Could not save the job identity. Try again after freeing storage.")); return
        }
        confirmed = true; visible = true; nonce = UUID.randomUUID().toString().replace("-", "")
        browser = WebView(context).apply {
            settings.javaScriptEnabled = true; settings.domStorageEnabled = true
            settings.allowFileAccess = false; settings.allowContentAccess = false
            settings.mixedContentMode = android.webkit.WebSettings.MIXED_CONTENT_NEVER_ALLOW
            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    val u = request.url
                    return u.scheme != "https" || u.host !in setOf("chatgpt.com", "auth.openai.com", "auth0.openai.com", "accounts.google.com", "login.live.com", "appleid.apple.com")
                }
            }
            loadUrl("https://chatgpt.com/")
        }
        val epoch = ++generation
        task = scope.launch {
            try {
                while (request == r && epoch == generation && System.currentTimeMillis() < r.deadline) {
                    if (!foreground) { report("queued", "Paused — return to Allternit to reconnect"); delay(1500); continue }
                    if (needsUser) { delay(1500); continue }
                    if (browser?.url?.let { Uri.parse(it).host } != "chatgpt.com") {
                        report("queued", "Needs you — finish sign-in in this browser"); delay(1500); continue
                    }
                    val state = JSONObject(evaluate(probe(r.prompt)))
                    when (state.optString("state")) {
                        "needs-user" -> { visible = true; needsUser = true; report("queued", "Needs you — finish sign-in or verification, then tap Resume"); delay(2000) }
                        "mode" -> {
                            val mode = JSONObject(evaluate(selectImageMode()))
                            if (mode.optString("state") == "unsupported") {
                                finish(error("unsupported", "Picture mode is unavailable in the browser. Open a regular empty chat with Create image selected, then request an image again.")); return@launch
                            }
                            report("queued", "Opening picture mode"); delay(1000)
                        }
                        "ready" -> {
                            if (!submitted) {
                                // Persist ambiguity BEFORE clicking. Resume only observes; it never sends again.
                                check(prefs.edit().putString(r.id, JSONObject().put("state", "submitted").put("session", r.session).put("conversation", browser?.url).toString()).commit())
                                submitted = true
                                val reply = JSONObject(evaluate(submit(r.prompt)))
                                var clicked = false
                                if (reply.optString("state") == "prepared") {
                                    repeat(10) {
                                        if (!clicked && foreground) {
                                            delay(250)
                                            clicked = JSONObject(evaluate(click(r.prompt))).optString("state") == "submitted"
                                        }
                                    }
                                }
                                if (!clicked) {
                                    finish(error("unsupported", "The browser page is unsupported. Check the browser before starting a new image request.")); return@launch
                                }
                            }
                            report("drawing", "Making the picture"); delay(1500)
                        }
                        "waiting" -> {
                            val preview = state.optString("preview").takeIf { validPreview(it) }
                            report(if (preview != null) "rendering" else "drawing", if (preview != null) "Image preview" else "Making the picture", preview = preview)
                            delay(1500)
                        }
                        "image" -> {
                            report("downloading", "Downloading the picture")
                            evaluate(startDownload(state.getString("url")))
                            val bytes = readDownload(r)
                            report("saving", "Saving the picture", bytes.size.toLong(), bytes.size.toLong())
                            val result = withContext(Dispatchers.Default) { normalize(bytes) }
                            finish(result); return@launch
                        }
                        "unsupported" -> { finish(error("unsupported", "The page changed or returned no picture. Open the browser and check this request.")); return@launch }
                        else -> delay(1000)
                    }
                }
                if (request == r) finish(error("timeout", "The image request expired. Check the browser before starting another request; it will not be sent again."))
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { if (request == r) finish(error("image_failed", e.message ?: "The image could not be downloaded. Check the browser before starting again.")) }
        }
    }

    fun resume() { if (confirmed) { needsUser = false; visible = true; label = "Reconnecting — observing the existing request" } }
    fun cancel() { if (request != null) finish(error("cancelled", "Image generation stopped on this phone. The browser may still finish a submitted request.")) }
    private fun report(stage: String, text: String, bytes: Long? = null, total: Long? = null, preview: String? = null) {
        label = text
        progress?.invoke(JSONObject().put("lane", "picture").put("stage", stage).put("label", text).apply {
            if (preview != null) put("preview", preview)
            if (bytes != null) put("bytes", bytes)
            if (total != null) put("total_bytes", total)
        })
    }
    private fun validPreview(value: String): Boolean {
        if (!value.startsWith("data:image/jpeg;base64,") || value.length > 180000) return false
        return runCatching {
            val b = Base64.decode(value.substringAfter(','), Base64.DEFAULT)
            val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeByteArray(b, 0, b.size, options)
            options.outWidth in 1..320 && options.outHeight in 1..320
        }.getOrDefault(false)
    }
    private fun finish(result: JSONObject) {
        request?.let {
            seen += it.id
            prefs.edit().putString(it.id, "finished:${it.session}").commit()
        }
        generation++; task?.cancel(); task = null
        val callback = complete
        request = null; confirmed = false; visible = false; complete = null; progress = null
        destroyBrowser(); callback?.invoke(result)
    }
    private fun destroyBrowser() {
        browser?.let { (it.parent as? android.view.ViewGroup)?.removeView(it); it.stopLoading(); it.loadUrl("about:blank"); it.destroy() }; browser = null
    }
    fun dispose() {
        if (active === this) active = null
        complete = null; progress = null
        cancel(); destroyBrowser(); scope.cancel()
        application.unregisterActivityLifecycleCallbacks(lifecycle)
        // Remove this provider's website state on account teardown. Never read or export cookies.
        android.webkit.CookieManager.getInstance().removeAllCookies(null)
        android.webkit.CookieManager.getInstance().flush()
        android.webkit.WebStorage.getInstance().deleteAllData()
    }
    private suspend fun evaluate(script: String): String = suspendCancellableCoroutine { c ->
        val view = browser
        if (view == null) { c.cancel(); return@suspendCancellableCoroutine }
        view.evaluateJavascript(script) { raw ->
            if (c.isActive) c.resume(runCatching { org.json.JSONTokener(raw).nextValue() as? String ?: "{}" }.getOrDefault("{}"))
        }
    }
    // Keep role containers canonical when modern wrappers also contain legacy role nodes.
    private val dom = """
        function role(t){return t.getAttribute('data-message-author-role')||t.getAttribute('data-conversation-role')||(t.hasAttribute('data-user-message-bubble')?'user':'');}
        function turns(){var all=Array.from(document.querySelectorAll('[data-message-author-role],[data-conversation-role],[data-user-message-bubble]'));return all.filter(function(t){return !all.some(function(p){return p!==t&&p.contains(t)&&role(p)===role(t);});});}
        function composer(){return document.querySelector('#prompt-textarea,textarea[data-testid="prompt-textarea"],[contenteditable="true"][role="textbox"]');}
        function send(){return document.querySelector('button[aria-label="Send prompt"],[data-testid="send-button"]');}
        function imageMode(){return Array.from(document.querySelectorAll('[aria-pressed="true"],[aria-selected="true"],[data-state="active"]')).some(function(b){return /^(Create image|Create images|Image)$/i.test((b.innerText||b.getAttribute('aria-label')||'').trim());})||Array.from(document.querySelectorAll('button[aria-label]')).some(function(b){return /^(Remove|Deselect) (Create image|Create images|Image)( mode)?$/i.test(b.getAttribute('aria-label'));});}
        function regular(){return !new URL(location.href).searchParams.has('temporary-chat')&&!/tempchat/i.test(location.href)&&!Array.from(document.querySelectorAll('[aria-pressed="true"],[data-state="checked"]')).some(function(e){return /temporary chat/i.test(e.innerText||e.getAttribute('aria-label')||'');});}
    """.trimIndent()
    private fun selectImageMode() = """
        (function(){
          $dom
          if(location.origin!=='https://chatgpt.com'||!regular()||turns().length)return JSON.stringify({state:'unsupported'});
          if(imageMode())return JSON.stringify({state:'ready'});
          var choices=Array.from(document.querySelectorAll('button,[role="menuitem"]'));
          var create=choices.find(function(b){return /^Create images?$/i.test((b.innerText||b.getAttribute('aria-label')||'').trim());});
          if(create&&!window['aci_selected_$nonce']){window['aci_selected_$nonce']=true;create.click();return JSON.stringify({state:'opening'});}
          if(window['aci_selected_$nonce']){window['aci_mode_wait_$nonce']=(window['aci_mode_wait_$nonce']||0)+1;return JSON.stringify({state:window['aci_mode_wait_$nonce']<8?'opening':'unsupported'});}
          if(!window['aci_mode_$nonce']){var tools=choices.find(function(b){return /^(Tools|View tools|Open tools menu)$/i.test((b.getAttribute('aria-label')||b.innerText||'').trim());});if(tools){window['aci_mode_$nonce']=true;tools.click();return JSON.stringify({state:'opening'});}}
          return JSON.stringify({state:'unsupported'});
        })()
    """.trimIndent()
    private fun probe(prompt: String) = """
        (function(){
          if(location.origin!=='https://chatgpt.com')return JSON.stringify({state:'needs-user'});
          if(document.querySelector('iframe[src*="challenges.cloudflare.com"], [id*="challenge"], input[type="password"], a[href*="/auth/login"],button[data-testid="login-button"]'))return JSON.stringify({state:'needs-user'});
          if(/verify you are human|checking your browser|just a moment/i.test(document.title))return JSON.stringify({state:'needs-user'});
          $dom
          if(!regular())return JSON.stringify({state:'unsupported'});
          var j=window['aci_$nonce'];
          if(!j){if(turns().length)return JSON.stringify({state:'unsupported'});return JSON.stringify({state:composer()?(imageMode()?'ready':'mode'):'loading'});}
          var rows=turns();
          var user=rows.findIndex(function(t){return role(t)==='user'&&(t.querySelector('[data-user-message-bubble]')||t).innerText.trim()===${JSONObject.quote(prompt.trim())};});
          if(user<0)return JSON.stringify({state:'waiting'});
          var after=rows.slice(user+1);if(after.some(function(t){return role(t)==='user';}))return JSON.stringify({state:'unsupported'});
          var a=after.filter(function(t){return role(t)==='assistant';});
          var images=a.flatMap(function(t){return Array.from(t.querySelectorAll('img')).filter(function(i){return i.complete&&i.naturalWidth>=256&&i.naturalHeight>=256&&!j.images.includes(i.currentSrc||i.src);});});
          var busy=document.querySelector('[data-testid="stop-button"],button[aria-label="Stop generating"],button[aria-label="Stop streaming"]')||a.some(function(t){return t.getAttribute('aria-busy')==='true'||t.querySelector('[aria-busy="true"],[data-is-streaming="true"]');});
          if(images.length>1)return JSON.stringify({state:'unsupported'});
          var finished=a.some(function(t){return /image created|created image/i.test(t.innerText)||Array.from(t.querySelectorAll('button,a,[role="button"]')).some(function(b){return /^(Download( this image| image)?|Save image|Copy image)$/i.test((b.getAttribute('aria-label')||b.getAttribute('title')||b.innerText||'').trim());});});
          if(images.length&&!busy&&finished)return JSON.stringify({state:'image',url:images[0].currentSrc||images[0].src});
          if(images.length&&busy){try{var image=images[images.length-1],scale=Math.min(320/image.naturalWidth,320/image.naturalHeight,1),canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.floor(image.naturalWidth*scale));canvas.height=Math.max(1,Math.floor(image.naturalHeight*scale));canvas.getContext('2d').drawImage(image,0,0,canvas.width,canvas.height);var preview=canvas.toDataURL('image/jpeg',0.65);if(preview.length<=180000)return JSON.stringify({state:'waiting',preview:preview});}catch(e){}}
          if(a.length&&!busy&&!images.length){if(!j.noImageAt)j.noImageAt=Date.now();if(Date.now()-j.noImageAt>30000)return JSON.stringify({state:'unsupported'});}else{j.noImageAt=0;}
          return JSON.stringify({state:'waiting'});
        })()
    """.trimIndent()
    private fun submit(prompt: String) = """
        (function(){
          if(location.origin!=='https://chatgpt.com'||window['aci_$nonce'])return JSON.stringify({state:'unsupported'});
          $dom
          if(!regular()||!imageMode()||turns().length)return JSON.stringify({state:'unsupported'});
          var e=composer(),b=send();
          if(!e||e.innerText.trim()||e.value)return JSON.stringify({state:'unsupported'});
          window['aci_$nonce']={images:Array.from(document.images).map(function(i){return i.currentSrc||i.src;})};
          e.focus();
          if(e.tagName==='TEXTAREA'){Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,${JSONObject.quote(prompt)});e.dispatchEvent(new Event('input',{bubbles:true}));}
          else {var selection=window.getSelection(),range=document.createRange();range.selectNodeContents(e);selection.removeAllRanges();selection.addRange(range);if(!document.execCommand('insertText',false,${JSONObject.quote(prompt)}))return JSON.stringify({state:'unsupported'});}
          return JSON.stringify({state:'prepared'});
        })()
    """.trimIndent()
    private fun click(prompt: String) = """
        (function(){
          if(location.origin!=='https://chatgpt.com')return JSON.stringify({state:'unsupported'});
          $dom
          if(!regular()||!imageMode()||turns().length)return JSON.stringify({state:'unsupported'});
          var j=window['aci_$nonce'],e=composer(),b=send();
          if(!j||j.clicked||!e||(e.value||e.innerText)!==${JSONObject.quote(prompt)}||!b||b.disabled)return JSON.stringify({state:'unsupported'});
          j.clicked=true;b.click();return JSON.stringify({state:'submitted'});
        })()
    """.trimIndent()
    private fun startDownload(url: String) = """
        (function(){
          var j=window['aci_$nonce'];if(!j||j.download)return '{}';j.download={bytes:0};
          (async function(){try{
            var u=new URL(${JSONObject.quote(url)},location.href);
            if(!(u.protocol==='blob:'&&u.origin===location.origin)&&!(u.protocol==='https:'&&(u.hostname==='chatgpt.com'||u.hostname.endsWith('.oaiusercontent.com')||u.hostname==='oaiusercontent.com')))throw Error('Unsupported image location');
            var r=await fetch(u.href,{credentials:'include'});if(!r.ok)throw Error('Image download failed');
            if(r.url&&r.url!==u.href){var dest=new URL(r.url);if(dest.protocol!=='https:'||!(dest.hostname==='chatgpt.com'||dest.hostname.endsWith('.oaiusercontent.com')||dest.hostname==='oaiusercontent.com'))throw Error('Unsupported image redirect');}
            var mime=(r.headers.get('content-type')||'').split(';')[0];if(!['image/png','image/jpeg','image/webp'].includes(mime))throw Error('Unsupported image type');
            var total=Number(r.headers.get('content-length'))||0;if(total>20971520)throw Error('Source image exceeds 20 MiB');j.download.total=total;
            var reader=r.body.getReader(),parts=[],size=0;
            while(true){var p=await reader.read();if(p.done)break;size+=p.value.length;if(size>20971520){await reader.cancel();throw Error('Source image exceeds 20 MiB');}parts.push(p.value);j.download.bytes=size;}
            var data=new Uint8Array(size),at=0;parts.forEach(function(p){data.set(p,at);at+=p.length;});
            j.download.chunks=[];for(var i=0;i<size;i+=49152){var s='';data.subarray(i,i+49152).forEach(function(b){s+=String.fromCharCode(b);});j.download.chunks.push(btoa(s));}j.download.done=true;
          }catch(e){j.download.error=String(e.message||e);}})();return '{}';
        })()
    """.trimIndent()
    private suspend fun readDownload(r: Request): ByteArray {
        while (System.currentTimeMillis() < r.deadline) {
            if (!foreground) { report("queued", "Paused — return to Allternit to finish downloading"); delay(1000); continue }
            val d = JSONObject(evaluate("JSON.stringify((window['aci_$nonce']||{}).download||{})"))
            check(!d.has("error")) { d.optString("error") }
            report("downloading", "Downloading the picture", d.optLong("bytes"), d.optLong("total").takeIf { it > 0 })
            if (d.optBoolean("done")) {
                val out = ByteArrayOutputStream()
                val count = JSONObject(evaluate("JSON.stringify({count:window['aci_$nonce'].download.chunks.length})")).getInt("count")
                check(count in 1..427)
                repeat(count) { i ->
                    val encoded = JSONObject(evaluate("JSON.stringify({chunk:window['aci_$nonce'].download.chunks[$i]})")).getString("chunk")
                    check(encoded.length <= 65536)
                    out.write(Base64.decode(encoded, Base64.DEFAULT)); check(out.size() <= 20 * 1024 * 1024)
                }
                return out.toByteArray()
            }
            delay(500)
        }
        throw IllegalStateException("Image download timed out. Check the browser before requesting another image.")
    }
    private fun normalize(bytes: ByteArray): JSONObject {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        check(bounds.outMimeType in setOf("image/png", "image/jpeg", "image/webp")) { "Unsupported decoded image type" }
        check(bounds.outWidth in 1..4096 && bounds.outHeight in 1..4096 && bounds.outWidth.toLong() * bounds.outHeight <= 16000000) { "Image dimensions exceed the supported limit" }
        val bitmap = checkNotNull(BitmapFactory.decodeByteArray(bytes, 0, bytes.size)) { "The downloaded image could not be decoded" }
        try {
            val out = object : ByteArrayOutputStream() {
                override fun write(b: ByteArray, off: Int, len: Int) { check(count + len <= 8 * 1024 * 1024) { "Final PNG exceeds 8 MiB" }; super.write(b, off, len) }
                override fun write(b: Int) { check(count < 8 * 1024 * 1024); super.write(b) }
            }
            check(bitmap.compress(Bitmap.CompressFormat.PNG, 100, out))
            return JSONObject().put("ok", true).put("image", "data:image/png;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)).put("width", bitmap.width).put("height", bitmap.height)
        } finally { bitmap.recycle() }
    }
    companion object {
        /** The shell-owned job, so a failed result card can offer the sign-in action. */
        @Volatile var active: BrowserImageJob? = null
        fun error(code: String, text: String) = JSONObject().put("ok", false).put("error", text).put("code", code)
    }
}
