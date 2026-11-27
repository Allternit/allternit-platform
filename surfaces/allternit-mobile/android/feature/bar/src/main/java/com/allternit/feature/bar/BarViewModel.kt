package com.allternit.feature.bar

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * The bar's state machine (iOS IslandBarModel): apps → tools → blanks → review → live → done. One per process
 * ([shared]) so a job keeps being followed after the bar is hidden; [BarJobs.current] carries it to lane 37.
 */
class BarViewModel internal constructor(private val client: BarClient = BarClient()) {
    enum class Step { Apps, Tools, Blanks, Review, Live, Done }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var step by mutableStateOf(Step.Apps); private set
    var query by mutableStateOf("")
    var apps by mutableStateOf<List<BarApp>>(emptyList()); private set
    var app by mutableStateOf<BarApp?>(null); private set
    var tools by mutableStateOf<List<BarTool>>(emptyList()); private set
    var intent by mutableStateOf<BarIntent?>(null); private set
    val values = mutableStateMapOf<String, BarValue>()
    var focusedBlank by mutableStateOf<String?>(null)
    var review by mutableStateOf<BarReview?>(null); private set
    var botId by mutableStateOf<String?>(null); private set
    var loading by mutableStateOf(false); private set
    var errorText by mutableStateOf<String?>(null); private set
    var noComputer by mutableStateOf(false); private set
    var jobId by mutableStateOf<String?>(null); private set
    var jobState by mutableStateOf<String?>(null); private set
    var progressLine by mutableStateOf<String?>(null); private set
    var receipt by mutableStateOf<String?>(null); private set
    var jobError by mutableStateOf<String?>(null); private set
    /** Bumped on a terminal state so the screen plays the success haptic once. */
    var finishedTick by mutableStateOf(0); private set

    private var follow: Job? = null
    private var pendingValues: Map<String, String> = emptyMap()
    private var jobTitle: String = ""

    val isTerminal: Boolean get() = (jobState ?: "") in BarJobState.TERMINAL

    val approverName: String? get() = review?.takeIf { !it.approverIsYou }?.approverName

    val stateLine: String get() = BarWording.stateLine(jobState ?: "working", approverName, receipt, jobError)

    val matchingApps: List<BarApp>
        get() {
            val words = query.lowercase().split(' ').filter { it.length > 1 }
            if (words.isEmpty()) return apps
            val hits = apps.filter { a -> words.any { a.name.lowercase().contains(it) || it.contains(a.name.lowercase()) } }
            return hits.ifEmpty { apps }
        }

    val firstEmptyBlank: String? get() = intent?.blanks?.firstOrNull { it.required && values[it.key] == null }?.key
    val canReview: Boolean get() = intent != null && firstEmptyBlank == null

    fun open() {
        if (step == Step.Apps && apps.isEmpty()) scope.launch { loadApps() }
    }

    /** A deep link or an entry with an app / tool / values (iOS open(with:)). While a job runs, just show it. */
    fun open(request: BarLinkRequest) {
        if (step == Step.Live) return
        reset()
        botId = request.bot
        pendingValues = request.values
        scope.launch {
            loadApps()
            val id = request.app ?: return@launch
            val picked = apps.firstOrNull { it.id == id } ?: BarApp.split(id) ?: return@launch
            pick(picked, request.tool)
        }
    }

    /** Hiding the bar: a finished job resets; a running one keeps following. */
    fun close() {
        if (step == Step.Done) reset()
    }

    fun reset() {
        if (step == Step.Live) return
        jobId?.let { com.allternit.core.design.JobBanners.dismiss(it) }
        step = Step.Apps; query = ""; app = null; tools = emptyList(); intent = null; values.clear(); focusedBlank = null
        review = null; botId = null; errorText = null; noComputer = false; jobId = null; jobState = null
        progressLine = null; receipt = null; jobError = null; pendingValues = emptyMap()
        BarJobs.publish(null)
    }

    suspend fun loadApps() {
        loading = true
        try {
            apps = client.apps()
            errorText = null
        } catch (e: Exception) {
            show(e)
        } finally {
            loading = false
        }
    }

    fun pickApp(app: BarApp) { scope.launch { pick(app, null) } }

    fun pickTool(tool: BarTool) { scope.launch { pick(tool) } }

    private suspend fun pick(app: BarApp, tool: String?) {
        if (!app.isRunnable) {
            errorText = BarWording.notRunnable
            return
        }
        this.app = app
        errorText = null
        loading = true
        try {
            val list = client.tools(app)
            tools = list
            val given = tool?.lowercase()
            val hit = given?.let { g -> list.firstOrNull { it.name.lowercase() == g || it.title.lowercase() == g } }
                ?: list.singleOrNull()
                ?: bestTool(list)
            if (hit != null) {
                pick(hit)
            } else {
                step = if (list.isEmpty()) Step.Apps else Step.Tools
                if (list.isEmpty()) errorText = BarWording.noTools
            }
        } catch (e: Exception) {
            show(e)
        } finally {
            loading = false
        }
    }

    private fun bestTool(list: List<BarTool>): BarTool? {
        val words = query.lowercase().split(' ').filter { it.isNotEmpty() }.toSet()
        if (words.isEmpty()) return null
        return list.firstOrNull { t -> t.title.lowercase().split(' ').any { it in words } }
    }

    private suspend fun pick(tool: BarTool) {
        val app = app ?: return
        loading = true
        try {
            val intent = client.intent(app, tool.name, botId)
            this.intent = intent
            values.clear()
            intent.blanks.forEach { b -> pendingValues[b.key]?.let { BarWording.parse(it, b) }?.let { values[b.key] = it } }
            pendingValues = emptyMap()
            // A number typed in the bar ("refund 320") fills the first empty number blank (iOS).
            val n = query.split(' ').firstNotNullOfOrNull { w -> w.filter { it.isDigit() || it == '.' }.toDoubleOrNull() }
            val numberBlank = intent.blanks.firstOrNull { it.type in setOf("number", "integer") && values[it.key] == null }
            if (n != null && numberBlank != null) values[numberBlank.key] = BarValue.Number(n)
            step = Step.Blanks
            focusedBlank = firstEmptyBlank ?: intent.blanks.firstOrNull()?.key
            errorText = null
        } catch (e: Exception) {
            show(e)
        } finally {
            loading = false
        }
    }

    fun set(value: BarValue?, key: String) {
        if (value == null) values.remove(key) else values[key] = value
        if (value != null && focusedBlank == key) focusedBlank = firstEmptyBlank ?: key
    }

    fun goReview() {
        val intent = intent ?: return
        scope.launch {
            loading = true
            try {
                review = client.review(intent, values.toMap(), botId)
                step = Step.Review
                errorText = null
            } catch (e: Exception) {
                show(e)
            } finally {
                loading = false
            }
        }
    }

    fun back() {
        errorText = null
        when (step) {
            Step.Tools -> { step = Step.Apps; app = null }
            Step.Blanks -> {
                step = if (tools.size > 1) Step.Tools else Step.Apps
                intent = null; values.clear()
                if (step == Step.Apps) app = null
            }
            Step.Review -> step = Step.Blanks
            else -> Unit
        }
    }

    fun run() {
        val review = review ?: return
        val approver = approverName
        jobId = review.jobId
        jobTitle = intent?.toolTitle.orEmpty()
        scope.launch {
            loading = true
            try {
                val job = client.run(review.jobId, approver)
                step = Step.Live
                if (job != null) applyJob(job) else { jobState = "working"; publish() }
            } catch (gate: BarFailure.Gate) {
                step = Step.Live
                val job = runCatching { client.job(review.jobId, gate.approverName ?: approver) }.getOrNull()
                if (job != null) applyJob(job) else { jobState = "waiting_approval"; publish() }
            } catch (e: Exception) {
                show(e)
                return@launch
            } finally {
                loading = false
            }
            startFollowing()
        }
    }

    fun stop() {
        val id = jobId ?: return
        scope.launch {
            client.cancel(id)
            runCatching { client.job(id, approverName) }.getOrNull()?.let(::applyJob)
        }
    }

    /** SSE first; when the stream drops or the server closes it early, poll every 2 s (iOS startFollowing). */
    private fun startFollowing() {
        follow?.cancel()
        val id = jobId ?: return
        val approver = approverName
        follow = scope.launch {
            val me = this
            try {
                client.events(id, approver).collect { event ->
                    handle(event)
                    if (isTerminal) me.cancel()
                }
            } catch (_: Exception) {
                // Dropped stream: fall through to polling.
            }
            while (isActive && !isTerminal && jobId == id) {
                delay(2_000)
                runCatching { client.job(id, approver) }.getOrNull()?.let(::applyJob)
            }
        }
    }

    private fun handle(event: BarEvent) {
        when (event) {
            is BarEvent.Job -> applyJob(event.job)
            is BarEvent.Task -> {
                if (event.status == "working") progressLine = event.message ?: progressLine
                if (jobState == "waiting_approval" && event.status == "working") jobState = "working"
                publish()
            }
            BarEvent.NeedsInput -> { jobState = "input_required"; publish() }
            BarEvent.InputResolved -> { if (jobState == "input_required") jobState = "working"; publish() }
        }
    }

    private fun applyJob(job: BarJob) {
        jobState = job.state
        job.receipt?.let { receipt = it }
        jobError = job.error
        if (job.title.isNotBlank()) jobTitle = job.title
        publish(job)
        if (isTerminal) {
            step = Step.Done
            finishedTick++
            follow?.cancel()
        }
    }

    private fun publish(job: BarJob? = null) {
        val id = jobId ?: return
        val line = BarWording.stateLine(jobState ?: "working", job?.approverName ?: approverName, receipt, jobError)
        val label = if (jobTitle.isBlank()) line else "$jobTitle · $line"
        if ((jobState ?: "") in BarJobState.TERMINAL) com.allternit.core.design.JobBanners.update(id, label, done = true)
        else com.allternit.core.design.JobBanners.update(id, label, done = false)
        BarJobs.publish(
            BarJobState(
                jobId = id,
                state = jobState ?: "working",
                title = jobTitle,
                appName = job?.appName ?: intent?.app?.name ?: app?.name,
                botName = job?.botName,
                approverName = job?.approverName ?: approverName,
                progressLine = progressLine,
                receipt = receipt,
                error = jobError,
            ),
        )
    }

    private fun show(e: Exception) {
        noComputer = e is BarFailure.NoComputer
        errorText = (e as? BarFailure)?.message ?: BarWording.fallback
    }

    companion object {
        val shared: BarViewModel by lazy { BarViewModel() }
    }
}
