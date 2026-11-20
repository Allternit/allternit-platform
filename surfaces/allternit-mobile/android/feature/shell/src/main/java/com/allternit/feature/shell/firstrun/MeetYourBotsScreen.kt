package com.allternit.feature.shell.firstrun

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.feature.shell.ModelStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * The flow's drafts (iOS MeetYourBotsModel): nothing is created until "Start the job". Team, main
 * bot, routines and the first job are picks; then [MeetFinish.run] makes the real calls and the
 * done step reports each result.
 */
internal class MeetYourBotsModel(context: Context, private val scope: CoroutineScope) {
    enum class Step { Start, Team, Main, Routines, Notifications, Job, Done }
    sealed interface RosterState { data object Loading : RosterState; data object Loaded : RosterState; data class Failed(val why: String) : RosterState }

    private val appContext = context.applicationContext
    val catalog = MeetBotCatalog.get(context)
    private val models = SelectedModel(
        appContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE).getString(ModelStore.KEY_SELECTED, null),
    )

    var roster by mutableStateOf<List<MeetRosterBot>>(emptyList())
        private set
    var rosterState by mutableStateOf<RosterState>(RosterState.Loading)
        private set
    var entry by mutableStateOf(MeetEntry.Setup)
        private set
    var stepIndex by mutableStateOf(0)
        private set
    var members by mutableStateOf<List<String>>(emptyList())
        private set
    var category by mutableStateOf("all")
    var mainChoice by mutableStateOf<MeetMainChoice>(MeetMainChoice.Primary)
    var lookName by mutableStateOf("")
    var primaryName by mutableStateOf("")
    var routines by mutableStateOf<List<MeetRoutineDraft>>(emptyList())
        private set
    var jobKey by mutableStateOf<String?>(null)
        private set
    var jobText by mutableStateOf("")
    var items by mutableStateOf<List<MeetFinishItem>>(emptyList())
        private set
    var running by mutableStateOf(false)
        private set
    var started by mutableStateOf(false)
        private set
    var adding by mutableStateOf<Set<String>>(emptySet())
        private set

    private val removedDrafts = mutableSetOf<String>()
    private var seeded = false
    private val state = MeetFinishState()
    private var teamAtStart = emptyList<String>()

    val steps: List<Step>
        get() = if (entry == MeetEntry.Setup) Step.entries else Step.entries - Step.Main
    val step: Step get() = steps[minOf(stepIndex, steps.size - 1)]

    suspend fun loadRoster() {
        rosterState = RosterState.Loading
        try {
            val bots = MeetAgentsApi.roster()
            roster = bots
            rosterState = RosterState.Loaded
            if (primaryName.isEmpty()) bots.firstOrNull { it.templateId == catalog.primary.id }?.let { primaryName = it.displayName }
            // Bot Mode's first visit skips "main" (a main bot already exists), decided on the start step only.
            if (stepIndex == 0) entry = if (MeetMainBot.find(bots) == null) MeetEntry.Setup else MeetEntry.BotMode
            if (!seeded) {
                seeded = true
                // Template bots the person already has start on the team (never made twice).
                val have = existingTemplateIds
                val ids = catalog.templates.map { it.id }.filter { it in have }
                if (members.isEmpty() && ids.isNotEmpty()) members = ids.take(12)
                syncDrafts()
            }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            rosterState = RosterState.Failed((e as? SetupError)?.message ?: "Something went wrong")
        }
    }

    val existingTemplateIds: Set<String> get() = roster.mapNotNull { it.templateId }.toSet()
    val mainBot: MeetRosterBot? get() = MeetMainBot.find(roster)

    /** The name the flow calls the main bot. */
    val leader: String
        get() {
            if (entry == MeetEntry.BotMode) return mainBot?.displayName ?: "your main bot"
            return when (val m = mainChoice) {
                MeetMainChoice.Primary -> primaryName.trim().ifEmpty { catalog.primary.name }
                is MeetMainChoice.Look -> lookName.trim().ifEmpty { catalog.primary.name }
                is MeetMainChoice.Bot -> roster.firstOrNull { it.id == m.id }?.displayName ?: "your main bot"
            }
        }

    fun next() {
        if (stepIndex >= steps.size - 1) return
        (mainChoice as? MeetMainChoice.Look)?.let { if (step == Step.Main) mainChoice = it.copy(name = lookName.trim()) }
        stepIndex += 1
    }

    fun back() { if (stepIndex > 0 && step != Step.Done) stepIndex -= 1 }

    val shownTemplates get() = catalog.templates.filter { category == "all" || it.category == category }

    fun toggle(id: String) {
        members = if (id in members) members - id else members + id
        syncDrafts()
        val key = jobKey
        if (key != null && key != MEET_MAIN_KEY && key !in members) { jobKey = null; jobText = "" }
    }

    /** team-slots syncDrafts: drop a removed bot's routines, add a new bot's template routines. */
    private fun syncDrafts() {
        val keep = routines.filter { it.botKey == MEET_MAIN_KEY || it.botKey in members }
        val have = keep.map { it.id }.toSet()
        val skip = existingTemplateIds
        val add = members.filter { it !in skip }.flatMap { id ->
            catalog.template(id)?.let(MeetRoutineDraft::drafts).orEmpty().filter { it.id !in have && it.id !in removedDrafts }
        }
        routines = keep + add
    }

    fun toggleRoutine(id: String) { routines = routines.map { if (it.id == id) it.copy(enabled = !it.enabled) else it } }

    /** The routine time editor's save (iOS saveRoutine): the new cron and its label. */
    fun setSchedule(id: String, cron: String, label: String) { routines = routines.map { if (it.id == id) it.copy(cron = cron, scheduleLabel = label) else it } }

    fun removeRoutine(id: String) { routines = routines.filter { it.id != id }; removedDrafts += id }

    fun routineBotName(key: String) = if (key == MEET_MAIN_KEY) leader else catalog.template(key)?.name ?: "Bot"

    val routinesOn get() = routines.count { it.enabled }

    data class JobOption(val key: String, val name: String, val prompt: String, val expect: String, val starters: List<String>)

    val jobs: List<JobOption>
        get() {
            val p = catalog.primary
            return listOf(JobOption(MEET_MAIN_KEY, leader, p.firstTask.prompt, p.firstTask.expect, p.starterPrompts.take(3))) +
                members.mapNotNull { id -> catalog.template(id)?.let { JobOption(id, it.name, it.firstTask.prompt, it.firstTask.expect, it.starterPrompts.take(3)) } }
        }

    fun selectJob(job: JobOption) {
        if (jobKey == job.key) return
        jobKey = job.key
        jobText = job.prompt
    }

    val canStart get() = jobKey != null && jobText.isNotBlank() && !started

    private fun input(): MeetFinishInput {
        val text = jobText.trim()
        return MeetFinishInput(entry, members, mainChoice, primaryName.trim().ifEmpty { null }, routines,
            jobKey?.let { k -> if (text.isEmpty()) null else k to text })
    }

    fun startJob() {
        if (!canStart) return
        val input = input()
        started = true
        teamAtStart = members
        items = MeetFinish.plannedItems(input, catalog)
        stepIndex = steps.indexOf(Step.Done).coerceAtLeast(0)
        scope.launch { BotsIntro.markSeen(appContext) }
        run(input, null)
    }

    fun retry() = run(input(), items)

    val failed get() = !running && items.any { it.status == MeetItemStatus.Failed }

    private fun run(input: MeetFinishInput, prior: List<MeetFinishItem>?, after: () -> Unit = {}) {
        if (running) return
        running = true
        scope.launch {
            try {
                merge(MeetFinish.run(input, catalog, models, state, prior) { merge(it) })
            } finally {
                running = false
                after()
            }
        }
    }

    /** A later run's items replace the same keys and add the new ones. */
    private fun merge(out: List<MeetFinishItem>) {
        val keys = out.map { it.key }.toSet()
        fun order(k: String) = if (k == "main") 0 else if (k.startsWith("bot:")) 1 else if (k == "routines") 2 else 3
        items = (items.filter { it.key !in keys } + out).sortedBy { order(it.key) }
    }

    /** "Works well with your team": the bots the team hands work to. */
    val suggestions get() = catalog.handoffSuggestions(teamAtStart.ifEmpty { members })

    fun isMade(id: String) = state.botIds[id] != null || id in existingTemplateIds

    /** Adds a suggested bot after the team was made; its routines come in paused (nobody reviewed them). */
    fun addLater(id: String) {
        if (running || id in adding) return
        val t = catalog.template(id) ?: return
        adding = adding + id
        val drafts = MeetRoutineDraft.drafts(t).map { it.copy(enabled = false) }
        val prior = buildList {
            add(MeetFinishItem("main", items.firstOrNull { it.key == "main" }?.label ?: "Main bot", MeetItemStatus.Ok))
            add(MeetFinishItem("bot:$id", t.name, MeetItemStatus.Pending))
            if (drafts.isNotEmpty()) add(MeetFinishItem("routines", "Routines", MeetItemStatus.Pending))
        }
        run(MeetFinishInput(entry, listOf(id), mainChoice, null, drafts, null), prior) {
            if (state.botIds[id] != null) {
                teamAtStart = teamAtStart + id
                if (id !in members) members = members + id
            }
            adding = adding - id
        }
    }

    val openBotId: String? get() = state.startedThreadBotId(input()) ?: state.mainBotId

    val mainFailed get() = items.firstOrNull { it.key == "main" }?.status == MeetItemStatus.Failed
}

/**
 * "Meet your bots" on the phone (iOS MeetYourBotsView, web MeetYourBotsCard `is-phone`): the
 * step's visual on top, step dots with Back / Skip, the step's panel, and a pinned footer.
 * Steps: start, team, who leads, routines, notifications, first job, done.
 *
 * [onFinish] gets the bot whose chat should open (thread #1's bot, or the main bot), or null.
 */
@Composable
internal fun MeetYourBotsScreen(onFinish: (String?) -> Unit, doneLabel: String? = null) {
    val c = setupColors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val model = remember { MeetYourBotsModel(context, scope) }
    var notificationsOn by remember { mutableStateOf(notificationsEnabled(context)) }
    var asking by remember { mutableStateOf(false) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        notificationsOn = granted
        // Same consent moment registers this phone for push (iOS requestPermissionAndRegister).
        if (granted) com.allternit.feature.push.AllternitPush.registerIfAuthorized(context)
        asking = false
        model.next()
    }
    LaunchedEffect(Unit) { model.loadRoster() }
    val scroll = rememberScrollState()
    LaunchedEffect(model.stepIndex) { scroll.scrollTo(0) }

    BoxWithConstraints(Modifier.fillMaxSize().background(c.bg).windowInsetsPadding(WindowInsets.safeDrawing).testTag("meet-your-bots")) {
        val cap = when (model.step) {
            MeetYourBotsModel.Step.Start -> 340f
            MeetYourBotsModel.Step.Routines -> 380f
            MeetYourBotsModel.Step.Main -> 260f
            else -> 250f
        }
        val visualH = minOf(maxHeight.value * 0.45f, cap)
        Column(Modifier.fillMaxSize()) {
            AnimatedContent(model.step, transitionSpec = { fadeIn(tween(300)) togetherWith fadeOut(tween(200)) },
                label = "meet-visual", modifier = Modifier.fillMaxWidth().height(visualH.dp)) { step ->
                MeetVisual(model, step, notificationsOn)
            }
            // Header: dots, Back, Skip
            Row(Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(start = 20.dp, end = 20.dp, top = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Row(
                    Modifier.semantics(mergeDescendants = true) { contentDescription = "Step ${model.stepIndex + 1} of ${model.steps.size}" },
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    model.steps.indices.forEach { n ->
                        Box(Modifier.width(if (n == model.stepIndex) 18.dp else 6.dp).height(6.dp)
                            .background(if (n <= model.stepIndex) c.coral else c.line, CircleShape))
                    }
                }
                Spacer(Modifier.weight(1f))
                if (model.stepIndex > 0 && model.step != MeetYourBotsModel.Step.Done) LinkButton("Back") { model.back() }
                if (model.step != MeetYourBotsModel.Step.Start && model.step != MeetYourBotsModel.Step.Done) LinkButton("Skip") { onFinish(null) }
            }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(scroll).padding(horizontal = 20.dp, vertical = 12.dp)) {
                when (model.step) {
                    MeetYourBotsModel.Step.Start -> StartStep(model)
                    MeetYourBotsModel.Step.Team -> TeamStep(model)
                    MeetYourBotsModel.Step.Main -> MainStep(model)
                    MeetYourBotsModel.Step.Routines -> RoutinesStep(model)
                    MeetYourBotsModel.Step.Notifications -> {
                        H2("Know when a bot needs you")
                        Sub(if (notificationsOn) "Notifications are on. You’ll hear when a bot asks for your OK, finishes a job, or messages you. Pick which ones in Settings."
                        else "Bots work while you’re away. We’ll tell you when one asks for your OK, finishes a job, or messages you, nothing else. You choose which ones in Settings.")
                    }
                    MeetYourBotsModel.Step.Job -> JobStep(model)
                    MeetYourBotsModel.Step.Done -> DoneStep(model)
                }
            }
            // Footer
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.fill))
            Column(Modifier.padding(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                val setup = model.entry == MeetEntry.Setup
                when (model.step) {
                    MeetYourBotsModel.Step.Start -> {
                        MeetPrimary(if (setup) "Build my team" else "Let’s go", tag = "meet-build-team") { model.next() }
                        MeetGhost(if (setup) "Skip for now" else "I’ll look around", tag = "meet-skip-for-now") { onFinish(null) }
                    }
                    MeetYourBotsModel.Step.Team -> {
                        val n = model.members.size
                        MeetPrimary(if (n == 0) "Add at least one bot" else "Continue with $n bot${if (n > 1) "s" else ""}", disabled = n == 0) { model.next() }
                    }
                    MeetYourBotsModel.Step.Main -> MeetPrimary("Make ${model.leader} my main bot") { model.next() }
                    MeetYourBotsModel.Step.Routines -> MeetPrimary("Looks good") { model.next() }
                    MeetYourBotsModel.Step.Notifications -> {
                        MeetPrimary(if (notificationsOn) "Continue" else "Turn on notifications", disabled = asking) {
                            when {
                                notificationsOn -> model.next()
                                Build.VERSION.SDK_INT >= 33 -> { asking = true; permission.launch(Manifest.permission.POST_NOTIFICATIONS) }
                                else -> {
                                    // Before Android 13 there is no prompt: the app's notification settings.
                                    runCatching {
                                        context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                                            .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                                    }
                                    model.next()
                                }
                            }
                        }
                        if (!notificationsOn) MeetGhost("Not now") { model.next() }
                    }
                    MeetYourBotsModel.Step.Job -> MeetPrimary("Start the job", disabled = !model.canStart, tag = "meet-start-job") { model.startJob() }
                    MeetYourBotsModel.Step.Done -> {
                        MeetPrimary(if (model.running) "Setting up…" else doneLabel ?: "Open Bot Mode", disabled = model.running, tag = "meet-done") {
                            onFinish(model.openBotId)
                        }
                        if (model.failed) MeetGhost("Retry what didn’t finish") { model.retry() }
                    }
                }
            }
        }
    }
}

private fun notificationsEnabled(context: Context): Boolean {
    val enabled = (context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)?.areNotificationsEnabled() == true
    val granted = Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
    return enabled && granted
}

@Composable
private fun MeetVisual(model: MeetYourBotsModel, step: MeetYourBotsModel.Step, notificationsOn: Boolean) {
    val c = setupColors
    val hasLeader = model.entry == MeetEntry.BotMode || model.steps.indexOf(MeetYourBotsModel.Step.Main).let { it < 0 || model.stepIndex > it }
    when (step) {
        MeetYourBotsModel.Step.Start -> MeetNetworkView()
        MeetYourBotsModel.Step.Team -> {
            val n = model.members.size
            MeetTeamGrid(model.members, model.catalog, if (hasLeader) ({ LeaderTile(model, 68.dp, live = true) }) else null, model.leader,
                if (n > 0) "$n bot${if (n > 1) "s" else ""} · tap to take one off" else "Tap a card to add a bot", model.existingTemplateIds, model::toggle)
        }
        MeetYourBotsModel.Step.Main -> MeetMainStage(leaderField(model), "${model.leader} idle") { LeaderTile(model, 150.dp, live = true) }
        MeetYourBotsModel.Step.Routines -> MeetRoutinesPicture(model)
        MeetYourBotsModel.Step.Notifications -> Box(Modifier.fillMaxSize().background(meetSoft), contentAlignment = Alignment.Center) {
            SetupBlockField()
            Box(Modifier.size(96.dp).background(c.bg, RoundedCornerShape(24.dp)).border(1.dp, c.line, RoundedCornerShape(24.dp)), contentAlignment = Alignment.Center) {
                Text(if (notificationsOn) "🔔" else "🔕", fontSize = 40.sp)
            }
        }
        MeetYourBotsModel.Step.Job -> {
            val job = model.jobs.firstOrNull { it.key == model.jobKey }
            val handoff = job != null && job.key != MEET_MAIN_KEY
            val leader = model.leader.replaceFirstChar { it.uppercase() }
            MeetFirstJobVisual(
                main = { LeaderTile(model, 56.dp, live = true) },
                other = if (handoff) ({ KeyTile(model, job!!.key, 56.dp) }) else null,
                caption = if (job == null) "Pick a first job" else if (handoff) "$leader hands it to ${job.name}" else "$leader does this one",
                expect = job?.expect,
            )
        }
        MeetYourBotsModel.Step.Done -> {
            val n = model.members.size
            MeetTeamGrid(model.members, model.catalog, { LeaderTile(model, 68.dp, live = true) }, model.leader,
                "${model.leader.replaceFirstChar { it.uppercase() }} leads $n bot${if (n == 1) "" else "s"}",
                model.existingTemplateIds + model.members.filter(model::isMade), model::toggle)
        }
    }
}

private fun leaderField(model: MeetYourBotsModel): Color = when (val m = model.mainChoice) {
    MeetMainChoice.Primary -> Color(0xFF141413)
    is MeetMainChoice.Look -> hexColor(model.catalog.looks.firstOrNull { it.id == m.id }?.field)
    is MeetMainChoice.Bot -> Color(0xFFFAFAFB)
}

private fun botColor(model: MeetYourBotsModel, key: String): Color =
    if (key == MEET_MAIN_KEY) Color(0xFFD97757) else hexColor(model.catalog.template(key)?.color, Color(0xFFD97757))

@Composable
private fun LeaderTile(model: MeetYourBotsModel, size: androidx.compose.ui.unit.Dp, live: Boolean = false) {
    if (model.entry == MeetEntry.BotMode) {
        model.mainBot?.let { MeetBotTile(it, size) } ?: MeetLookTile("a", model.catalog, size, live)
        return
    }
    when (val m = model.mainChoice) {
        MeetMainChoice.Primary -> MeetLookTile("a", model.catalog, size, live)
        is MeetMainChoice.Look -> MeetLookTile(m.id, model.catalog, size, live)
        is MeetMainChoice.Bot -> model.roster.firstOrNull { it.id == m.id }?.let { MeetBotTile(it, size) } ?: MeetLookTile("a", model.catalog, size, live)
    }
}

@Composable
private fun KeyTile(model: MeetYourBotsModel, key: String, size: androidx.compose.ui.unit.Dp) {
    if (key == MEET_MAIN_KEY) LeaderTile(model, size)
    else model.catalog.template(key)?.let { MeetTemplateTile(it.color, it.look, it.name, size) }
}

// Pieces

@Composable
private fun H2(text: String) {
    Text(text, fontSize = 24.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.36).sp, color = setupColors.ink,
        modifier = Modifier.semantics { heading() })
}

@Composable
private fun Sub(text: String) {
    Text(text, fontSize = 14.sp, lineHeight = 19.sp, color = setupColors.sub, modifier = Modifier.padding(top = 4.dp, bottom = 12.dp))
}

@Composable
private fun MonoLabel(text: String) {
    Text(text.uppercase(), style = mono(11.5.sp, FontWeight.SemiBold), letterSpacing = 0.7.sp, color = setupColors.mut, modifier = Modifier.padding(bottom = 8.dp))
}

@Composable
private fun LinkButton(text: String, onClick: () -> Unit) {
    Text(text, fontSize = 13.sp, color = setupColors.sub, modifier = Modifier.clickable(onClick = onClick).padding(horizontal = 8.dp, vertical = 6.dp))
}

@Composable
private fun MeetPrimary(text: String, disabled: Boolean = false, tag: String? = null, onClick: () -> Unit) {
    val c = setupColors
    Box(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).background(c.ink.copy(alpha = if (disabled) 0.35f else 1f), RoundedCornerShape(12.dp))
            .clickable(enabled = !disabled, onClick = onClick).then(if (tag != null) Modifier.testTag(tag) else Modifier),
        contentAlignment = Alignment.Center,
    ) { Text(text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.bg) }
}

@Composable
private fun MeetGhost(text: String, tag: String? = null, onClick: () -> Unit) {
    Box(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(onClick = onClick).then(if (tag != null) Modifier.testTag(tag) else Modifier),
        contentAlignment = Alignment.Center,
    ) { Text(text, fontSize = 15.sp, fontWeight = FontWeight.Medium, color = setupColors.sub) }
}

@Composable
private fun OfflineCard(model: MeetYourBotsModel) {
    val c = setupColors
    val scope = rememberCoroutineScope()
    val failed = model.rosterState as? MeetYourBotsModel.RosterState.Failed ?: return
    Column(
        Modifier.fillMaxWidth().padding(bottom = 12.dp).background(c.fill, RoundedCornerShape(12.dp)).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("Your bots live on your computer", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
        Text("We couldn’t reach it right now (${failed.why}). You can still pick your team; it’s made when your computer is online.", fontSize = 13.sp, color = c.sub)
        SetupQuietButton("Try again") { scope.launch { model.loadRoster() } }
    }
}

@Composable
private fun StartStep(model: MeetYourBotsModel) {
    val c = setupColors
    val setup = model.entry == MeetEntry.Setup
    Text(if (setup) "BOTS" else "BOT MODE · ${model.leader.uppercase()}", style = mono(11.5.sp, FontWeight.SemiBold), letterSpacing = 1.4.sp,
        color = c.coral, modifier = Modifier.padding(bottom = 10.dp))
    Text(if (setup) "Your bots hand work to each other." else "Welcome to ${model.leader}’s team.", fontSize = 30.sp, lineHeight = 34.sp,
        fontWeight = FontWeight.SemiBold, color = c.ink, modifier = Modifier.semantics { heading() })
    Text(
        if (setup) "Pick ready-made bots, choose who leads them, and see when they’ll work. You approve what goes out."
        else "${model.leader.replaceFirstChar { it.uppercase() }} leads your bots. Add a few, see their week, and give ${model.leader} a first job.",
        fontSize = 16.sp, lineHeight = 22.sp, color = c.sub, modifier = Modifier.padding(top = 10.dp, bottom = 14.dp),
    )
    if (model.rosterState == MeetYourBotsModel.RosterState.Loading) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
            Text("Loading your bots…", fontSize = 13.sp, color = c.sub)
        }
    }
    OfflineCard(model)
}

@Composable
private fun TeamStep(model: MeetYourBotsModel) {
    val c = setupColors
    H2(if (model.entry == MeetEntry.Setup) "Who’s on your team?" else "Add bots to ${model.leader}’s team")
    Sub("Tap a bot to add it to your team. Each one is a ready-made template you can change later.")
    OfflineCard(model)
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(bottom = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Chip("All ${model.catalog.templates.size}", model.category == "all") { model.category = "all" }
        model.catalog.categories.forEach { cat -> Chip(MeetBotCatalog.categoryLabel(cat), model.category == cat) { model.category = cat } }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        model.shownTemplates.forEach { t ->
            TemplateCard(t, on = t.id in model.members, have = t.id in model.existingTemplateIds) { model.toggle(t.id) }
        }
    }
}

@Composable
private fun Chip(label: String, on: Boolean, onClick: () -> Unit) {
    val c = setupColors
    Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = if (on) c.bg else c.ink,
        modifier = Modifier.background(if (on) c.ink else c.fill, CircleShape).clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 7.dp))
}

@Composable
private fun TemplateCard(t: MeetBotTemplate, on: Boolean, have: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val c = setupColors
    Row(
        Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(14.dp))
            .border(if (on) 1.5.dp else 1.dp, if (on) c.ink else c.line, RoundedCornerShape(14.dp))
            .clickable(enabled = enabled, onClick = onClick).padding(12.dp).testTag("meet-template-${t.id}"),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        MeetTemplateTile(t.color, t.look, t.name, 44.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(t.name, fontSize = 14.5.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
            Text(if (have) "Already on your account" else t.tagline, fontSize = 12.5.sp, color = c.sub, maxLines = 2)
        }
        Text(if (on) "✓" else "+", fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = if (on) c.ink else c.mut)
    }
}

@Composable
private fun MainStep(model: MeetYourBotsModel) {
    val c = setupColors
    var discOpen by remember { mutableStateOf(false) }
    var leadSheet by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val botsStore = remember { com.allternit.feature.bots.BotModeStore(context) }
    H2("Who leads your team?")
    Sub("Your main bot talks to you and hands each job to the bot that owns it.")
    val bubble = when (val m = model.mainChoice) {
        is MeetMainChoice.Bot -> "${model.roster.firstOrNull { it.id == m.id }?.displayName ?: "This bot"} keeps its own memory and leads your team."
        is MeetMainChoice.Look -> model.catalog.primary.welcome.orEmpty().replace("{name}", model.lookName.trim().ifEmpty { "A://" })
        MeetMainChoice.Primary -> model.catalog.primary.welcome.orEmpty().replace("{name}", model.leader)
    }
    Row(Modifier.padding(bottom = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        LeaderTile(model, 28.dp)
        Text(bubble, fontSize = 13.5.sp, color = c.ink, modifier = Modifier.weight(1f).background(c.fill, RoundedCornerShape(14.dp)).padding(horizontal = 12.dp, vertical = 9.dp))
    }
    MonoLabel("Allternit")
    PickGrid {
        Pick("A://", model.mainChoice == MeetMainChoice.Primary, { model.mainChoice = MeetMainChoice.Primary }) {
            MeetLookTile("a", model.catalog, 52.dp, live = model.mainChoice == MeetMainChoice.Primary)
        }
        model.catalog.looks.forEach { look ->
            val selected = (model.mainChoice as? MeetMainChoice.Look)?.id == look.id
            Pick(look.label, selected, {
                if (model.lookName.isEmpty() || model.catalog.looks.any { it.label == model.lookName }) model.lookName = look.label
                model.mainChoice = MeetMainChoice.Look(look.id, model.lookName)
            }) { MeetLookTile(look.id, model.catalog, 52.dp, live = selected) }
        }
    }
    if (model.roster.isNotEmpty()) {
        Spacer(Modifier.height(12.dp))
        MonoLabel("Bots you already use")
        PickGrid {
            model.roster.forEach { bot ->
                Pick(bot.displayName, (model.mainChoice as? MeetMainChoice.Bot)?.id == bot.id, { model.mainChoice = MeetMainChoice.Bot(bot.id) }) {
                    MeetBotTile(bot, 52.dp)
                }
            }
        }
    }
    // iOS "Who leads": vendor / new / import (useBotSetup modes), from Bots mode's sheets.
    Spacer(Modifier.height(12.dp))
    MonoLabel("Bring your own")
    PickGrid {
        Pick("Vendor bot", false, { leadSheet = "vendor" }) { ActionTile("⇄") }
        Pick("New bot", false, { leadSheet = "new" }) { ActionTile("+") }
        Pick("Import", false, { leadSheet = "import" }) { ActionTile("↓") }
    }
    fun adopt(ids: List<String>) {
        leadSheet = null
        scope.launch {
            model.loadRoster()
            ids.firstOrNull()?.let { model.mainChoice = MeetMainChoice.Bot(it) }
        }
    }
    when (leadSheet) {
        "vendor" -> com.allternit.feature.bots.VendorConnectSheet(offerMain = true, onDismiss = { leadSheet = null }) { first, main -> adopt(listOfNotNull(main ?: first)) }
        "new" -> com.allternit.feature.bots.NewBotSheet(store = botsStore, onBrowseTemplates = null, onDismiss = { leadSheet = null }) { ids -> adopt(ids) }
        "import" -> com.allternit.feature.bots.ImportBotSheet(onDismiss = { leadSheet = null }) { ids -> adopt(ids) }
    }
    val isPrimary = model.mainChoice == MeetMainChoice.Primary
    if (isPrimary || model.mainChoice is MeetMainChoice.Look) {
        Column(Modifier.padding(top = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Name", fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
            val value = if (isPrimary) model.primaryName else model.lookName
            BasicTextField(
                value = value,
                onValueChange = { v -> val cut = v.take(24); if (isPrimary) model.primaryName = cut else model.lookName = cut },
                singleLine = true,
                textStyle = TextStyle(fontSize = 15.sp, color = c.ink),
                cursorBrush = SolidColor(c.ink),
                modifier = Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(10.dp)).border(1.dp, c.line, RoundedCornerShape(10.dp))
                    .padding(12.dp).testTag("meet-main-name"),
                decorationBox = { inner ->
                    Box { if (value.isEmpty()) Text(if (isPrimary) model.catalog.primary.name else "Name", fontSize = 15.sp, color = c.mut); inner() }
                },
            )
            Text("Shown in the Bots rail and in every thread.", fontSize = 12.sp, color = c.mut)
        }
    }
    Column(Modifier.padding(top = 14.dp)) {
        Text((if (discOpen) "▾ " else "▸ ") + "What your main bot can and can’t do", fontSize = 13.5.sp, fontWeight = FontWeight.Medium, color = c.sub,
            modifier = Modifier.clickable { discOpen = !discOpen }.padding(vertical = 6.dp))
        if (discOpen) {
            listOf(
                "Answers you directly, or hands a job to the bot that owns it and tells you who has it.",
                "Sees the status of your bots’ threads (titles and progress), not their conversations. Incognito threads stay private.",
                "Never sends, posts, pays, signs or deploys anything without your approval.",
                "You can change your main bot any time from a bot’s ⋯ menu.",
            ).forEach { line ->
                Row(Modifier.padding(vertical = 5.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("•", fontSize = 13.sp, color = c.mut)
                    Text(line, fontSize = 13.sp, color = c.sub)
                }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PickGrid(content: @Composable () -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { content() }
}

@Composable
private fun Pick(label: String, selected: Boolean, onClick: () -> Unit, tile: @Composable () -> Unit) {
    val c = setupColors
    Column(
        Modifier.width(70.dp).clickable(onClick = onClick).semantics(mergeDescendants = true) { contentDescription = label + if (selected) ", selected" else "" },
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Box(Modifier.border(if (selected) 2.dp else 0.dp, if (selected) c.ink else Color.Transparent, RoundedCornerShape(16.dp)).padding(3.dp)) { tile() }
        com.allternit.core.design.BotNameText(label, fontSize = 11.5.sp, color = if (selected) c.ink else c.sub, maxLines = 1)
    }
}

@Composable
private fun RoutinesStep(model: MeetYourBotsModel) {
    val c = setupColors
    var editing by remember { mutableStateOf<MeetRoutineDraft?>(null) }
    editing?.let { r ->
        // iOS RoutineScheduleEditor from the routine row.
        com.allternit.feature.bots.RoutineScheduleEditor(
            title = r.title,
            initial = com.allternit.feature.bots.RoutineRule.fromCron(r.cron)
                ?: com.allternit.feature.bots.RoutineRule(com.allternit.feature.bots.RoutineRule.Repeat.Weekdays, 9 * 60, days = com.allternit.feature.bots.RoutineRule.WEEKDAYS),
            saving = false, error = null,
            onSave = { rule -> model.setSchedule(r.id, rule.cron, rule.label); editing = null },
            onCancel = { editing = null },
        )
    }
    H2("When your bots work")
    Sub("Your bots come with these routines. Tap one to change when it runs, turn one off, or take it out.")
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        model.routines.forEach { r ->
            Row(
                Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).border(1.dp, c.line, RoundedCornerShape(12.dp)).padding(10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                KeyTile(model, r.botKey, 32.dp)
                Column(Modifier.weight(1f).clickable(onClickLabel = "Change when ${r.title} runs") { editing = r }.testTag("meet-routine-${r.id}")) {
                    Text(r.title, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink, maxLines = 1)
                    Text("${model.routineBotName(r.botKey)} · ${r.scheduleLabel}", fontSize = 12.sp, color = c.sub, maxLines = 1)
                }
                Text("Remove", fontSize = 12.sp, color = c.mut, modifier = Modifier.clickable { model.removeRoutine(r.id) }.padding(6.dp))
                Switch(checked = r.enabled, onCheckedChange = { model.toggleRoutine(r.id) },
                    colors = SwitchDefaults.colors(checkedTrackColor = c.coral))
            }
        }
        if (model.routines.isEmpty()) Text("None of your bots run on a schedule yet.", fontSize = 13.sp, color = c.mut)
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun JobStep(model: MeetYourBotsModel) {
    val c = setupColors
    H2("Give ${model.leader} a first job")
    Sub("Each bot comes with a first job that proves it works. Pick one and edit it if you like.")
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        model.jobs.forEach { job ->
            val on = model.jobKey == job.key
            Column(
                Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(14.dp))
                    .border(if (on) 1.5.dp else 1.dp, if (on) c.ink else c.line, RoundedCornerShape(14.dp))
                    .clickable(enabled = !on) { model.selectJob(job) }.padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    KeyTile(model, job.key, 28.dp)
                    Text(job.name, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink, modifier = Modifier.weight(1f))
                    if (job.key == MEET_MAIN_KEY) Text("MAIN BOT", style = mono(10.sp, FontWeight.SemiBold), color = c.coral)
                }
                if (!on) {
                    Text(job.prompt, fontSize = 12.5.sp, color = c.sub, maxLines = 2)
                } else {
                    BasicTextField(
                        value = model.jobText, onValueChange = { model.jobText = it },
                        textStyle = TextStyle(fontSize = 14.sp, color = c.ink), cursorBrush = SolidColor(c.ink),
                        modifier = Modifier.fillMaxWidth().background(c.fill, RoundedCornerShape(10.dp)).padding(10.dp).testTag("meet-job-text"),
                    )
                    if (job.starters.isNotEmpty()) {
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            job.starters.forEach { s ->
                                Text(s, fontSize = 12.5.sp, color = c.ink, modifier = Modifier.background(c.fill2, RoundedCornerShape(10.dp))
                                    .clickable { model.jobText = s }.padding(horizontal = 10.dp, vertical = 6.dp))
                            }
                        }
                    }
                }
            }
        }
    }
}

private data class DoneRow(val key: String, val value: String, val detail: String? = null, val bad: Boolean = false, val busy: Boolean = false)

@Composable
private fun DoneStep(model: MeetYourBotsModel) {
    val c = setupColors
    val items = model.items
    fun item(k: String) = items.firstOrNull { it.key == k }
    val bots = items.filter { it.key.startsWith("bot:") }
    val botBad = bots.filter { it.status == MeetItemStatus.Failed }
    val main = item("main")
    val routines = item("routines")
    val thread = item("thread")
    val jobName = model.jobs.firstOrNull { it.key == model.jobKey }?.name
    val n = model.members.size
    val anyFailed = items.any { it.status == MeetItemStatus.Failed }
    val rows = listOf(
        DoneRow("main bot", model.leader, if (main?.status == MeetItemStatus.Failed) "Not set up: ${main?.detail.orEmpty()}" else null,
            main?.status == MeetItemStatus.Failed, main?.status == MeetItemStatus.Running),
        DoneRow("team", "$n bot${if (n == 1) "" else "s"}",
            if (botBad.isEmpty()) null else "Not added: " + botBad.joinToString("; ") { "${it.label} (${it.detail.orEmpty()})" },
            botBad.isNotEmpty(), bots.any { it.status == MeetItemStatus.Running || it.status == MeetItemStatus.Pending }),
        DoneRow("routines", "${model.routinesOn} on", if (routines?.status == MeetItemStatus.Failed) routines?.detail else null,
            routines?.status == MeetItemStatus.Failed, routines?.status == MeetItemStatus.Running),
        DoneRow("thread #1", if (thread == null) "—" else jobName ?: "—", if (thread?.status == MeetItemStatus.Failed) "Didn’t start: ${thread?.detail.orEmpty()}" else null,
            thread?.status == MeetItemStatus.Failed, thread?.status == MeetItemStatus.Running),
        DoneRow("status", if (model.running) "setting up…" else if (anyFailed) "needs attention" else "running", null, !model.running && anyFailed, model.running),
    )
    val anyBad = rows.any { it.bad }
    val clean = !model.running && !anyBad
    H2(if (model.running) "Setting up your team" else if (anyBad) "Your team is mostly ready" else "Your team is ready")
    Spacer(Modifier.height(12.dp))
    if (clean) {
        // What was made, ticked, then the next useful step (web OnboardingChecklist).
        val botNames = items.filter { it.key.startsWith("bot:") }.map { it.label }
        val steps = buildList {
            add("Main bot: ${model.leader}" to "Answers first and hands work to the team.")
            add("Team: $n bot${if (n == 1) "" else "s"}" to botNames.joinToString(", ").ifEmpty { null })
            add("Routines: ${model.routinesOn} on" to routines?.detail)
            if (thread != null) add("Thread #1 started${jobName?.let { " with $it" } ?: ""}" to thread.detail)
        }
        Column(Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(14.dp)).border(1.dp, c.line, RoundedCornerShape(14.dp)).padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Finish setting up your team", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink, modifier = Modifier.weight(1f))
                Text("${steps.size}/${steps.size + 1}", style = mono(11.5.sp, FontWeight.SemiBold), color = c.mut)
            }
            steps.forEach { (title, detail) -> CheckRow(true, title, detail) }
            CheckRow(false, "Connect ChatGPT or Claude", "Your bots use your own subscription instead of credits. Connect one in Settings › Connections.")
        }
    } else {
        Column(Modifier.fillMaxWidth().background(c.term(), RoundedCornerShape(12.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            rows.forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("▸ ${row.key}", style = mono(12.sp), color = Color(0xFF7C7C86), modifier = Modifier.width(96.dp))
                    Column(Modifier.weight(1f)) {
                        Text(row.value, style = mono(12.sp, FontWeight.SemiBold), color = if (row.bad) Color(0xFFF19A9A) else Color(0xFFD4D4D8))
                        row.detail?.let { Text(it, style = mono(11.sp), color = Color(0xFFA1A1AA)) }
                    }
                    if (row.key == "status" && !row.bad) Text("●", fontSize = 12.sp, color = if (row.busy) c.coral else Color(0xFF5FD39B))
                }
            }
        }
    }
    val suggestions = model.suggestions
    if (suggestions.isNotEmpty()) {
        Text("Works well with your team", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink, modifier = Modifier.padding(top = 18.dp))
        Text("Your bots hand work to these. Tap one to add it.", fontSize = 12.5.sp, color = c.sub, modifier = Modifier.padding(top = 2.dp, bottom = 8.dp))
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            suggestions.forEach { t ->
                if (t.id in model.adding) {
                    Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
                        Text("Adding ${t.name}…", fontSize = 13.sp, color = c.sub)
                    }
                } else {
                    TemplateCard(t, on = model.isMade(t.id), enabled = !model.running && !model.mainFailed) { if (!model.isMade(t.id)) model.addLater(t.id) }
                }
            }
        }
    }
}

private fun SetupColors.term(): Color = if (bg == Color.White) Color(0xFF0F0F0F) else Color(0xFF0A0A0A)

@Composable
private fun CheckRow(done: Boolean, title: String, detail: String?) {
    val c = setupColors
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(
            Modifier.size(18.dp).then(if (done) Modifier.background(c.ok, CircleShape) else Modifier.border(1.5.dp, c.lineDash, CircleShape)),
            contentAlignment = Alignment.Center,
        ) { if (done) Text("✓", fontSize = 11.sp, fontWeight = FontWeight.Bold, color = Color.White) }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, color = c.ink)
            if (!detail.isNullOrEmpty()) Text(detail, fontSize = 12.sp, color = c.mut)
        }
    }
}

@Composable
private fun ActionTile(glyph: String) {
    val c = setupColors
    Box(Modifier.size(52.dp).background(c.card, RoundedCornerShape(14.dp)).border(1.dp, c.line, RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
        Text(glyph, fontSize = 20.sp, color = c.sub)
    }
}

/**
 * The routines picture with Day, Week and Month (iOS MeetYourBotsVisuals routines views): Day is the
 * timeline; Week shows each routine on the weekdays its cron runs; Month marks the days of the month.
 */
@Composable
private fun MeetRoutinesPicture(model: MeetYourBotsModel) {
    val c = setupColors
    var view by remember { mutableStateOf("Day") }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.padding(10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("Day", "Week", "Month").forEach { v ->
                Text(v, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = if (view == v) c.bg else c.ink,
                    modifier = Modifier.background(if (view == v) c.ink else c.card, RoundedCornerShape(50)).clickable { view = v }.testTag("meet-routines-$v").padding(horizontal = 12.dp, vertical = 5.dp))
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val rules = model.routines.filter { it.enabled }.map { it to com.allternit.feature.bots.RoutineRule.fromCron(it.cron) }
            when (view) {
                "Day" -> MeetRoutineDay(model.routines, { key -> botColor(model, key) }, model::toggleRoutine)
                "Week" -> Row(Modifier.fillMaxSize().padding(horizontal = 10.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    com.allternit.feature.bots.RoutineRule.DAY_SHORT.forEachIndexed { d, name ->
                        Column(Modifier.weight(1f).fillMaxHeight().background(c.card, RoundedCornerShape(8.dp)).padding(3.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Text(name, fontSize = 10.sp, color = c.mut, modifier = Modifier.align(Alignment.CenterHorizontally))
                            rules.filter { (_, rule) -> rule == null || (rule.repeatKind != com.allternit.feature.bots.RoutineRule.Repeat.Monthly && d in rule.days) }.forEach { (r, _) ->
                                Box(Modifier.fillMaxWidth().height(14.dp).background(botColor(model, r.botKey).copy(alpha = 0.8f), RoundedCornerShape(4.dp)))
                            }
                        }
                    }
                }
                else -> Column(Modifier.fillMaxSize().padding(10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    val today = java.time.LocalDate.now()
                    val first = today.withDayOfMonth(1)
                    (1..today.lengthOfMonth()).chunked(7).forEach { week ->
                        Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            week.forEach { dom ->
                                val dow = first.plusDays((dom - 1).toLong()).dayOfWeek.value % 7
                                val on = rules.filter { (_, rule) -> rule == null || (if (rule.repeatKind == com.allternit.feature.bots.RoutineRule.Repeat.Monthly) dom in rule.dom.orEmpty() else dow in rule.days) }
                                Column(Modifier.weight(1f).fillMaxHeight().background(if (dom == today.dayOfMonth) c.fill else c.card, RoundedCornerShape(6.dp)).padding(2.dp)) {
                                    Text("$dom", fontSize = 9.sp, color = c.mut)
                                    Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) { on.take(4).forEach { (r, _) -> Box(Modifier.size(5.dp).background(botColor(model, r.botKey), RoundedCornerShape(50))) } }
                                }
                            }
                            repeat(7 - week.size) { Spacer(Modifier.weight(1f)) }
                        }
                    }
                }
            }
        }
    }
}
