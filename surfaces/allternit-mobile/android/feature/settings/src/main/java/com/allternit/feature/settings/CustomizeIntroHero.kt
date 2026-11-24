package com.allternit.feature.settings

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.MeFlagsClient
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.IntroKeys
import com.allternit.core.design.IntroSeenStore
import com.allternit.core.design.IntroSheet
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.rememberReducedMotion
import com.allternit.feature.artifacts.LibraryNet
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private class HeroItem(val name: String, val icon: Int)

private val HERO_TABS = listOf("Skills", "Connectors", "Plugins")
private val HERO_ITEMS = listOf(
    listOf(
        HeroItem("Algorithmic Art", DesignR.drawable.ph_book_open),
        HeroItem("Brand Guidelines", DesignR.drawable.ph_book_open),
        HeroItem("Codebase To Course", DesignR.drawable.ph_book_open),
    ),
    listOf(
        HeroItem("GitHub", DesignR.drawable.ph_code),
        HeroItem("Gmail", DesignR.drawable.ph_envelope),
        HeroItem("Slack", DesignR.drawable.ph_chat_circle),
    ),
    listOf(
        HeroItem("Allternit for Excel", DesignR.drawable.ph_table),
        HeroItem("Allternit for Word", DesignR.drawable.ph_file_text),
        HeroItem("Allternit for Chrome", DesignR.drawable.ph_globe),
    ),
)

/**
 * The moving copy of the real Customize page for the first-visit sheet: the Skills, Connectors and Plugins tabs
 * take turns (the underline slides), and the first card in each turns to added. Under reduced motion it holds
 * the Skills tab with its first card added.
 */
@Composable
internal fun CustomizeIntroHero(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    var tab by remember { mutableIntStateOf(0) }
    LaunchedEffect(reduced) {
        if (reduced) { tab = 0; return@LaunchedEffect }
        while (true) {
            delay(1900)
            tab = (tab + 1) % HERO_TABS.size
        }
    }
    Box(
        modifier.fillMaxWidth().height(200.dp).clip(RoundedCornerShape(24.dp)).background(if (PolishGlass.isDark()) Color(0xFF1E1E20) else Color(0xFFF7F7F7)),
    ) {
        Column(
            Modifier.padding(start = 12.dp, end = 12.dp, top = 12.dp).fillMaxWidth().height(230.dp)
                .clip(RoundedCornerShape(18.dp)).background(c.bgPrimary).border(1.dp, PolishGlass.border(), RoundedCornerShape(18.dp))
                .padding(horizontal = 12.dp, vertical = 11.dp),
        ) {
            Box(Modifier.padding(bottom = 9.dp)) { BlockText("CUSTOMIZE", height = 8.dp) }
            SlidingTabRow(HERO_TABS, tab, onSelect = null, fontSize = 11.sp, gap = 14.dp)
            Text("For you", color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 8.dp, bottom = 5.dp))
            AnimatedContent(
                targetState = tab,
                transitionSpec = {
                    if (reduced) fadeIn(tween(150)) togetherWith fadeOut(tween(100))
                    else (fadeIn(tween(220)) + slideInHorizontally(tween(220)) { it / 8 }) togetherWith (fadeOut(tween(150)) + slideOutHorizontally(tween(150)) { -it / 8 })
                },
                label = "customizeHeroTab",
            ) { t ->
                var added by remember(t) { mutableStateOf(reduced) }
                LaunchedEffect(t, reduced) { if (!reduced) { delay(700); added = true } }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                    HERO_ITEMS[t].forEachIndexed { j, item ->
                        Column(
                            Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(PolishGlass.track()).padding(4.dp),
                        ) {
                            Box(
                                Modifier.fillMaxWidth().height(42.dp).clip(RoundedCornerShape(9.dp)).background(if (PolishGlass.isDark()) Color(0xFF3A3A3C) else Color.White),
                            ) {
                                Icon(painterResource(item.icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.align(Alignment.Center).size(15.dp))
                                val isAdded = j == 0 && added
                                val dot by animateColorAsState(if (isAdded) c.textPrimary else Color.Transparent, tween(600), label = "heroAdd")
                                Box(
                                    Modifier.align(Alignment.TopEnd).padding(3.dp).size(15.dp).clip(CircleShape)
                                        .background(if (isAdded) dot else Color.Transparent).border(1.dp, PolishGlass.border(), CircleShape),
                                    contentAlignment = Alignment.Center,
                                ) {
                                    Icon(
                                        painterResource(if (isAdded) DesignR.drawable.ph_check_bold else DesignR.drawable.ph_plus_bold),
                                        contentDescription = null, tint = if (isAdded) c.bgPrimary else c.textPrimary, modifier = Modifier.size(8.dp),
                                    )
                                }
                            }
                            Text(item.name, color = c.textPrimary, fontSize = 9.5.sp, fontWeight = FontWeight.SemiBold, lineHeight = 11.sp, modifier = Modifier.padding(start = 2.dp, end = 2.dp, top = 4.dp, bottom = 1.dp))
                        }
                    }
                }
            }
        }
    }
}

/**
 * Shows the Customize first-visit sheet once per account (`introCustomizeSeenAt`). The caller shows it only
 * when signed in. An intro already seen on this device never waits on the network. The store is built the way
 * the Artifacts lane builds it: `IntroSeenStore(MeFlagsClient(...))` over the shared client in LibraryNet.
 */
@Composable
internal fun CustomizeIntroHost() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember {
        val client = MeFlagsClient(LibraryNet.okHttp, LibraryNet.config.cloudBaseUrl)
        IntroSeenStore(context, fetch = { client.read() }, push = { k, iso -> client.write(k, iso) })
    }
    var show by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        if (store.isSeen(IntroKeys.CUSTOMIZE)) return@LaunchedEffect
        store.refresh()
        show = !store.isSeen(IntroKeys.CUSTOMIZE)
    }
    if (show) {
        IntroSheet(
            title = "Teach the agent new tricks.",
            line = "Add skills, connect your tools and install plugins. Every chat can use them.",
            eyebrow = { BlockText("CUSTOMIZE", height = 8.dp, color = PolishGlass.Coral) },
            hero = { CustomizeIntroHero() },
            onContinue = { show = false; scope.launch { store.markSeen(IntroKeys.CUSTOMIZE) } },
        )
    }
}
