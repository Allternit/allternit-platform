package com.allternit.feature.office.sign

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Typeface
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.SignDetail
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.PolishHaptic
import com.allternit.feature.office.R
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Who is asking (iOS `SignSheetView.Mode`). */
enum class SignMode {
    /** A pending record someone else asked for. */
    Request,

    /** The user signs their own document (a record is created). */
    Self,

    /** Signed out / offline: places the image only, no record, and the sheet says so. */
    Local,
}

/** `method` values the sign route accepts. */
object SignMethods {
    const val DRAW = "draw"
    const val TYPE = "type"
    const val UPLOAD = "upload"
}

private val Warn = Color(0xFFE0612F)
private val WriteEase = CubicBezierEasing(0.4f, 0f, 0.6f, 1f)
private const val WRITE_MS = 850
private const val WRITE_HOLD_MS = 1050L
private enum class Step { Review, Sign }
private enum class Tab { Draw, Type }

/**
 * Review & sign sheet (iOS `SignSheetView`, web `after.tsx`): a review step with rows, then a sign step with a Draw | Type
 * segmented control, a pad on the neutral fill with an Undo chip, a small "Upload image" link (photo picker), the consent
 * line, and a black pill that stays disabled until there is ink. Tapping it clears the label and writes "Signed" in Sacramento
 * with a left-to-right clip reveal, then calls [onSign]. [mode] Local says "Place signature" and that no record is kept.
 *
 * The sheet never uploads. In [onSign], dismiss it (stop composing it) and start [SignFlight] from [sourceRectInWindow]
 * (screen coordinates of the ink, the flight's `from`); [png] is at most `SignRepository.MAX_SIGNATURE_BYTES`, [method] one
 * of [SignMethods]. With no details, summary or [sendTo] it opens straight on the sign step.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SignSheet(
    title: String,
    summary: String? = null,
    details: List<SignDetail> = emptyList(),
    sendTo: String? = null,
    mode: SignMode = SignMode.Request,
    onSign: (png: ByteArray, bitmap: Bitmap, method: String, sourceRectInWindow: Rect) -> Unit,
    onClose: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val context = LocalContext.current
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    val reduce = rememberReduceMotion()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val bare = details.isEmpty() && summary.isNullOrBlank() && sendTo.isNullOrBlank()
    val script = remember { FontFamily(Font(R.font.sacramento_regular)) }
    val typeface = remember { runCatching { context.resources.getFont(R.font.sacramento_regular) }.getOrDefault(Typeface.create("cursive", Typeface.NORMAL)) }
    val pad = remember { SignPadState() }

    var step by remember { mutableStateOf(if (bare) Step.Sign else Step.Review) }
    var tab by remember { mutableStateOf(Tab.Draw) }
    var typed by remember { mutableStateOf("") }
    var uploaded by remember { mutableStateOf<Bitmap?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var padRect by remember { mutableStateOf(Rect.Zero) }
    var writing by remember { mutableStateOf(false) }
    var built by remember { mutableStateOf<Built?>(null) }
    val typeFocus = remember { FocusRequester() }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri: Uri? ->
        if (uri != null) scope.launch {
            val bmp = withContext(Dispatchers.IO) { decodeUploaded(context, uri) }
            if (bmp == null) failure = "That image could not be opened." else { failure = null; uploaded = bmp }
        }
    }

    val hasInk = uploaded != null || if (tab == Tab.Draw) !pad.isEmpty else typed.isNotBlank()

    fun signAndSend() {
        if (!hasInk || writing) return
        val b = buildResult(uploaded, tab, pad, typed, typeface, padRect, density.density)
        if (b == null) { failure = "The signature could not be saved. Try again."; return }
        failure = null
        PolishHaptic.perform(view, PolishHaptic.Kind.Medium)
        built = b
        writing = true
    }

    // The "Signed" write-on plays, then the caller takes over.
    LaunchedEffect(built) {
        val b = built ?: return@LaunchedEffect
        if (!reduce) delay(WRITE_HOLD_MS)
        onSign(b.png, b.bitmap, b.method, b.rect)
    }

    ModalBottomSheet(
        onDismissRequest = { if (!writing) onClose() },
        sheetState = sheetState,
        containerColor = c.bgPrimary,
        shape = RoundedCornerShape(topStart = 34.dp, topEnd = 34.dp),
        dragHandle = null,
    ) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().imePadding().padding(horizontal = 16.dp).padding(top = 8.dp, bottom = 16.dp).testTag("signSheet"),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(Modifier.width(36.dp).height(5.dp).clip(CircleShape).background(c.borderStrong))
            Spacer(Modifier.height(12.dp))
            AnimatedContent(
                targetState = step,
                transitionSpec = { fadeIn(tween(if (reduce) 0 else 220)) togetherWith fadeOut(tween(if (reduce) 0 else 120)) },
                label = "signStep",
            ) { s ->
                if (s == Step.Review) {
                    ReviewStep(title, summary, details, sendTo, onClose) {
                        PolishHaptic.perform(view, PolishHaptic.Kind.Light)
                        step = Step.Sign
                    }
                } else {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("Your signature", color = c.textPrimary, fontSize = 26.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.4).sp, modifier = Modifier.semantics { heading() })
                        Text("Draw your name or type it below", color = c.textSecondary, fontSize = 14.sp)
                        Segmented(tab, enabled = !writing, modifier = Modifier.padding(top = 12.dp)) { t ->
                            if (t != tab) {
                                PolishHaptic.perform(view, PolishHaptic.Kind.Selection)
                                tab = t
                                uploaded = null
                            }
                        }
                        PadArea(
                            tab = tab, state = pad, typed = typed, uploaded = uploaded, writing = writing, script = script,
                            onTyped = { typed = it }, typeFocus = typeFocus,
                            onUndo = { PolishHaptic.perform(view, PolishHaptic.Kind.Light); pad.undo() },
                            onRect = { padRect = it },
                            modifier = Modifier.padding(top = 8.dp),
                        )
                        LaunchedEffect(tab) { if (tab == Tab.Type) runCatching { typeFocus.requestFocus() } }
                        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                if (uploaded == null) "Upload image" else "Replace image", color = c.textSecondary, fontSize = 12.5.sp, textDecoration = TextDecoration.Underline,
                                modifier = Modifier.clickable(enabled = !writing, role = Role.Button) { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }.padding(vertical = 8.dp).testTag("signUpload"),
                            )
                            if (uploaded != null) {
                                Text("Remove", color = c.textSecondary, fontSize = 12.5.sp,
                                    modifier = Modifier.padding(start = 12.dp).clickable(enabled = !writing, role = Role.Button) { uploaded = null }.padding(vertical = 8.dp))
                            }
                        }
                        Text(
                            if (mode == SignMode.Local) "You are signed out, so this places your signature on the document only. No signing record is kept." else SignRepository.CONSENT_TEXT,
                            color = c.textSecondary, fontSize = 11.5.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 4.dp).testTag("signConsent"),
                        )
                        failure?.let { Text(it, color = Warn, fontSize = 12.5.sp, modifier = Modifier.padding(top = 4.dp).testTag("signFailure")) }
                        Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Pill("Close", filled = false, enabled = !writing, modifier = Modifier.width(96.dp).testTag("signClose")) { if (bare) onClose() else step = Step.Review }
                            SignButton(
                                label = if (mode == SignMode.Local) "Place signature" else "Sign & send",
                                hasInk = hasInk, writing = writing, reduce = reduce, script = script, modifier = Modifier.weight(1f),
                            ) { signAndSend() }
                        }
                    }
                }
            }
        }
    }
}

private class Built(val png: ByteArray, val bitmap: Bitmap, val method: String, val rect: Rect)

private fun buildResult(uploaded: Bitmap?, tab: Tab, pad: SignPadState, typed: String, typeface: Typeface, padRect: Rect, density: Float): Built? {
    val method: String
    val source: Bitmap
    val rect: Rect
    when {
        uploaded != null -> {
            method = SignMethods.UPLOAD
            source = uploaded.scaledToMaxSide(800)
            rect = fitRect(source.width.toFloat(), source.height.toFloat(), padRect.deflate(16f * density))
        }
        tab == Tab.Draw -> {
            method = SignMethods.DRAW
            val ink = renderDrawnInk(pad.strokes.toList(), strokeWidthPx = 3f * density) ?: return null
            source = ink.bitmap
            rect = Rect(padRect.left + ink.boundsInPad.left, padRect.top + ink.boundsInPad.top, padRect.left + ink.boundsInPad.right, padRect.top + ink.boundsInPad.bottom)
        }
        else -> {
            method = SignMethods.TYPE
            source = renderTypedInk(typed, typeface, textSizePx = 54f * density) ?: return null
            rect = fitRect(source.width.toFloat(), source.height.toFloat(), padRect.deflate(12f * density))
        }
    }
    val (png, encoded) = encodeSignaturePng(source) ?: return null
    return Built(png, encoded, method, rect)
}

private fun decodeUploaded(context: android.content.Context, uri: Uri): Bitmap? = runCatching {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > 1600) sample *= 2
    val decoded = context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample }) }
    // Photos are stored sideways with an EXIF orientation tag; BitmapFactory ignores it (ImageDecoder needs API 28, minSdk is 26).
    val orientation = runCatching {
        context.contentResolver.openInputStream(uri)?.use { android.media.ExifInterface(it).getAttributeInt(android.media.ExifInterface.TAG_ORIENTATION, android.media.ExifInterface.ORIENTATION_NORMAL) }
    }.getOrNull() ?: android.media.ExifInterface.ORIENTATION_NORMAL
    val m = android.graphics.Matrix()
    when (orientation) {
        android.media.ExifInterface.ORIENTATION_ROTATE_90 -> m.postRotate(90f)
        android.media.ExifInterface.ORIENTATION_ROTATE_180 -> m.postRotate(180f)
        android.media.ExifInterface.ORIENTATION_ROTATE_270 -> m.postRotate(270f)
        android.media.ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> m.postScale(-1f, 1f)
        android.media.ExifInterface.ORIENTATION_FLIP_VERTICAL -> m.postScale(1f, -1f)
        android.media.ExifInterface.ORIENTATION_TRANSPOSE -> { m.postRotate(90f); m.postScale(-1f, 1f) }
        android.media.ExifInterface.ORIENTATION_TRANSVERSE -> { m.postRotate(270f); m.postScale(-1f, 1f) }
        else -> return@runCatching decoded
    }
    decoded?.let { Bitmap.createBitmap(it, 0, 0, it.width, it.height, m, true) }
}.getOrNull()

@Composable
private fun ReviewStep(title: String, summary: String?, details: List<SignDetail>, sendTo: String?, onClose: () -> Unit, onAdd: () -> Unit) {
    val c = AllternitTheme.colors
    val rows = details.take(8).map { it.label to it.value } + listOfNotNull(sendTo?.takeIf { it.isNotBlank() }?.let { "Send signed copy to" to it })
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
        Text("⚠ Awaiting your signature", color = Warn, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text(title, color = c.textPrimary, fontSize = 26.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.4).sp, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 2.dp).semantics { heading() })
        if (!summary.isNullOrBlank()) Text(summary, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(horizontal = 12.dp))
        if (rows.isNotEmpty()) {
            Column(Modifier.padding(top = 12.dp).fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(c.bgTertiary).padding(horizontal = 16.dp)) {
                rows.forEachIndexed { i, (label, value) ->
                    if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                    Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), verticalAlignment = Alignment.Top) {
                        Text(label, color = c.textSecondary, fontSize = 14.sp)
                        Spacer(Modifier.width(12.dp))
                        Text(value, color = c.textPrimary, fontSize = 14.sp, textAlign = TextAlign.End, modifier = Modifier.weight(1f))
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Pill("Close", filled = false, modifier = Modifier.width(96.dp).testTag("signReviewClose"), onClick = onClose)
            Pill("Add signature", filled = true, modifier = Modifier.weight(1f).testTag("signAdd"), onClick = onAdd)
        }
    }
}

@Composable
private fun Pill(label: String, filled: Boolean, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier.height(44.dp).clip(CircleShape).background(if (filled) c.textPrimary else c.bgTertiary)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (filled) c.bgPrimary else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
private fun Segmented(selected: Tab, enabled: Boolean, modifier: Modifier = Modifier, onPick: (Tab) -> Unit) {
    val c = AllternitTheme.colors
    Row(modifier.fillMaxWidth().clip(CircleShape).background(c.bgTertiary).padding(3.dp)) {
        listOf(Tab.Draw to "Draw signature", Tab.Type to "Type signature").forEach { (t, label) ->
            val on = t == selected
            Box(
                Modifier.weight(1f).height(36.dp).clip(CircleShape).background(if (on) c.bgPrimary else Color.Transparent)
                    .clickable(enabled = enabled, role = Role.Tab) { onPick(t) }.semantics { this.selected = on }.testTag(if (t == Tab.Draw) "signTabDraw" else "signTabType"),
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = if (on) c.textPrimary else c.textSecondary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}

@Composable
private fun PadArea(
    tab: Tab, state: SignPadState, typed: String, uploaded: Bitmap?, writing: Boolean, script: FontFamily,
    onTyped: (String) -> Unit, typeFocus: FocusRequester, onUndo: () -> Unit, onRect: (Rect) -> Unit, modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val density = LocalDensity.current.density
    Box(
        modifier.fillMaxWidth().height(168.dp).clip(RoundedCornerShape(20.dp)).background(c.bgTertiary).testTag("signPad")
            .onGloballyPositioned {
                val b = it.boundsInWindow()
                val loc = IntArray(2)
                view.getLocationOnScreen(loc)
                onRect(b.translate(loc[0].toFloat(), loc[1].toFloat()))
            },
    ) {
        when {
            uploaded != null -> Image(uploaded.asImageBitmap(), "Uploaded signature image", Modifier.fillMaxSize().padding(16.dp), contentScale = ContentScale.Fit)
            tab == Tab.Draw -> {
                SignPad(state, strokeWidthPx = 3f * density, enabled = !writing)
                if (state.isEmpty) Text("Sign here", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.align(Alignment.Center))
            }
            else -> Column(Modifier.fillMaxSize()) {
                Box(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
                    Text(
                        typed.ifEmpty { " " }, color = c.textPrimary, fontFamily = script, fontSize = (54 - maxOf(0, typed.length - 12) * 1.5f).coerceAtLeast(26f).sp,
                        maxLines = 1, overflow = TextOverflow.Clip, softWrap = false,
                    )
                }
                Box(
                    Modifier.padding(start = 12.dp, end = 12.dp, bottom = 12.dp).fillMaxWidth().height(36.dp).clip(CircleShape).background(c.bgPrimary).padding(horizontal = 12.dp),
                    contentAlignment = Alignment.CenterStart,
                ) {
                    if (typed.isEmpty()) Text("Type your full name", color = c.textTertiary, fontSize = 14.sp)
                    BasicTextField(
                        value = typed, onValueChange = { if (!writing) onTyped(it.take(60)) }, singleLine = true,
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp),
                        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words),
                        cursorBrush = androidx.compose.ui.graphics.SolidColor(c.textPrimary),
                        modifier = Modifier.fillMaxWidth().focusRequester(typeFocus).testTag("signTypeField"),
                    )
                }
            }
        }
        if (tab == Tab.Draw && uploaded == null && !state.isEmpty && !writing) {
            Box(
                Modifier.align(Alignment.TopEnd).padding(10.dp).height(28.dp).clip(CircleShape).background(c.bgPrimary)
                    .clickable(role = Role.Button, onClick = onUndo).padding(horizontal = 10.dp).testTag("signUndo"),
                contentAlignment = Alignment.Center,
            ) { Text("↶ Undo", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium) }
        }
    }
}


@Composable
private fun SignButton(label: String, hasInk: Boolean, writing: Boolean, reduce: Boolean, script: FontFamily, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val bg by animateColorAsState(if (hasInk) c.textPrimary else c.textPrimary.copy(alpha = 0.18f), tween(if (reduce) 0 else 250), label = "signButtonBg")
    Box(
        modifier.height(44.dp).clip(CircleShape).background(bg)
            .clickable(enabled = hasInk && !writing, role = Role.Button, onClick = onClick)
            .semantics { if (!hasInk) disabled() }.testTag("signAction"),
        contentAlignment = Alignment.Center,
    ) {
        if (writing) SignedWriteOn(reduce, script) else Text(label, color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

/** "Signed" in Sacramento, revealed left to right by a growing clip (~0.85 s). Reduced motion shows it at once. */
@Composable
private fun SignedWriteOn(reduce: Boolean, script: FontFamily) {
    val c = AllternitTheme.colors
    val progress = remember { Animatable(if (reduce) 1f else 0f) }
    LaunchedEffect(Unit) { if (!reduce) progress.animateTo(1f, tween(WRITE_MS, easing = WriteEase)) }
    Text(
        "Signed", color = c.bgPrimary, fontFamily = script, fontSize = 28.sp,
        modifier = Modifier.semantics { contentDescription = "Signed" }.drawWithContent {
            clipRect(right = size.width * progress.value) { this@drawWithContent.drawContent() }
        },
    )
}
