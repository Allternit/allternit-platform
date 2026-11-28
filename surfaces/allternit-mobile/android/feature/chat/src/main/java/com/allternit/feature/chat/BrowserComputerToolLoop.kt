package com.allternit.feature.chat

import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put

/**
 * Client-driven tool loop for the browser computer (CONTRACT-colibri-unkernel section 7), the Kotlin twin of iOS
 * `BrowserComputerToolLoop.swift` and allternit-ai `tool-loop.ts`.
 *
 * The phone sends `tools`, receives a tool call, runs it through the box RPC `exec`, and sends the result on the next
 * request. No server holds the loop, so a closed app cannot leave a hanging run. The wire call and the executor are
 * injected, so the loop is tested with fakes.
 */
object BrowserToolCaps {
    const val STREAM_BYTES = 16 * 1024
    const val STREAM_KEEP_BYTES = 8 * 1024
    const val RESULT_MESSAGE_BYTES = 32 * 1024
    const val DEFAULT_TIMEOUT_SECONDS = 60
    const val MAX_TIMEOUT_SECONDS = 300
    const val RUN_CODE_MAX_CHARS = 200_000
    const val WRITE_FILE_MAX_CHARS = 1_048_576
    const val READ_FILE_DEFAULT_BYTES = 65_536
    const val READ_FILE_MAX_BYTES = 262_144
    const val MAX_ROUNDS_PER_MESSAGE = 8
}

/** Contract 7.4: the exact line a vendor plan brain gets in place of tools. */
const val BROWSER_PLAN_NO_TOOLS_LINE = "Plan models cannot run commands. Open the Terminal to use your computer yourself."

/** The `exec` params of contract 6.2. [stdin] is base64 text. */
data class BoxExecParams(
    val cmd: String,
    val args: List<String> = emptyList(),
    val cwd: String? = null,
    val stdin: String? = null,
    val timeoutMs: Long = 60_000,
)

data class BoxExecResult(val stdout: String, val stderr: String, val exitCode: Int, val timedOut: Boolean = false)

/** Runs a command in the browser computer. Implemented by the computer's host in feature:computer. */
interface BrowserComputerExecutor {
    suspend fun exec(params: BoxExecParams): BoxExecResult
}

/** The running browser computer, for the chat to find. The computer's screen sets it when the VM is ready and clears it when it stops. */
object BrowserComputerRegistry {
    @Volatile var executor: BrowserComputerExecutor? = null
        private set

    fun attach(e: BrowserComputerExecutor) { executor = e }

    /** Clears only if [e] is still the attached one, so a newer computer is not detached by an older one's teardown. */
    fun detach(e: BrowserComputerExecutor) { if (executor === e) executor = null }
}

data class BrowserToolCall(val id: String, val name: String, val arguments: String)

class BrowserToolRequest(
    val messages: List<JsonObject>,
    /** Null for a brain that cannot take tools. */
    val tools: JsonArray?,
    /** True on the last round: `tool_choice: "none"`. */
    val toolChoiceNone: Boolean = false,
    val onDelta: (String) -> Unit,
)

class BrowserToolResponse(val toolCalls: List<BrowserToolCall> = emptyList())

typealias BrowserStreamCall = suspend (BrowserToolRequest) -> BrowserToolResponse

enum class BrowserToolBrain { Tools, Plan }

class BrowserToolLoopCallbacks(
    val onChunk: (String) -> Unit = {},
    val onToolCall: (id: String, name: String, input: Map<String, String>) -> Unit = { _, _, _ -> },
    val onToolResult: (id: String, name: String, result: String) -> Unit = { _, _, _ -> },
    val onToolError: (id: String, name: String, error: String) -> Unit = { _, _, _ -> },
)

data class BrowserToolLoopResult(val rounds: Int, /** "Used N of 8 tool rounds", or null when no tool ran. */ val roundsLine: String?)

object BrowserToolDefinitions {
    /** OpenAI function format, copied from contract 7.1. */
    val all: JsonArray by lazy {
        Json.parseToJsonElement(
            """
            [
              { "type": "function", "function": {
                  "name": "bash",
                  "description": "Run a shell command in the user's Linux computer. Returns stdout, stderr and the exit code.",
                  "parameters": { "type": "object", "additionalProperties": false, "required": ["command"],
                    "properties": {
                      "command": { "type": "string", "description": "Shell text, run as sh -c." },
                      "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 60 } } } } },
              { "type": "function", "function": {
                  "name": "run_code",
                  "description": "Run a short program in the user's Linux computer and return its output.",
                  "parameters": { "type": "object", "additionalProperties": false, "required": ["language", "code"],
                    "properties": {
                      "language": { "type": "string", "enum": ["python", "node"] },
                      "code": { "type": "string", "maxLength": 200000 },
                      "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300, "default": 60 } } } } },
              { "type": "function", "function": {
                  "name": "read_file",
                  "description": "Read a text file from the user's Linux computer.",
                  "parameters": { "type": "object", "additionalProperties": false, "required": ["path"],
                    "properties": {
                      "path": { "type": "string", "description": "Absolute, or relative to /root." },
                      "max_bytes": { "type": "integer", "minimum": 1, "maximum": 262144, "default": 65536 } } } } },
              { "type": "function", "function": {
                  "name": "write_file",
                  "description": "Write a text file in the user's Linux computer. Creates parent folders.",
                  "parameters": { "type": "object", "additionalProperties": false, "required": ["path", "content"],
                    "properties": {
                      "path": { "type": "string" },
                      "content": { "type": "string", "maxLength": 1048576 },
                      "append": { "type": "boolean", "default": false } } } } }
            ]
            """.trimIndent(),
        ) as JsonArray
    }
}

object BrowserToolLoop {
    // MARK: caps

    /** Keeps the first and last [keep] bytes; the cut is named in the middle. */
    fun cutMiddle(text: String, total: Int, keep: Int): String {
        val bytes = text.toByteArray(Charsets.UTF_8)
        if (bytes.size <= total) return text
        val cut = bytes.size - 2 * keep
        val head = String(bytes, 0, keep, Charsets.UTF_8)
        val tail = String(bytes, bytes.size - keep, keep, Charsets.UTF_8)
        return "$head\n[... $cut bytes cut ...]\n$tail"
    }

    private fun byteCount(s: String) = s.toByteArray(Charsets.UTF_8).size

    /** A command stream returned to the model: over 16 KiB keeps 8 KiB from each end. */
    fun capStream(text: String): Pair<String, Boolean> =
        if (byteCount(text) <= BrowserToolCaps.STREAM_BYTES) text to false
        else cutMiddle(text, BrowserToolCaps.STREAM_BYTES, BrowserToolCaps.STREAM_KEEP_BYTES) to true

    /** The whole result message stays under 32 KiB: shrink the biggest text field until it fits. */
    fun fitResultMessage(result: Map<String, JsonElement>): String {
        val out = result.toMutableMap()
        var json = JsonObject(out).toString()
        var i = 0
        while (i < 24 && byteCount(json) > BrowserToolCaps.RESULT_MESSAGE_BYTES) {
            i++
            val field = listOf("stdout", "stderr", "content")
                .filter { (out[it] as? JsonPrimitive)?.isString == true }
                .maxByOrNull { byteCount((out[it] as JsonPrimitive).content) } ?: break
            val current = (out[field] as JsonPrimitive).content
            val keep = maxOf(16, byteCount(current) / 4)
            out[field] = JsonPrimitive(cutMiddle(current, 0, keep))
            out["truncated"] = JsonPrimitive(true)
            json = JsonObject(out).toString()
        }
        return json
    }

    // MARK: tools

    private fun timeoutMs(v: JsonElement?): Long {
        val n = (v as? JsonPrimitive)?.doubleOrNull ?: return BrowserToolCaps.DEFAULT_TIMEOUT_SECONDS * 1000L
        return n.toLong().coerceIn(1, BrowserToolCaps.MAX_TIMEOUT_SECONDS.toLong()) * 1000
    }

    private fun fail(error: String): Map<String, JsonElement> = mapOf("error" to JsonPrimitive(error))

    private fun commandResult(r: BoxExecResult): Map<String, JsonElement> {
        val (out, outCut) = capStream(r.stdout)
        val (err, errCut) = capStream(r.stderr)
        return mapOf(
            "exit_code" to JsonPrimitive(r.exitCode), "stdout" to JsonPrimitive(out), "stderr" to JsonPrimitive(err),
            "truncated" to JsonPrimitive(outCut || errCut), "timed_out" to JsonPrimitive(r.timedOut),
        )
    }

    /** Content goes in stdin as base64 and the path in args, never in shell text. */
    private suspend fun writeText(ex: BrowserComputerExecutor, path: String, content: String, append: Boolean): Map<String, JsonElement> {
        val script = if (append) "mkdir -p \"$(dirname -- \"\$1\")\" && base64 -d >> \"\$1\""
        else "mkdir -p \"$(dirname -- \"\$1\")\" && base64 -d > \"\$1\""
        val stdin = java.util.Base64.getEncoder().encodeToString(content.toByteArray(Charsets.UTF_8))
        val r = ex.exec(BoxExecParams("sh", listOf("-c", script, "sh", path), cwd = "/root", stdin = stdin, timeoutMs = 60_000))
        if (r.exitCode != 0) return fail(r.stderr.trim().ifEmpty { "Could not write the file." })
        return mapOf("bytes_written" to JsonPrimitive(content.toByteArray(Charsets.UTF_8).size))
    }

    private var tempCounter = 0

    private fun intArg(v: JsonElement?): Int? = (v as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull?.toInt()

    /** Runs one tool call. Never throws (except cancellation): a failure is `{error}` and the loop continues. */
    suspend fun runTool(ex: BrowserComputerExecutor, name: String, rawArgs: String): Map<String, JsonElement> {
        val args = runCatching { Json.parseToJsonElement(rawArgs.ifEmpty { "{}" }) as? JsonObject }.getOrNull()
            ?: return fail("The tool arguments were not valid JSON.")
        fun str(key: String) = (args[key] as? JsonPrimitive)?.takeIf { it.isString }?.content
        try {
            when (name) {
                "bash" -> {
                    val command = str("command")?.takeIf { it.isNotEmpty() } ?: return fail("bash needs a command.")
                    return commandResult(ex.exec(BoxExecParams("sh", listOf("-c", command), cwd = "/root", timeoutMs = timeoutMs(args["timeout_seconds"]))))
                }
                "run_code" -> {
                    val language = str("language")
                    if (language != "python" && language != "node") return fail("run_code language must be python or node.")
                    val code = str("code")?.takeIf { it.isNotEmpty() } ?: return fail("run_code needs code.")
                    if (code.length > BrowserToolCaps.RUN_CODE_MAX_CHARS) return fail("The code is too long to run.")
                    tempCounter += 1
                    val file = "/tmp/run-${java.lang.Long.toString(System.currentTimeMillis(), 36)}-$tempCounter.${if (language == "python") "py" else "js"}"
                    val wrote = writeText(ex, file, code, append = false)
                    if (wrote["error"] != null) return wrote
                    val result = try {
                        commandResult(ex.exec(BoxExecParams(if (language == "python") "python3" else "node", listOf(file), cwd = "/root", timeoutMs = timeoutMs(args["timeout_seconds"]))))
                    } catch (e: Exception) {
                        runCatching { ex.exec(BoxExecParams("rm", listOf("-f", file), timeoutMs = 10_000)) }
                        throw e
                    }
                    runCatching { ex.exec(BoxExecParams("rm", listOf("-f", file), timeoutMs = 10_000)) }
                    return result
                }
                "read_file" -> {
                    val path = str("path")?.takeIf { it.isNotEmpty() } ?: return fail("read_file needs a path.")
                    val max = intArg(args["max_bytes"])?.coerceIn(1, BrowserToolCaps.READ_FILE_MAX_BYTES) ?: BrowserToolCaps.READ_FILE_DEFAULT_BYTES
                    // One byte past the limit tells us whether the file was longer.
                    val r = ex.exec(BoxExecParams("head", listOf("-c", (max + 1).toString(), "--", path), cwd = "/root", timeoutMs = 30_000))
                    if (r.exitCode != 0) return fail(r.stderr.trim().ifEmpty { "Could not read the file." })
                    val bytes = r.stdout.toByteArray(Charsets.UTF_8)
                    val truncated = bytes.size > max
                    return mapOf("content" to JsonPrimitive(if (truncated) String(bytes, 0, max, Charsets.UTF_8) else r.stdout), "truncated" to JsonPrimitive(truncated))
                }
                "write_file" -> {
                    val path = str("path")?.takeIf { it.isNotEmpty() } ?: return fail("write_file needs a path.")
                    val content = str("content") ?: return fail("write_file needs content.")
                    if (content.length > BrowserToolCaps.WRITE_FILE_MAX_CHARS) return fail("The file content is too long to write.")
                    return writeText(ex, path, content, (args["append"] as? JsonPrimitive)?.booleanOrNull == true)
                }
                else -> return fail("There is no tool called $name.")
            }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            return fail(e.message?.takeIf { it.isNotEmpty() } ?: "The computer did not answer.")
        }
    }

    // MARK: loop

    private fun assistantToolCalls(calls: List<BrowserToolCall>): JsonObject = buildJsonObject {
        put("role", "assistant")
        put("content", JsonNull)
        put("tool_calls", JsonArray(calls.map { c ->
            buildJsonObject {
                put("id", c.id); put("type", "function")
                put("function", buildJsonObject { put("name", c.name); put("arguments", c.arguments) })
            }
        }))
    }

    private fun toolResult(callId: String, content: String): JsonObject = buildJsonObject {
        put("role", "tool"); put("tool_call_id", callId); put("content", content)
    }

    /** `Plan`: the server refuses tools, so none are sent and the contract's plain line is shown. */
    suspend fun run(
        brain: BrowserToolBrain,
        messages: List<JsonObject>,
        executor: BrowserComputerExecutor,
        streamCall: BrowserStreamCall,
        callbacks: BrowserToolLoopCallbacks = BrowserToolLoopCallbacks(),
    ): BrowserToolLoopResult {
        if (brain == BrowserToolBrain.Plan) {
            streamCall(BrowserToolRequest(messages, null, onDelta = callbacks.onChunk))
            callbacks.onChunk("\n\n$BROWSER_PLAN_NO_TOOLS_LINE")
            return BrowserToolLoopResult(0, null)
        }
        val thread = messages.toMutableList()
        var rounds = 0
        while (true) {
            currentCoroutineContext().ensureActive()
            val last = rounds >= BrowserToolCaps.MAX_ROUNDS_PER_MESSAGE
            val response = streamCall(BrowserToolRequest(thread.toList(), BrowserToolDefinitions.all, toolChoiceNone = last, onDelta = callbacks.onChunk))
            // The last request is "no more tools": any call in it is ignored.
            if (last || response.toolCalls.isEmpty()) break
            rounds += 1
            thread += assistantToolCalls(response.toolCalls)
            // Parallel calls in one assistant message run in order, one at a time.
            for (call in response.toolCalls) {
                currentCoroutineContext().ensureActive()
                val input = ((runCatching { Json.parseToJsonElement(call.arguments) as? JsonObject }.getOrNull()) ?: JsonObject(emptyMap()))
                    .mapNotNull { (k, v) -> (v as? JsonPrimitive)?.takeIf { it.isString }?.let { k to it.content } }.toMap()
                callbacks.onToolCall(call.id, call.name, input)
                val result = runTool(executor, call.name, call.arguments)
                val content = fitResultMessage(result)
                val error = (result["error"] as? JsonPrimitive)?.contentOrNull
                if (error != null) callbacks.onToolError(call.id, call.name, error) else callbacks.onToolResult(call.id, call.name, content)
                thread += toolResult(call.id, content)
            }
        }
        val line = if (rounds > 0) "Used $rounds of ${BrowserToolCaps.MAX_ROUNDS_PER_MESSAGE} tool rounds" else null
        if (line != null) callbacks.onChunk("\n\n$line")
        return BrowserToolLoopResult(rounds, line)
    }
}
