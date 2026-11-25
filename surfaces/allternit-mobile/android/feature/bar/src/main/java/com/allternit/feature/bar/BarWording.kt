package com.allternit.feature.bar

import java.text.NumberFormat
import java.util.Currency
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.doubleOrNull

/** Every sentence the bar shows, word for word with iOS (DoWording, IslandBarWording, ConnectionWording, CalmCopy in #3037). */
object BarWording {
    const val signedOut = "Sign in to Allternit first. Open the app to sign in."
    const val noComputer = "No computer connected"
    const val noComputerDetail = "Connect a computer or start Allternit Cloud to use this."
    const val fallback = "Something went wrong. Try again."
    const val notRunnable = "Apps made from artifacts can't run tools yet."
    const val noTools = "That app has no tools you can run."
    const val noApps = "No connected apps yet. Connect one under Connections."
    const val loadingApps = "Loading your apps"
    const val prompt = "What do you want to do?"
    const val whichApp = "Which app?"
    const val nothingToFill = "Nothing to fill in."
    const val answerInTasks = "Answer it in Tasks."
    const val entry = "Do with an app"

    fun whatInApp(appName: String?) = "What should it do in ${appName ?: "it"}?"

    fun ownForm(appName: String) = "This needs $appName's own form. Open $appName on your computer to fill it in."

    fun level(level: String): String = when (level) {
        "browser" -> "Browser level"
        "app" -> "App level"
        "full" -> "Full level"
        else -> level.replaceFirstChar { it.uppercase() }
    }

    /** "App level · 3 tools"; artifact apps say they cannot run yet. */
    fun subtitle(app: BarApp): String {
        if (!app.isRunnable) return "Not runnable yet"
        val tools = if (app.toolCount == 1) "1 tool" else "${app.toolCount} tools"
        return listOf(level(app.level), tools).filter { it.isNotEmpty() }.joinToString(" · ")
    }

    /** The picker's question: "Which order?", "What amount, in EUR?". */
    fun question(blank: BarBlank): String {
        val label = blank.label.ifEmpty { blank.key }.lowercase()
        return when (blank.type) {
            "number", "integer" -> if (blank.unit != null) "What $label, in ${blank.unit}?" else "What $label?"
            "boolean" -> "${blank.label.ifEmpty { blank.key }}? Yes or no."
            "enum" -> {
                val choices = blank.options.joinToString(", ") { it.label }
                if (choices.isEmpty()) "Which $label?" else "Which $label? $choices."
            }
            else -> "What $label?"
        }
    }

    /** "Noah gets a push. It expires in 24 hours." / "You approve." */
    fun approverLine(review: BarReview): String? {
        if (review.approverIsYou) return "You approve."
        val name = review.approverName ?: return null
        return "$name gets a push. It expires in 24 hours."
    }

    fun runLabel(review: BarReview) = if (review.approverIsYou || review.approverName == null) "Run" else "Send for approval"

    /** The live line (iOS TaskActivityAttributes.ContentState.stateLine; CONTRACT section 5). */
    fun stateLine(state: String, approverName: String?, receipt: String?, error: String?): String = when (state) {
        "waiting_approval" -> if (approverName.isNullOrBlank()) "Waiting on approval" else "Waiting on $approverName"
        "working" -> "Working"
        "input_required" -> "Needs input"
        "review" -> "Review"
        "completed" -> receipt?.takeIf { it.isNotBlank() }?.let(::pastTense) ?: "Completed"
        "failed" -> if (error.isNullOrBlank()) "Failed" else "Failed: $error"
        "cancelled" -> "Cancelled"
        else -> "Working"
    }

    /** Asked, Approval, Running, Done: how many steps the job has reached (1-4). */
    fun stepsReached(state: String): Int = when (state) {
        "waiting_approval" -> 2
        "working", "input_required" -> 3
        "completed", "failed", "cancelled" -> 4
        else -> 1
    }

    /** Parses typed or deep-linked text into the blank's value; null when it does not fit (iOS DoWording.parse). */
    fun parse(raw: String, blank: BarBlank): BarValue? {
        val text = raw.trim()
        if (text.isEmpty()) return null
        return when (blank.type) {
            "number", "integer" -> text.filter { it.isDigit() || it == '.' || it == '-' }.toDoubleOrNull()?.let { BarValue.Number(it) }
            "boolean" -> when (text.lowercase()) {
                "yes", "true", "on", "1" -> BarValue.Bool(true)
                "no", "false", "off", "0" -> BarValue.Bool(false)
                else -> null
            }
            "enum" -> blank.options.firstOrNull { it.value.equals(text, true) || it.label.equals(text, true) }?.let { BarValue.Text(it.value) }
            else -> BarValue.Text(text)
        }
    }

    /** A value as the sentence shows it: an enum's label, otherwise the value. */
    fun display(value: BarValue, blank: BarBlank?): String =
        if (value is BarValue.Text) blank?.options?.firstOrNull { it.value == value.value }?.label ?: value.value else value.display

    // MARK: - Receipt (CONTRACT section 5: past tense, "Refunded €320")

    private val known = mapOf(
        "refund" to "Refunded", "create" to "Created", "send" to "Sent", "add" to "Added", "update" to "Updated",
        "delete" to "Deleted", "remove" to "Removed", "cancel" to "Cancelled", "list" to "Listed", "post" to "Posted",
        "charge" to "Charged", "pay" to "Paid", "book" to "Booked", "schedule" to "Scheduled", "close" to "Closed",
        "open" to "Opened", "archive" to "Archived", "invite" to "Invited", "approve" to "Approved", "reply" to "Replied",
        "get" to "Got", "find" to "Found", "search" to "Searched", "issue" to "Issued", "transfer" to "Transferred",
        "assign" to "Assigned", "move" to "Moved", "start" to "Started", "stop" to "Stopped", "run" to "Ran",
    )

    fun pastTense(verb: String): String? = known[verb.lowercase()]

    /** A pushed or server receipt ("Refund payment €320") in the past tense ("Refunded €320"). */
    fun pastTense(receipt: String): String {
        val words = receipt.split(' ').filter { it.isNotEmpty() }
        val verb = words.firstOrNull()?.let(::pastTense) ?: return receipt
        val rest = words.drop(1).toMutableList()
        if (rest.lastOrNull()?.lowercase() == "done") {
            rest.removeAt(rest.lastIndex)
            return if (rest.isEmpty()) verb else "$verb ${rest.joinToString(" ").lowercase()}"
        }
        val i = rest.indexOfFirst { w -> w.any { it.isDigit() || Character.getType(it) == Character.CURRENCY_SYMBOL.toInt() } }
        if (i >= 0) return "$verb ${rest.drop(i).joinToString(" ")}"
        return if (rest.isEmpty()) verb else "$verb ${rest.joinToString(" ").lowercase()}"
    }

    /** The server's `receipt` when sent (round 3), else built from the title and amount like iOS BarWire.receipt. */
    fun receipt(job: JsonObject?): String {
        job.s("receipt")?.takeIf { it.isNotBlank() }?.let { return pastTense(it) }
        val title = job.s("title")?.takeIf { it.isNotBlank() } ?: job.s("tool") ?: "Done"
        val structured = job?.get("result").o()?.get("structuredContent").o()
        val args = job?.get("args").o()
        val amount = (structured?.get("amount") as? JsonPrimitive)?.doubleOrNull ?: (args?.get("amount") as? JsonPrimitive)?.doubleOrNull
        val currency = structured.s("currency") ?: args.s("currency")
        val words = title.split(' ').filter { it.isNotEmpty() }
        val verb = words.firstOrNull()?.let(::pastTense) ?: return "$title: done"
        if (amount != null) return "$verb ${money(amount, currency)}"
        val rest = words.drop(1).joinToString(" ")
        return if (rest.isEmpty()) verb else "$verb ${rest.lowercase()}"
    }

    private fun money(value: Double, currency: String?): String {
        val whole = value % 1.0 == 0.0
        val code = currency?.uppercase()?.takeIf { it.length == 3 } ?: return if (whole) value.toLong().toString() else value.toString()
        return runCatching {
            NumberFormat.getCurrencyInstance().apply {
                this.currency = Currency.getInstance(code)
                maximumFractionDigits = if (whole) 0 else 2
                minimumFractionDigits = if (whole) 0 else 2
            }.format(value)
        }.getOrDefault("$value $code")
    }
}
