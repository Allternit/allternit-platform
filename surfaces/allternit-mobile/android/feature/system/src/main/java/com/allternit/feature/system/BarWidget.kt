package com.allternit.feature.system

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.LocalContext
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.padding
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import com.allternit.feature.bar.BarDeepLink
import com.allternit.feature.bar.BarFlag
import com.allternit.feature.bar.BarJobs
import com.allternit.feature.bar.BarWording

/**
 * The "Do with Allternit" home-screen widget (Island bar + Team map, lane 37; mockups/B-android.jsx). One tap opens the
 * island bar through [BarDeepLink.intent]. While a bar job is live ([BarJobs.current]) it shows the same state line as
 * the iOS Live Activity and the bar itself ("Waiting on Noah", "Working", "Refunded EUR 320"). With `bar.island` off it
 * does what the + sheet does without the flag: it just opens the app.
 */
class BarWidget : GlanceAppWidget() {
    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val on = BarFlag.isOn(context)
        val job = if (on) BarJobs.current.value else null
        provideContent { BarWidgetBody(on, job?.title, job?.stateLine) }
    }
}

class BarWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = BarWidget()
}

private val Primary = Color(0xFF1A1A1A)
private val Secondary = Color(0xFF6B6B6B)
private val Card = Color(0xFFFFFFFF)

@Composable
private fun BarWidgetBody(on: Boolean, jobTitle: String?, stateLine: String?) {
    val context = LocalContext.current
    val intent = if (on) {
        BarDeepLink.intent(context)
    } else {
        context.packageManager.getLaunchIntentForPackage(context.packageName) ?: Intent(Intent.ACTION_MAIN).setPackage(context.packageName)
    }
    Column(
        modifier = GlanceModifier.fillMaxSize().background(ColorProvider(Card)).cornerRadius(22.dp).padding(12.dp)
            .clickable(actionStartActivity(intent)),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("A://TERNIT", style = TextStyle(color = ColorProvider(Secondary), fontSize = 9.sp, fontWeight = FontWeight.Bold))
        if (on && stateLine != null) {
            Text(jobTitle ?: BarWording.entry, style = TextStyle(color = ColorProvider(Primary), fontSize = 13.sp, fontWeight = FontWeight.Medium), maxLines = 1)
            Text(stateLine, style = TextStyle(color = ColorProvider(Secondary), fontSize = 11.sp), maxLines = 2)
        } else {
            Text(if (on) "Do with Allternit" else "Turn on Island bar in Settings", style = TextStyle(color = ColorProvider(Primary), fontSize = 15.sp, fontWeight = FontWeight.Medium), maxLines = 2)
            if (on) Text(BarWording.prompt, style = TextStyle(color = ColorProvider(Secondary), fontSize = 11.sp), maxLines = 1)
        }
    }
}
