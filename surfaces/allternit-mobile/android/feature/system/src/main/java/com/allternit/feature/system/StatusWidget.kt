package com.allternit.feature.system

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.action.clickable
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider

/**
 * The home-screen status widget (iOS `StatusWidget`, families small and medium; the lock-screen accessory families have
 * no Android widget). Small is the status face: working and needs-you counts, the focus bot's step, the five empty
 * states. Medium adds the actions face beside it (iOS flips between the two faces; Android has no flip, so the wide
 * widget shows both): Ask a bot, Call main bot, Needs you, Run routine. Data is the stored status snapshot ([StatusSync]).
 */
class StatusWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Responsive(setOf(SMALL, MEDIUM))

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        // Like iOS, refetch when the stored snapshot is stale; the refresh redraws every widget when it lands.
        if (StatusSync.isStale(context)) StatusSync.refreshAsync(context)
        val content = StatusSync.content(context)
        provideContent { StatusWidgetBody(content) }
    }

    private companion object {
        val SMALL = DpSize(110.dp, 110.dp)
        val MEDIUM = DpSize(250.dp, 110.dp)
    }
}

class StatusWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = StatusWidget()
}

private val Primary = Color(0xFF1A1A1A)
private val Secondary = Color(0xFF6B6B6B)
private val Working = Color(0xFF2F7D4F)
private val Amber = Color(0xFFB7791F)
private val Fill = Color(0xFFF1F1F0)
private val Card = Color(0xFFFFFFFF)

@Composable
private fun StatusWidgetBody(content: StatusContent) {
    val context = LocalContext.current
    val wide = LocalSize.current.width >= 220.dp
    val snap = content.snapshotOrNull
    val open = actionStartActivity(SystemLinks.intent(context, SystemLinks.INBOX))
    Row(
        modifier = GlanceModifier.fillMaxSize().background(ColorProvider(Card)).cornerRadius(22.dp).padding(12.dp).clickable(open),
    ) {
        Column(modifier = if (wide) GlanceModifier.defaultWeight() else GlanceModifier) {
            val copy = content.stateCopy()
            if (copy != null) {
                Text("A://TERNIT", style = TextStyle(color = ColorProvider(Secondary), fontSize = 9.sp, fontWeight = FontWeight.Bold))
                Spacer(GlanceModifier.height(6.dp))
                Text(copy.first, style = TextStyle(color = ColorProvider(Primary), fontSize = 15.sp, fontWeight = FontWeight.Medium), maxLines = 2)
                Text(copy.second, style = TextStyle(color = ColorProvider(Secondary), fontSize = 11.sp), maxLines = 2)
            } else if (snap != null) {
                val needs = snap.needsCount
                Row {
                    Count(snap.activeBots, "working", Working)
                    Spacer(GlanceModifier.width(16.dp))
                    Count(needs, "needs you", if (needs > 0) Amber else Secondary)
                }
                Spacer(GlanceModifier.height(6.dp))
                val focus = snap.focus
                if (focus != null && focus.botName.isNotBlank()) {
                    Text(focus.botName, style = TextStyle(color = ColorProvider(Primary), fontSize = 12.sp, fontWeight = FontWeight.Medium), maxLines = 1)
                    // A private bot's step stays off the home screen (iOS keeps it out of the snapshot's lock-screen faces).
                    if (focus.step.isNotBlank() && !focus.isPrivate) {
                        Text(focus.step, style = TextStyle(color = ColorProvider(Secondary), fontSize = 11.sp), maxLines = if (wide) 2 else 1)
                    }
                }
                val routine = snap.lastRoutine
                if (wide && routine != null && routine.title.isNotBlank()) {
                    Text(routineLine(routine), style = TextStyle(color = ColorProvider(Secondary), fontSize = 10.sp), maxLines = 1)
                }
            }
        }
        if (wide) {
            Spacer(GlanceModifier.width(10.dp))
            Column(modifier = GlanceModifier.defaultWeight()) {
                Row(modifier = GlanceModifier.fillMaxWidth().defaultWeight()) {
                    ActionButton("Ask a bot", SystemLinks.ASK_MAIN, Primary)
                    Spacer(GlanceModifier.width(6.dp))
                    ActionButton("Call main bot", SystemLinks.CALL_MAIN, Primary)
                }
                Spacer(GlanceModifier.height(6.dp))
                Row(modifier = GlanceModifier.fillMaxWidth().defaultWeight()) {
                    val needs = snap?.needsCount ?: 0
                    ActionButton(if (needs > 0) "Needs you $needs" else "Needs you", SystemLinks.INBOX, if (needs > 0) Amber else Primary)
                    Spacer(GlanceModifier.width(6.dp))
                    ActionButton("Run routine", SystemLinks.ROUTINES, Primary)
                }
            }
        }
    }
}

@Composable
private fun Count(n: Int, label: String, color: Color) {
    Column {
        Text("$n", style = TextStyle(color = ColorProvider(color), fontSize = 28.sp, fontWeight = FontWeight.Medium))
        Text(label, style = TextStyle(color = ColorProvider(Secondary), fontSize = 10.sp))
    }
}

@Composable
private fun ActionButton(title: String, url: String, tint: Color) {
    val context = LocalContext.current
    Box(
        modifier = GlanceModifier.width(72.dp).height(44.dp).background(ColorProvider(Fill)).cornerRadius(11.dp)
            .clickable(actionStartActivity(SystemLinks.intent(context, url))),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, style = TextStyle(color = ColorProvider(tint), fontSize = 10.sp, fontWeight = FontWeight.Medium), maxLines = 2)
    }
}

/** iOS `Copy.routineLine`. */
private fun routineLine(r: StatusSnapshot.LastRoutine): String {
    val word = when (r.status.lowercase()) {
        "ok", "success", "succeeded", "done", "completed" -> "done"
        "failed", "error" -> "failed"
        "running" -> "running"
        else -> r.status
    }
    return if (r.title.isBlank()) word else "${r.title} $word"
}
