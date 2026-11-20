package com.allternit.feature.chat.games

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.chat.voice.rememberReduceMotion
import kotlinx.coroutines.delay
import java.util.UUID

/**
 * The game window opened from a message box, busy or not (iOS WaitingGamesStore, web waiting-games.store.ts).
 * Only the Play button that opened it reports the work, so another chat's composer can't mark this reply
 * ready. [work] is null once that composer is gone. Compose state, so the window and every Play button
 * follow a change at once.
 */
object WaitingGamesStore {
    enum class Work { Working, Done }

    /** Settings › Advanced › Waiting games: the same file and key the Settings toggle has always saved. */
    private const val FILE = "allternit_settings"
    const val ENABLED_KEY = "allternit.waitingGames"
    private const val GAME_KEY = "allternit.waitingGames.game"
    internal const val BEST_KEY = "allternit.waitingGames.best"

    private var prefs: SharedPreferences? = null

    var open by mutableStateOf(false); private set
    var expanded by mutableStateOf(false)
    var game by mutableStateOf(WaitingGameId.Fish); private set
    var work by mutableStateOf<Work?>(null); private set
    var owner by mutableStateOf<String?>(null); private set
    var enabled by mutableStateOf(true); private set

    /** Reads the saved switch and game once; every later call returns the same prefs. */
    fun attach(context: Context): SharedPreferences {
        prefs?.let { return it }
        val p = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        prefs = p
        enabled = p.getBoolean(ENABLED_KEY, true)
        game = WaitingGameId.from(p.getString(GAME_KEY, null)) ?: WaitingGameId.Fish
        return p
    }

    fun isEnabled(context: Context): Boolean {
        attach(context)
        return enabled
    }

    /** The Settings switch. Turning it off closes an open game. */
    fun setEnabled(context: Context, on: Boolean) {
        attach(context).edit().putBoolean(ENABLED_KEY, on).apply()
        enabled = on
        if (!on) close()
    }

    fun openFor(owner: String, busy: Boolean = true) {
        this.owner = owner
        expanded = false
        work = if (busy) Work.Working else null
        open = true
    }

    fun close() {
        open = false
        expanded = false
        owner = null
        work = null
    }

    fun setGame(game: WaitingGameId) {
        prefs?.edit()?.putString(GAME_KEY, game.raw)?.apply()
        this.game = game
    }

    /** A Play button's bot is working (true), finished (false) or the button is gone (null). */
    fun report(owner: String, busy: Boolean?) {
        if (!open || this.owner != owner) return
        val next = when (busy) {
            null -> null
            true -> Work.Working
            false -> if (work == Work.Working) Work.Done else work
        }
        if (next != work) work = next
    }
}

/** Work shorter than this never shows the button, so quick replies stay calm. */
private const val SHOW_AFTER_MS = 5_000L

/**
 * Play, in a message box (iOS WaitingGamesPlayButton, web WaitingGamesPlayButton). With [anytime] (Bot and
 * Code) it shows as soon as the session opens; otherwise beside Stop after 5 s of work. Keep it in the
 * composition either way: it tells the open game when the reply is ready. [hidden]: the person is typing,
 * so the button steps aside (it still reports).
 */
@Composable
fun WaitingGamesPlayButton(busy: Boolean, anytime: Boolean = false, hidden: Boolean = false) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    remember { WaitingGamesStore.attach(context) }
    val store = WaitingGamesStore
    val owner = remember { UUID.randomUUID().toString() }
    var shown by remember { mutableStateOf(false) }
    val reduceMotion = rememberReduceMotion()
    val playing = store.open && store.owner == owner

    LaunchedEffect(busy) {
        shown = false
        store.report(owner, busy)
        if (!busy) return@LaunchedEffect
        delay(SHOW_AFTER_MS)
        shown = true
    }
    DisposableEffect(owner) { onDispose { store.report(owner, null) } }

    AnimatedVisibility(
        visible = store.enabled && !playing && !hidden && (anytime || (busy && shown)),
        enter = if (reduceMotion) EnterTransition.None else scaleIn(tween(200), initialScale = 0.9f) + fadeIn(tween(200)),
        exit = if (reduceMotion) ExitTransition.None else scaleOut(tween(160), targetScale = 0.9f) + fadeOut(tween(160)),
    ) {
        Box(
            Modifier
                .size(44.dp)
                .clip(CircleShape)
                .testTag("waitingGamesPlay")
                .clickable(role = Role.Button, onClickLabel = "Play a game") {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    store.openFor(owner, busy)
                }
                .semantics { contentDescription = "Play a game" },
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier
                    .size(32.dp)
                    .clip(CircleShape)
                    .background(c.accentPrimary.copy(alpha = 0.10f))
                    .border(1.dp, c.accentPrimary.copy(alpha = 0.55f), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_game_controller_fill), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
            }
        }
    }
}
