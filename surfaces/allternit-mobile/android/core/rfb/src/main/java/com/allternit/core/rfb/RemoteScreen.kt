package com.allternit.core.rfb

import android.content.Context
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import androidx.compose.material3.Icon
import androidx.compose.ui.res.painterResource
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/*
 * The VNC canvas shared by a bot's desktop and a computer's screen (iOS BotDesktopView +
 * ComputerScreenCanvas). TOUCH MAPPING (touch has no hover), same as iOS:
 *   tap = left click · two-finger tap = right click · long-press = right click
 *   one-finger drag at fit = pointer move with the left button held
 *   one-finger drag zoomed in = pan · pinch = zoom (1…8× the fit scale; back to 1× resets pan)
 * Keyboard: a hidden key-capture view (software + hardware keyboards → X11 keysyms).
 */
@Composable
fun RemoteScreenCanvas(
    session: RfbSession,
    keyboardVisible: Boolean,
    modifier: Modifier = Modifier,
    bottomOverlay: @Composable () -> Unit = {},
) {
    var size by remember { mutableStateOf(IntSize.Zero) }
    var userScale by remember { mutableFloatStateOf(1f) }
    var pan by remember { mutableStateOf(Offset.Zero) }
    val touchSlop = LocalViewConfiguration.current.touchSlop

    fun displayedRect(): Rect {
        val fw = session.framebufferWidth.toFloat()
        val fh = session.framebufferHeight.toFloat()
        if (fw <= 0f || fh <= 0f || size.width == 0) return Rect.Zero
        val fit = min(size.width / fw, size.height / fh)
        val w = fw * fit * userScale
        val h = fh * fit * userScale
        val left = (size.width - w) / 2f + pan.x
        val top = (size.height - h) / 2f + pan.y
        return Rect(left, top, left + w, top + h)
    }

    /** Canvas point → framebuffer pixel; null in the letterbox (no stray clicks). */
    fun fbPoint(p: Offset): Pair<Int, Int>? {
        val r = displayedRect()
        if (r.width <= 0f) return null
        val x = (p.x - r.left) / r.width * session.framebufferWidth
        val y = (p.y - r.top) / r.height * session.framebufferHeight
        if (x < 0 || y < 0 || x >= session.framebufferWidth || y >= session.framebufferHeight) return null
        return x.toInt() to y.toInt()
    }

    fun click(p: Offset, mask: Int) {
        fbPoint(p)?.let { (x, y) -> session.tap(x, y, mask) }
    }

    Box(
        modifier
            .fillMaxSize()
            .background(Color.Black)
            .onSizeChanged { size = it }
            .testTag("rfbCanvas")
            .pointerInput(session) {
                awaitPointerEventScope {
                    while (true) {
                        val first = awaitPointerEvent()
                        val down = first.changes.firstOrNull { it.pressed } ?: continue
                        val startTime = down.uptimeMillis
                        val startPos = down.position
                        var moved = false
                        var multi = false
                        var multiMoved = false
                        var dragging = false
                        var longPressed = false
                        var lastPos = startPos
                        var lastSpan = 0f
                        var lastCentroid = Offset.Zero
                        val panAtStart = pan
                        while (true) {
                            val event = withTimeoutOrNull(if (!moved && !multi && !longPressed) 500L else Long.MAX_VALUE) { awaitPointerEvent() }
                            if (event == null) {
                                // Held still for 0.5 s: right click.
                                longPressed = true
                                click(startPos, 4)
                                continue
                            }
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.size >= 2) {
                                val a = pressed[0].position
                                val b = pressed[1].position
                                val span = hypot(a.x - b.x, a.y - b.y)
                                val centroid = Offset((a.x + b.x) / 2f, (a.y + b.y) / 2f)
                                if (!multi) {
                                    multi = true
                                    if (dragging) {
                                        fbPoint(lastPos)?.let { (x, y) -> session.sendPointer(x, y, 0) }
                                        dragging = false
                                    }
                                } else if (lastSpan > 0f) {
                                    val zoom = span / lastSpan
                                    val delta = centroid - lastCentroid
                                    if (abs(zoom - 1f) > 0.01f || delta.getDistance() > 1f) multiMoved = true
                                    userScale = (userScale * zoom).coerceIn(1f, 8f)
                                    pan += delta
                                }
                                lastSpan = span
                                lastCentroid = centroid
                                event.changes.forEach { it.consume() }
                                continue
                            }
                            if (pressed.isEmpty()) {
                                if (multi) {
                                    if (!multiMoved && event.changes.first().uptimeMillis - startTime < 350) click(lastCentroid, 4)
                                    if (userScale <= 1.02f) {
                                        userScale = 1f
                                        pan = Offset.Zero
                                    }
                                } else if (dragging) {
                                    fbPoint(lastPos)?.let { (x, y) -> session.sendPointer(x, y, 0) }
                                } else if (!moved && !longPressed) {
                                    click(startPos, 1)
                                }
                                break
                            }
                            if (multi || longPressed) continue
                            val change = pressed.first()
                            val pos = change.position
                            if (!moved && (pos - startPos).getDistance() > touchSlop) moved = true
                            if (moved && change.positionChanged()) {
                                if (userScale > 1.02f) {
                                    pan = panAtStart + (pos - startPos)
                                } else if (session.allowsInput) {
                                    if (!dragging) {
                                        dragging = true
                                        fbPoint(startPos)?.let { (x, y) -> session.sendPointer(x, y, 1) }
                                    }
                                    fbPoint(pos)?.let { (x, y) -> session.sendPointer(x, y, 1) }
                                }
                                change.consume()
                            }
                            lastPos = pos
                        }
                    }
                }
            },
    ) {
        val bitmap = session.frame
        if (bitmap != null) {
            val image = remember(bitmap) { bitmap.asImageBitmap() }
            Canvas(Modifier.fillMaxSize()) {
                val r = displayedRect()
                if (r.width > 0f) {
                    drawImage(
                        image = image,
                        srcOffset = IntOffset.Zero,
                        srcSize = IntSize(image.width, image.height),
                        dstOffset = IntOffset(r.left.toInt(), r.top.toInt()),
                        dstSize = IntSize(r.width.toInt(), r.height.toInt()),
                    )
                }
            }
        }
        KeyCapture(
            visible = keyboardVisible && session.allowsInput,
            onKeysym = { keysym, down -> session.sendKey(keysym, down) },
        )
        if (session.state == RfbSession.State.Live) {
            Box(Modifier.fillMaxSize().padding(bottom = 20.dp), contentAlignment = Alignment.BottomCenter) { bottomOverlay() }
        }
        RfbStateOverlay(session)
    }
}

/** Blocks the canvas until frames flow; after a failure it owns Retry; asks for a sign-in. */
@Composable
fun RfbStateOverlay(session: RfbSession, connectingText: String = "Connecting to desktop…", failedTitle: String = "Couldn't show the desktop") {
    @Composable
    fun Cover(content: @Composable () -> Unit) {
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.9f)).padding(32.dp), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }
        }
    }
    when (val s = session.state) {
        RfbSession.State.Connecting -> Cover {
            CircularProgressIndicator(color = Color.White, modifier = Modifier.size(28.dp), strokeWidth = 2.dp)
            Text(connectingText, color = Color.White, fontSize = 15.sp)
        }
        is RfbSession.State.Reconnecting -> Cover {
            CircularProgressIndicator(color = Color.White, modifier = Modifier.size(28.dp), strokeWidth = 2.dp)
            Text("Connection lost — reconnecting (${s.attempt}/${RfbSession.MAX_RECONNECT_ATTEMPTS})…", color = Color.White, fontSize = 15.sp)
        }
        is RfbSession.State.Failed -> Cover {
            FriendlyState(
                style = FriendlyStyle.Error,
                title = failedTitle,
                message = s.message,
                actionTitle = "Retry",
                onAction = { session.retry() },
                modifier = Modifier.testTag("rfbFailed"),
            ) { Icon(painterResource(R.drawable.rfb_x), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp)) }
        }
        is RfbSession.State.NeedsCredentials -> Cover {
            RfbCredentialsForm(s.needsUsername, s.message) { u, p -> session.submitCredentials(u, p) }
        }
        RfbSession.State.Live -> if (session.frame == null) Cover {
            CircularProgressIndicator(color = Color.White, modifier = Modifier.size(28.dp), strokeWidth = 2.dp)
            Text("Waiting for the first frame…", color = Color.White, fontSize = 15.sp)
        }
    }
}

/**
 * Sign-in for a computer's own screen sharing (iOS RFBCredentialsForm): a Mac asks for the account
 * name and password of an account on that Mac; other VNC servers ask for a password.
 */
@Composable
fun RfbCredentialsForm(needsUsername: Boolean, message: String?, onSubmit: (String, String) -> Unit) {
    val c = AllternitTheme.colors
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    val canSubmit = password.isNotEmpty() && (!needsUsername || username.isNotBlank())
    fun submit() {
        if (!canSubmit) return
        onSubmit(username.trim(), password)
        password = ""
    }
    Column(
        Modifier.widthIn(max = 360.dp).clip(RoundedCornerShape(16.dp)).background(c.bgPanel).padding(20.dp).testTag("rfbCredentials"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Sign in to this computer", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Text(
            if (needsUsername) "Use the account name and password you log in to this computer with (not your Allternit sign-in)."
            else "Its screen sharing asks for a password.",
            color = c.textSecondary, fontSize = 13.sp,
        )
        if (message != null) Text(message, color = AllternitStatic.StatusError, fontSize = 13.sp)
        if (needsUsername) {
            OutlinedTextField(
                value = username, onValueChange = { username = it }, singleLine = true,
                placeholder = { Text("Account name") },
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Text, imeAction = ImeAction.Next),
                modifier = Modifier.fillMaxWidth().testTag("rfbUsername"),
            )
        }
        OutlinedTextField(
            value = password, onValueChange = { password = it }, singleLine = true,
            placeholder = { Text("Password") },
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { submit() }),
            modifier = Modifier.fillMaxWidth().testTag("rfbPassword"),
        )
        Button(
            onClick = { submit() }, enabled = canSubmit,
            colors = ButtonDefaults.buttonColors(containerColor = c.accentPrimary, contentColor = c.onAccent),
            modifier = Modifier.fillMaxWidth().height(48.dp).testTag("rfbConnect"),
        ) { Text("Connect", fontWeight = FontWeight.SemiBold) }
    }
}

/**
 * The keyboard first responder (iOS DesktopKeyboardCaptureView): an invisible 1 dp view whose input
 * connection turns software-keyboard text into keysyms and whose key events carry hardware keys.
 */
@Composable
private fun KeyCapture(visible: Boolean, onKeysym: (Long, Boolean) -> Unit) {
    val latest = remember { mutableStateOf(onKeysym) }
    latest.value = onKeysym
    var view by remember { mutableStateOf<KeyCaptureView?>(null) }
    AndroidView(
        factory = { ctx -> KeyCaptureView(ctx) { k, d -> latest.value(k, d) }.also { view = it } },
        modifier = Modifier.size(1.dp),
    )
    LaunchedEffect(visible, view) {
        val v = view ?: return@LaunchedEffect
        val imm = v.context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        if (visible) {
            v.requestFocus()
            imm.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT)
        } else {
            imm.hideSoftInputFromWindow(v.windowToken, 0)
            v.clearFocus()
        }
    }
}

private class KeyCaptureView(context: Context, private val onKeysym: (Long, Boolean) -> Unit) : View(context) {
    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // Plain passthrough typing: no suggestions or autocorrect rewriting what the desktop gets.
        outAttrs.inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            android.text.InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN
        return object : BaseInputConnection(this, false) {
            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                typeText(text.toString())
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(max(1, beforeLength)) { press(RfbKeysym.BACK_SPACE) }
                return true
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean {
                handleKey(event)
                return true
            }
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean = handleKey(event) || super.onKeyUp(keyCode, event)

    private fun handleKey(event: KeyEvent): Boolean {
        val down = event.action == KeyEvent.ACTION_DOWN
        if (event.action != KeyEvent.ACTION_DOWN && event.action != KeyEvent.ACTION_UP) return false
        special(event.keyCode)?.let {
            onKeysym(it, down)
            return true
        }
        val cp = event.unicodeChar
        val keysym = if (cp != 0) RfbKeysym.forCodePoint(cp) else null
        if (keysym != null) {
            onKeysym(keysym, down)
            return true
        }
        return false
    }

    private fun typeText(text: String) {
        var i = 0
        while (i < text.length) {
            val cp = text.codePointAt(i)
            val keysym = when (cp) {
                '\n'.code, '\r'.code -> RfbKeysym.RETURN
                '\t'.code -> RfbKeysym.TAB
                else -> RfbKeysym.forCodePoint(cp)
            }
            keysym?.let { press(it) }
            i += Character.charCount(cp)
        }
    }

    private fun press(keysym: Long) {
        onKeysym(keysym, true)
        onKeysym(keysym, false)
    }

    private fun special(code: Int): Long? = when (code) {
        KeyEvent.KEYCODE_DPAD_LEFT -> RfbKeysym.LEFT
        KeyEvent.KEYCODE_DPAD_UP -> RfbKeysym.UP
        KeyEvent.KEYCODE_DPAD_RIGHT -> RfbKeysym.RIGHT
        KeyEvent.KEYCODE_DPAD_DOWN -> RfbKeysym.DOWN
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> RfbKeysym.RETURN
        KeyEvent.KEYCODE_TAB -> RfbKeysym.TAB
        KeyEvent.KEYCODE_ESCAPE -> RfbKeysym.ESCAPE
        KeyEvent.KEYCODE_DEL -> RfbKeysym.BACK_SPACE
        KeyEvent.KEYCODE_FORWARD_DEL -> RfbKeysym.DELETE
        KeyEvent.KEYCODE_SHIFT_LEFT -> RfbKeysym.SHIFT_LEFT
        KeyEvent.KEYCODE_SHIFT_RIGHT -> RfbKeysym.SHIFT_RIGHT
        KeyEvent.KEYCODE_CTRL_LEFT -> RfbKeysym.CONTROL_LEFT
        KeyEvent.KEYCODE_CTRL_RIGHT -> RfbKeysym.CONTROL_RIGHT
        KeyEvent.KEYCODE_ALT_LEFT -> RfbKeysym.ALT_LEFT
        KeyEvent.KEYCODE_META_LEFT -> RfbKeysym.SUPER_LEFT
        KeyEvent.KEYCODE_META_RIGHT -> RfbKeysym.SUPER_RIGHT
        else -> null
    }
}
