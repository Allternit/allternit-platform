package com.allternit.core.api

/**
 * The one rule for "this account has no computer" (iOS APIError.isComputeGateRefusal, ComputeGate).
 * Every screen that loads relay data uses it, so none shows "isn't responding" or "couldn't load"
 * for a missing computer. Only a real timeout with a computer present is "isn't responding".
 */
object NoComputerRule {
    private val codes = Regex(
        """\b(compute_required|free_computers_retired|no_computer|no_runtime|no_runtime_selected|runtime_not_found|no_brain)\b""",
        RegexOption.IGNORE_CASE,
    )

    /** 428, a compute code on any status, or cloud-api's "No data-plane node" / "Agents need a computer." text. */
    fun isGate(status: Int?, code: String?, message: String?): Boolean {
        if (status == 428) return true
        val text = "${code.orEmpty()} ${message.orEmpty()}"
        if (text.contains("data-plane node", ignoreCase = true)) return true
        if (message == "Agents need a computer.") return true
        if (status == 503 && code != "runtime_offline" && code != "runtime_warming" && codes.containsMatchIn(text)) return true
        return (status == 404 || status == 409 || status == 410 || status == 502 || status == 503) && codes.containsMatchIn(text)
    }

    fun isGate(r: ApiResult.HttpError): Boolean = isGate(r.status, r.code, r.message)
}
