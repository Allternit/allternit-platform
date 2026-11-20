package com.allternit.feature.chat.games

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.view.View
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.key.utf16CodePoint
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.voice.rememberReduceMotion

/** The window's dark surface and the status dots (iOS #0b1220, #34d399, #D97757). */
private val GameSurface = Color(0xFF0B1220)
private val ReadyGreen = Color(0xFF34D399)
private val WorkingCoral = Color(0xFFD97757)
private val Ink900 = Color(0xFF18181B)

/** The hidden field keeps one invisible character so a backspace on an "empty" field still arrives. */
private const val SENTINEL = "​"
private val FALLBACK_RACERS = listOf("Scout", "Pixel", "Byte", "Nova")
/** What you steer swims just above the finger, where you can see it (dp). */
private const val FINGER_LIFT = 40.0

/**
 * Mounted once at the shell root (iOS WaitingGamesLayer in WorkspaceShell). Once a game is opened the window
 * stays composed and is only hidden when closed, so every game picks up where you left it. [racers]: the
 * person's bots for Bot Race, read once when the window first opens.
 */
@Composable
fun WaitingGamesLayer(racers: () -> List<WaitingGameRacer>) {
    val context = LocalContext.current
    remember { WaitingGamesStore.attach(context) }
    val store = WaitingGamesStore
    var started by remember { mutableStateOf(false) }
    val reduceMotion = rememberReduceMotion()
    LaunchedEffect(store.open) { if (store.open) started = true }
    val shown by animateFloatAsState(if (store.open) 1f else 0f, tween(if (reduceMotion) 0 else 200), label = "waitingGamesShown")
    if (store.open || started) {
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer {
                    alpha = shown
                    val s = 0.95f + 0.05f * shown
                    scaleX = s
                    scaleY = s
                }
                .then(if (store.open) Modifier else Modifier.clearAndSetSemantics {}),
        ) {
            WaitingGamesWindow(racers)
        }
    }
    // Back: a full-screen game gets small first, then the window closes (iOS Escape).
    BackHandler(enabled = store.open) {
        if (store.expanded) store.expanded = false else store.close()
    }
}

/** Touch handlers only while the window is open, so a hidden window never takes the shell's taps. */
private fun Modifier.tapIf(open: Boolean, label: String, onClick: () -> Unit): Modifier =
    if (open) clickable(role = Role.Button, onClickLabel = label, onClick = onClick) else this

@Composable
private fun WaitingGamesWindow(racers: () -> List<WaitingGameRacer>) {
    val store = WaitingGamesStore
    val context = LocalContext.current
    val screen = LocalDensity.current
    val focusManager = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val reduceMotion = rememberReduceMotion()
    val engine = remember {
        // The racers are read once, when the window opens: the first 4 bots.
        val bots = racers().take(4).map { WaitingGameRacer(it.name.take(8), it.color) }
        WaitingGamesEngine(bots.ifEmpty { FALLBACK_RACERS.map { WaitingGameRacer(it) } }, WaitingGamesStore.attach(context))
    }
    var readyDismissed by remember { mutableStateOf(false) }
    val ready = store.work == WaitingGamesStore.Work.Done && !readyDismissed
    /** On screen and not waiting on "reply ready": the game runs. */
    val live = store.open && !ready
    /** Site games stay loaded once played, so switching back resumes too. */
    val visited = remember { mutableStateListOf<WaitingGameId>() }
    val typingFocus = remember { FocusRequester() }
    val keysFocus = remember { FocusRequester() }
    var focusAsk by remember { mutableStateOf(0) }
    val typedGame = engine.wantsKeyboard && store.game.site == null

    // The keyboard for typed games (Bot Race), hardware keys for the rest.
    LaunchedEffect(focusAsk, store.open, typedGame) {
        if (!store.open) {
            focusManager.clearFocus(force = true)
            keyboard?.hide()
            return@LaunchedEffect
        }
        runCatching {
            if (typedGame) { typingFocus.requestFocus(); keyboard?.show() } else { keysFocus.requestFocus(); keyboard?.hide() }
        }
    }
    LaunchedEffect(store.game) {
        engine.pick(store.game)
        if (store.game.site != null && store.game !in visited) visited.add(store.game)
        focusAsk += 1
    }
    LaunchedEffect(live) { engine.updatePlaying(live) }
    LaunchedEffect(store.work) { if (store.work == WaitingGamesStore.Work.Working) readyDismissed = false }
    DisposableEffect(engine) { onDispose { engine.updatePlaying(false) } }

    // The frame clock: runs only while the game plays (iOS TimelineView paused: !playing).
    var frame by remember { mutableLongStateOf(0L) }
    LaunchedEffect(engine.playing) {
        if (engine.playing) while (true) withFrameNanos { frame = it }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        val expanded = store.expanded
        val covered = with(screen) { WindowInsets.ime.getBottom(screen).toDp() }
        val navBottom = with(screen) { WindowInsets.navigationBars.getBottom(screen).toDp() }
        val statusTop = with(screen) { WindowInsets.statusBars.getTop(screen).toDp() }
        // Typed games sit in the room above the keyboard, so the word on top stays on screen.
        val aboveKeyboard = covered > 0.dp && engine.wantsKeyboard
        val height: Dp? = when {
            aboveKeyboard -> max(160.dp, maxHeight - covered - (if (expanded) 0.dp else 8.dp))
            expanded -> null
            else -> maxHeight * 0.46f
        }
        // Docked above the message box on a phone.
        val bottom = when {
            aboveKeyboard -> covered + (if (expanded) 0.dp else 4.dp)
            expanded -> 0.dp
            else -> 168.dp + navBottom
        }
        val shape = RoundedCornerShape(if (expanded) 0.dp else 16.dp)
        Box(Modifier.fillMaxSize().padding(bottom = bottom), contentAlignment = Alignment.BottomCenter) {
            Column(
                Modifier
                    .padding(horizontal = if (expanded) 0.dp else 8.dp)
                    .fillMaxWidth()
                    .then(if (height != null) Modifier.height(height) else Modifier.fillMaxHeight())
                    .shadow(if (expanded) 0.dp else 24.dp, shape, clip = false)
                    .clip(shape)
                    .border(1.dp, Color.White.copy(alpha = if (expanded) 0f else 0.10f), shape)
                    .background(GameSurface)
                    .padding(top = if (expanded) statusTop else 0.dp)
                    .testTag("waitingGamesWindow")
                    .semantics { contentDescription = "Waiting games" }
                    // Hardware keys (Snake's arrows, Stack's space, Race's letters).
                    .focusRequester(keysFocus)
                    .focusable()
                    .onPreviewKeyEvent { e ->
                        if (!store.open || e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                        val name = when (e.key) {
                            Key.DirectionUp -> "ArrowUp"
                            Key.DirectionDown -> "ArrowDown"
                            Key.DirectionLeft -> "ArrowLeft"
                            Key.DirectionRight -> "ArrowRight"
                            Key.Enter, Key.NumPadEnter -> "Enter"
                            Key.Spacebar -> " "
                            Key.Backspace -> "Backspace"
                            Key.Escape -> {
                                if (store.expanded) store.expanded = false else store.close()
                                return@onPreviewKeyEvent true
                            }
                            else -> e.utf16CodePoint.takeIf { it > 0 }?.let { String(Character.toChars(it)) } ?: return@onPreviewKeyEvent false
                        }
                        engine.key(name)
                    },
            ) {
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    for (id in visited) {
                        val site = id.site ?: continue
                        key(id) {
                            SiteGameView(
                                site = site,
                                playing = live && id == store.game,
                                visible = store.open && id == store.game,
                                modifier = Modifier.padding(top = 44.dp).fillMaxSize(),
                            )
                        }
                    }
                    if (store.game.site == null) {
                        Canvas(
                            Modifier
                                .fillMaxSize()
                                .testTag("waitingGamesCanvas")
                                .then(
                                    if (store.open) Modifier.pointerInput(engine) {
                                        awaitEachGesture {
                                            val down = awaitFirstDown()
                                            // The pointer scope's own density (px per dp).
                                            val d = density
                                            val x = down.position.x / d
                                            val y = down.position.y / d
                                            engine.pointer(x.toDouble(), y - FINGER_LIFT)
                                            engine.click(x.toDouble(), y.toDouble())
                                            down.consume()
                                            while (true) {
                                                val ev = awaitPointerEvent()
                                                val ch = ev.changes.firstOrNull { it.id == down.id } ?: break
                                                if (!ch.pressed) break
                                                engine.pointer((ch.position.x / d).toDouble(), ch.position.y / d - FINGER_LIFT)
                                                ch.consume()
                                            }
                                            focusAsk += 1
                                        }
                                    } else Modifier,
                                ),
                        ) {
                            val t = frame
                            val d = density
                            drawIntoCanvas { canvas ->
                                engine.render(canvas.nativeCanvas, (size.width / d).toDouble(), (size.height / d).toDouble(), d, t / 1e9)
                            }
                        }
                    }
                    Overlays(engine, ready, onBack = { store.close() }, onKeepPlaying = { readyDismissed = true })
                    TopBar(reduceMotion)
                    if (typedGame && store.open) HiddenField(engine, typingFocus)
                }
                val hint = store.game.site?.hint ?: engine.hint
                if (hint.isNotEmpty()) {
                    Text(
                        hint,
                        color = Color.White.copy(alpha = 0.6f),
                        fontSize = 11.5.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun Overlays(engine: WaitingGamesEngine, ready: Boolean, onBack: () -> Unit, onKeepPlaying: () -> Unit) {
    val store = WaitingGamesStore
    if (engine.hud.isNotEmpty()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 10.dp).padding(top = 48.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            engine.hud.forEachIndexed { i, item ->
                Text(
                    item,
                    color = Color.White,
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    fontFamily = FontFamily.Monospace,
                    modifier = Modifier.clip(RoundedCornerShape(50)).background(Color.Black.copy(alpha = 0.3f)).padding(horizontal = 10.dp, vertical = 4.dp),
                )
                if (i < engine.hud.size - 1) Spacer(Modifier.weight(1f))
            }
        }
    }
    if (ready) {
        Box(Modifier.fillMaxSize().padding(horizontal = 24.dp), contentAlignment = Alignment.Center) {
            Column(
                Modifier.clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.5f)).padding(horizontal = 20.dp, vertical = 16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Text("Your reply is ready", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)
                Text("The game is paused where you left it.", color = Color.White.copy(alpha = 0.8f), fontSize = 13.sp, textAlign = TextAlign.Center)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        "Back to chat", color = Ink900, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                        modifier = Modifier.clip(RoundedCornerShape(50)).background(Color.White).tapIf(store.open, "Back to chat", onBack)
                            .padding(horizontal = 14.dp, vertical = 7.dp).testTag("waitingGamesBackToChat"),
                    )
                    Text(
                        "Keep playing", color = Color.White, fontSize = 14.sp,
                        modifier = Modifier.clip(RoundedCornerShape(50)).border(1.dp, Color.White.copy(alpha = 0.4f), RoundedCornerShape(50))
                            .tapIf(store.open, "Keep playing", onKeepPlaying).padding(horizontal = 14.dp, vertical = 7.dp).testTag("waitingGamesKeepPlaying"),
                    )
                }
            }
        }
    } else if (store.game.site == null) {
        val banner = engine.banner ?: return
        // Typed games keep the word on top: their message sits under the lanes.
        Box(
            Modifier.fillMaxSize().padding(horizontal = 24.dp).padding(bottom = if (engine.wantsKeyboard) 10.dp else 0.dp),
            contentAlignment = if (engine.wantsKeyboard) Alignment.BottomCenter else Alignment.Center,
        ) {
            Column(
                Modifier.clip(RoundedCornerShape(16.dp)).background(Color.Black.copy(alpha = 0.4f)).padding(horizontal = 20.dp, vertical = 14.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(banner.title, color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)
                if (banner.sub.isNotEmpty()) Text(banner.sub, color = Color.White, fontSize = 13.sp, textAlign = TextAlign.Center)
            }
        }
    }
}

@Composable
private fun TopBar(reduceMotion: Boolean) {
    val store = WaitingGamesStore
    var menu by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp).padding(top = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        store.work?.let { work ->
            val done = work == WaitingGamesStore.Work.Done
            // The working dot breathes (iOS phaseAnimator); still with reduced motion or once the reply is ready.
            val pulse = if (done || reduceMotion) 1f else rememberInfiniteTransition(label = "waitingGamesDot")
                .animateFloat(1f, 0.35f, infiniteRepeatable(tween(800), RepeatMode.Reverse), label = "dot").value
            Row(
                Modifier.clip(RoundedCornerShape(50)).background(Color.Black.copy(alpha = 0.3f)).padding(horizontal = 10.dp, vertical = 4.dp)
                    .semantics(mergeDescendants = true) {},
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Box(Modifier.size(7.dp).alpha(pulse).clip(RoundedCornerShape(50)).background(if (done) ReadyGreen else WorkingCoral))
                Text(if (done) "Reply ready" else "Working…", color = Color.White, fontSize = 12.sp, maxLines = 1)
            }
        }
        Spacer(Modifier.weight(1f))
        Box {
            Row(
                Modifier.heightIn(min = 36.dp).tapIf(store.open, "Game: ${store.game.title}") { menu = true }.testTag("waitingGamesPicker"),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    Modifier.clip(RoundedCornerShape(50)).background(Color.Black.copy(alpha = 0.3f)).padding(horizontal = 10.dp, vertical = 5.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Text(store.game.title, color = Color.White, fontSize = 12.sp)
                    Icon(painterResource(DesignR.drawable.ph_caret_down), contentDescription = null, tint = Color.White, modifier = Modifier.size(10.dp))
                }
            }
            DropdownMenu(expanded = menu && store.open, onDismissRequest = { menu = false }) {
                for (g in WaitingGameId.entries) {
                    DropdownMenuItem(
                        text = { Text(g.title) },
                        leadingIcon = if (g == store.game) ({ Icon(painterResource(DesignR.drawable.ph_check), contentDescription = null, modifier = Modifier.size(16.dp)) }) else null,
                        onClick = {
                            menu = false
                            store.selectGame(g)
                        },
                        modifier = Modifier.testTag("waitingGame_${g.raw}"),
                    )
                }
            }
        }
        BarButton(
            icon = if (store.expanded) DesignR.drawable.ph_arrows_in else DesignR.drawable.ph_arrows_out,
            label = if (store.expanded) "Make the game small" else "Full screen",
            testTag = "waitingGamesExpand",
            fill = Color.Black.copy(alpha = 0.3f),
            tint = Color.White,
        ) { store.expanded = !store.expanded }
        BarButton(
            icon = DesignR.drawable.ph_x,
            label = "Close the game",
            testTag = "waitingGamesClose",
            fill = Color.White.copy(alpha = 0.9f),
            tint = Ink900,
        ) { store.close() }
    }
}

/** A 30×26 capsule in a 36dp touch target (iOS top bar buttons). */
@Composable
private fun BarButton(icon: Int, label: String, testTag: String, fill: Color, tint: Color, onClick: () -> Unit) {
    Box(
        Modifier.size(36.dp).tapIf(WaitingGamesStore.open, label, onClick).testTag(testTag).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.width(30.dp).height(26.dp).clip(RoundedCornerShape(50)).background(fill), contentAlignment = Alignment.Center) {
            Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(13.dp))
        }
    }
}

/** Typed games (Bot Race) read this field so the phone shows a keyboard. */
@Composable
private fun HiddenField(engine: WaitingGamesEngine, focus: FocusRequester) {
    var typed by remember { mutableStateOf(TextFieldValue(SENTINEL, TextRange(SENTINEL.length))) }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomStart) {
        BasicTextField(
            value = typed,
            onValueChange = { v ->
                val value = v.text
                if (value != SENTINEL) {
                    if (!value.startsWith(SENTINEL)) engine.key("Backspace")
                    else for (ch in value.drop(SENTINEL.length)) engine.key(ch.toString())
                }
                typed = TextFieldValue(SENTINEL, TextRange(SENTINEL.length))
            },
            singleLine = true,
            textStyle = TextStyle(color = Color.Transparent, fontSize = 1.sp),
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Ascii,
                imeAction = ImeAction.Go,
            ),
            keyboardActions = KeyboardActions(onGo = { engine.key("Enter") }),
            modifier = Modifier
                .size(1.dp)
                .alpha(0.01f)
                .focusRequester(focus)
                .testTag("waitingGamesTyping")
                .semantics { contentDescription = "Type the word" },
        )
    }
}

// MARK: - Site games

/**
 * Gizzi RPG and Gizzi Brawl are their own sites (iOS WaitingSiteGameView, web SiteGameFrame). JavaScript runs
 * only in these two views, which load only their own https site: a link anywhere else opens in the browser,
 * and there is no file or content access. Off screen or waiting on "reply ready" the game is told to pause.
 */
@Composable
private fun SiteGameView(site: WaitingSiteGame, playing: Boolean, visible: Boolean, modifier: Modifier) {
    AndroidView(
        modifier = modifier,
        factory = { ctx -> SiteGameHost(ctx, site) },
        update = { host ->
            host.playing = playing
            // INVISIBLE views take no touches: a hidden game never catches the shell's taps.
            host.visibility = if (visible) View.VISIBLE else View.INVISIBLE
        },
        onRelease = { it.release() },
    )
}

/**
 * Holds the game's web view. A landscape-only game in a window taller than wide is turned on its side here,
 * so touches land where the game drew them (iOS Host.layoutSubviews).
 */
@SuppressLint("SetJavaScriptEnabled", "ViewConstructor")
private class SiteGameHost(context: Context, private val site: WaitingSiteGame) : FrameLayout(context) {
    private val web = WebView(context)
    private val siteUri = Uri.parse(site.url)
    private val origin = "${siteUri.scheme}://${siteUri.host}"

    var playing = true
        set(value) {
            if (value != field) {
                field = value
                tell()
            }
        }

    init {
        setBackgroundColor(android.graphics.Color.BLACK)
        clipChildren = true
        web.settings.apply {
            javaScriptEnabled = true
            // The game keeps its save in its own site storage.
            domStorageEnabled = true
            allowFileAccess = false
            allowContentAccess = false
            @Suppress("DEPRECATION")
            allowFileAccessFromFileURLs = false
            @Suppress("DEPRECATION")
            allowUniversalAccessFromFileURLs = false
            mediaPlaybackRequiresUserGesture = false
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            setSupportMultipleWindows(false)
            javaScriptCanOpenWindowsAutomatically = false
            setGeolocationEnabled(false)
        }
        web.setBackgroundColor(android.graphics.Color.BLACK)
        web.isVerticalScrollBarEnabled = false
        web.isHorizontalScrollBarEnabled = false
        web.overScrollMode = View.OVER_SCROLL_NEVER
        web.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                val u = request.url
                val scheme = u.scheme?.lowercase()
                if (scheme == "https" && isGameHost(u.host)) return false
                // Anything else leaves the game: a tapped web link opens in the browser, the rest is refused.
                if (request.isForMainFrame && (scheme == "https" || scheme == "http")) openOutside(u)
                return true
            }

            // Sent again once the game has loaded: it can't hear a message sent before it started.
            override fun onPageFinished(view: WebView, url: String?) { tell() }
        }
        addView(web)
        web.loadUrl(site.url)
    }

    private fun isGameHost(host: String?): Boolean {
        val h = host?.lowercase() ?: return false
        return h == siteUri.host?.lowercase() || h.endsWith(".allternit.com")
    }

    private fun openOutside(uri: Uri) {
        try {
            context.startActivity(Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (_: ActivityNotFoundException) {
        }
    }

    /**
     * The game listens for messages from its parent; in a web view the page is its own parent. The games accept a
     * self-posted message only with `from: "allternit-ios"` (gizzi.js, main.js HostPause), so Android sends that tag too.
     */
    private fun tell() {
        val type = if (playing) "allternit:resume" else "allternit:pause"
        web.evaluateJavascript("window.postMessage({type:'$type', from:'allternit-ios'}, '$origin')", null)
    }

    private fun turned(w: Int, h: Int) = site.landscape && h > w

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val w = MeasureSpec.getSize(widthMeasureSpec)
        val h = MeasureSpec.getSize(heightMeasureSpec)
        setMeasuredDimension(w, h)
        val turn = turned(w, h)
        web.measure(
            MeasureSpec.makeMeasureSpec(if (turn) h else w, MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(if (turn) w else h, MeasureSpec.EXACTLY),
        )
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        val w = right - left
        val h = bottom - top
        val turn = turned(w, h)
        val cw = if (turn) h else w
        val ch = if (turn) w else h
        val x = (w - cw) / 2
        val y = (h - ch) / 2
        web.layout(x, y, x + cw, y + ch)
        web.pivotX = cw / 2f
        web.pivotY = ch / 2f
        web.rotation = if (turn) 90f else 0f
    }

    /** Stops the game's sound when the window goes away. */
    fun release() {
        web.stopLoading()
        web.loadUrl("about:blank")
        removeView(web)
        web.destroy()
    }
}
