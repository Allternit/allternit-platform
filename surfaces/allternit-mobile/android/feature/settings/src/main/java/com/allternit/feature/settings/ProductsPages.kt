package com.allternit.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.Locale

/** Which part of iOS ProductsSettingsView a page shows. */
internal enum class ProductsPart(val title: String, val tag: String) {
    GizziCode("Gizzi Code", "settingsGizziCode"),
    Cowork("Files & devices", "settingsCowork"),
    Extensions("Extensions", "settingsExtensions"),
}

/**
 * iOS ProductsSettingsView: Gizzi Code, Cowork and Extensions preferences. They are this phone's settings (iOS keeps
 * them in UserDefaults with the same defaults). Dispatch shares its flag with Settings → Platform, as iOS shares
 * `dispatchEnabled`.
 */
@Composable
internal fun ProductsPage(part: ProductsPart, host: SettingsHost, back: () -> Unit) {
    val context = LocalContext.current
    fun flag(key: String, default: Boolean) = SettingsPrefs.flag(context, key, default)
    var browserTools by remember { mutableStateOf(flag("gizzi.browserTools", true)) }
    var autoPRs by remember { mutableStateOf(flag("gizzi.autoCreatePRs", false)) }
    var autofix by remember { mutableStateOf(flag("gizzi.autofixPRs", true)) }
    var attention by remember { mutableStateOf(flag("gizzi.drawAttention", true)) }
    var dispatch by remember { mutableStateOf(flag("platform.dispatch", false)) }
    var extUpdate by remember { mutableStateOf(flag("extensions.autoUpdate", true)) }
    var builtinNode by remember { mutableStateOf(flag("extensions.builtinNode", true)) }
    SPage(part.title, tag = part.tag, onBack = back) {
        when (part) {
            ProductsPart.GizziCode -> {
                SLabel("Gizzi Code")
                SCard {
                    SToggleRow("Browser tools", "gizzi_browserTools", browserTools, detail = "Allow Gizzi Code to read from browser contexts.", onChange = { browserTools = it; SettingsPrefs.setFlag(context, "gizzi.browserTools", it) })
                    SDivider()
                    SToggleRow("Auto-create pull requests", "gizzi_autoPRs", autoPRs, onChange = { autoPRs = it; SettingsPrefs.setFlag(context, "gizzi.autoCreatePRs", it) })
                    SDivider()
                    SToggleRow("Autofix failing PRs", "gizzi_autofix", autofix, detail = "Retry code changes when CI checks fail.", onChange = { autofix = it; SettingsPrefs.setFlag(context, "gizzi.autofixPRs", it) })
                    SDivider()
                    SToggleRow("Draw-attention notifications", "gizzi_attention", attention, onChange = { attention = it; SettingsPrefs.setFlag(context, "gizzi.drawAttention", it) })
                }
            }
            ProductsPart.Cowork -> {
                SLabel("Cowork")
                SCard {
                    SToggleRow("Enable dispatch", "cowork_dispatch", dispatch, detail = "Route cowork tasks through Allternit's dispatch layer.", onChange = { dispatch = it; SettingsPrefs.setFlag(context, "platform.dispatch", it) })
                    SDivider()
                    SRow("Open Cowork workspace", "cowork_open", onClick = { host.openCowork() })
                }
            }
            ProductsPart.Extensions -> {
                SLabel("Extensions")
                SCard {
                    SToggleRow("Auto-update extensions", "extensions_autoUpdate", extUpdate, detail = "Background update marketplace and sidecar extensions.", onChange = { extUpdate = it; SettingsPrefs.setFlag(context, "extensions.autoUpdate", it) })
                    SDivider()
                    SToggleRow("Use built-in Node runtime", "extensions_builtinNode", builtinNode, detail = "Run extension sidecars with the bundled Node runtime.", onChange = { builtinNode = it; SettingsPrefs.setFlag(context, "extensions.builtinNode", it) })
                }
                SNote("Installed marketplace extensions are managed automatically.")
            }
        }
    }
}

private val Warn = Color(0xFFB7791F)
private val Healthy = Color(0xFF2F8F5B)

private fun credits(v: Double): String = if (!v.isFinite()) "0.0" else if (v >= 10) String.format(Locale.US, "%.0f", v) else String.format(Locale.US, "%.1f", v)

private fun pressureColor(p: Double): Color = when {
    p >= 90 -> Danger
    p >= 75 -> Warn
    else -> Healthy
}

/**
 * iOS BudgetDashboardView (`GET /runtime/budget`, `POST /runtime/budget/quota`): status, the four stat cards, the
 * shared hourly quota with quick picks, budget posture and projected overrun, resource pressure bars and alerts.
 */
@Composable
internal fun BudgetDashboard() {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("runtimeBudget") { Net.get("runtime/budget").toLoad("load runtime budget") }
    var draft by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    LoadView(load, reload, "load runtime budget") { body ->
        val o = body as? JsonObject
        val configured = o.num("credits_remaining") ?: 0.0
        val consumed = o.num("credits_consumed_this_hour") ?: 0.0
        val projected = o.num("projected_hourly_cost") ?: 0.0
        val status = o.str("status") ?: ""
        val metrics = listOf("CPU" to (o.num("cpu_percent") ?: 0.0), "Memory" to (o.num("memory_percent") ?: 0.0), "Network" to (o.num("network_percent") ?: 0.0), "Workers" to (o.num("worker_percent") ?: 0.0))
        val maxPressure = metrics.maxOf { it.second }
        val pressureLabel = when { maxPressure >= 90 -> "Critical pressure"; maxPressure >= 75 -> "Warning pressure"; else -> "Comfortable headroom" }
        val overrun = if (configured > 0) maxOf(0.0, projected - configured) else 0.0
        val (badge, badgeColor) = when {
            status == "exhausted" || maxPressure >= 100 -> "EXHAUSTED" to Danger
            status == "warning" || maxPressure >= 75 -> "WARNING" to Warn
            else -> "HEALTHY" to Healthy
        }
        val quota = draft ?: credits(configured)
        Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Panel("budget_status") {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text("ECONOMIC MODEL", color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 2.4.sp)
                        Text("Runtime Budget", color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
                        Text("Manage the shared runtime quota and watch live resource pressure.", color = c.textSecondary, fontSize = 12.sp, maxLines = 2)
                    }
                    Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(Modifier.clip(RoundedCornerShape(50)).background(badgeColor.copy(alpha = 0.12f)).padding(horizontal = 10.dp, vertical = 4.dp)) {
                            Text(badge, color = badgeColor, fontSize = 10.sp, fontWeight = FontWeight.Bold, letterSpacing = 0.8.sp)
                        }
                        Text(pressureLabel, color = c.textSecondary, fontSize = 12.sp)
                    }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                StatCard("Configured / Hour", credits(configured), "credits", Warn, Modifier.weight(1f))
                StatCard("Consumed This Hour", credits(consumed), "credits", c.accentPrimary, Modifier.weight(1f))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                StatCard("Projected Hourly", credits(projected), "credits", c.accentPrimary, Modifier.weight(1f))
                StatCard("Max Pressure", String.format(Locale.US, "%.1f", maxPressure), "%", pressureColor(maxPressure), Modifier.weight(1f))
            }
            Panel("budget_quota") {
                Text("Shared runtime quota", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text("Update the hourly quota for the default runtime tenant.", color = c.textSecondary, fontSize = 12.sp)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Box(Modifier.weight(1f)) { SField("Credits per hour", quota, "runtime_quota", { draft = it.filter { ch -> ch.isDigit() || ch == '.' } }) }
                    val canApply = !saving && quota.isNotBlank()
                    Box(
                        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).background(if (canApply) Warn else c.borderStrong).testTag("runtime_setQuota")
                            .clickable(enabled = canApply) {
                                error = null
                                val parsed = quota.toDoubleOrNull()
                                if (parsed == null || !parsed.isFinite()) { error = "Enter a valid number"; return@clickable }
                                saving = true
                                scope.launch {
                                    val r = Net.send("POST", "runtime/budget/quota", buildJsonObject { put("credits_per_hour", parsed) })
                                    saving = false
                                    if (r is ApiResult.Ok) { draft = null; reload() } else error = r.friendly("set the quota")
                                }
                            }.padding(horizontal = 14.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        if (saving) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = Color.Black)
                        else Text("Apply", color = Color.Black, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (q in listOf(5, 10, 20, 40)) {
                        Box(
                            Modifier.clip(RoundedCornerShape(50)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(50))
                                .testTag("runtime_quick_$q").clickable { draft = credits(q.toDouble()) }.padding(horizontal = 10.dp, vertical = 6.dp),
                        ) { Text("$q cr", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold) }
                    }
                }
                SError(error)
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.bgSecondary).padding(12.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text("Budget posture", color = c.textSecondary, fontSize = 12.sp)
                        Text(pressureLabel, color = pressureColor(maxPressure), fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    }
                    Column(horizontalAlignment = Alignment.End) {
                        Text("Projected overrun", color = c.textSecondary, fontSize = 12.sp)
                        Text("${credits(overrun)} credits", color = if (overrun > 0) Danger else c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
            Panel("budget_pressure") {
                Text("Resource pressure", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                for ((label, raw) in metrics) {
                    val p = raw.coerceIn(0.0, 100.0)
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(Modifier.fillMaxWidth()) {
                            Text(label, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                            Text(String.format(Locale.US, "%.1f%% utilized", p), color = pressureColor(raw), fontSize = 12.sp)
                        }
                        Box(Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)).background(c.bgSecondary)) {
                            Box(Modifier.fillMaxWidth((p / 100).toFloat()).height(8.dp).clip(RoundedCornerShape(4.dp)).background(pressureColor(raw)))
                        }
                    }
                }
            }
            Panel("budget_alerts") {
                Text("Alerts", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                val (title, message, tone) = when {
                    status == "exhausted" || maxPressure >= 100 -> Triple("Runtime budget exhausted", "The runtime has crossed its enforced pressure ceiling.", Danger)
                    status == "warning" || maxPressure >= 75 -> Triple("Runtime budget under pressure", "Pressure is elevated; consider reducing load.", Warn)
                    else -> Triple("Runtime budget healthy", "Operating inside the current budget envelope.", c.accentPrimary)
                }
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(tone.copy(alpha = 0.08f)).padding(12.dp)) {
                    Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    Text(message, color = c.textSecondary, fontSize = 12.sp, maxLines = 2)
                }
            }
        }
    }
}

@Composable
private fun Panel(tag: String, content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)).padding(14.dp).testTag(tag),
        verticalArrangement = Arrangement.spacedBy(12.dp),
        content = content,
    )
}

@Composable
private fun StatCard(label: String, value: String, unit: String, color: Color, modifier: Modifier) {
    val c = AllternitTheme.colors
    Column(
        modifier.clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.size(8.dp).clip(RoundedCornerShape(4.dp)).background(color))
        Text(label.uppercase(), color = c.textSecondary, fontSize = 9.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 1.sp)
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(value, color = c.textPrimary, fontSize = 24.sp, fontWeight = FontWeight.ExtraBold)
            Text(unit, color = c.textSecondary, fontSize = 12.sp)
        }
    }
}
