/**
 * LocalCliDriver adapter coverage
 *
 * Ensures every CLI discovered by the subprocess provider scanner has a
 * deterministic adapter status in LocalCliDriver: either supported with a
 * concrete mode, or explicitly unsupported with a clear reason.
 */

import { describe, expect, test } from "bun:test"
import { getCliAdapterInfo, StreamJsonBlocks, streamJsonCutShort } from "@/runtime/drivers/local-cli-driver"
import { SUBPROCESS_PROVIDERS } from "@/runtime/providers/discovery/subprocess"

const EXPECTED_ACP_CLIS = [
  "kimi-cli",
  "mcode",
  "hermes",
  "grok",
  "kiro-cli",
  "qodercli",
  "qoderclicn",
  "qwenpaw",
  "reasonix",
  "traecli",
  "gemini-cli",
]

const EXPECTED_STREAM_JSON_CLIS = [
  "claude-cli",
  "codebuddy",
  "cursor-agent",
  "opencode",
  "deveco",
  "openclaw",
  "qwen-cli",
]

const EXPECTED_UNSUPPORTED = ["dsh", "copilot"]

describe("LocalCliDriver adapter registry", () => {
  test("every discovered subprocess CLI has a known adapter status", () => {
    for (const spec of SUBPROCESS_PROVIDERS) {
      const info = getCliAdapterInfo(spec.id)
      expect(info).toBeDefined()
      expect(typeof info.supported).toBe("boolean")
    }
  })

  test("ACP CLIs are supported with mode 'acp'", () => {
    for (const cli of EXPECTED_ACP_CLIS) {
      const info = getCliAdapterInfo(cli)
      expect(info.supported).toBe(true)
      expect(info.mode).toBe("acp")
    }
  })

  test("stream-json CLIs are supported with mode 'stream-json'", () => {
    for (const cli of EXPECTED_STREAM_JSON_CLIS) {
      const info = getCliAdapterInfo(cli)
      expect(info.supported).toBe(true)
      expect(info.mode).toBe("stream-json")
    }
  })

  test("Droid (Factory) runs headless via one-shot `droid exec`", () => {
    const info = getCliAdapterInfo("droid")
    expect(info.supported).toBe(true)
    expect(info.mode).toBe("one-shot-text")
  })

  test("unsupported CLIs report a clear reason", () => {
    for (const cli of EXPECTED_UNSUPPORTED) {
      const info = getCliAdapterInfo(cli)
      expect(info.supported).toBe(false)
      expect(info.reason).toContain("not implemented")
    }
  })

  test("there is no generic fallback for unknown CLIs", () => {
    const info = getCliAdapterInfo("definitely-not-a-real-cli")
    expect(info.supported).toBe(false)
  })

  test("supported adapters declare whether they support attachments", () => {
    for (const spec of SUBPROCESS_PROVIDERS) {
      const info = getCliAdapterInfo(spec.id)
      if (!info.supported) continue
      expect(typeof info.supportsAttachments).toBe("boolean")
    }
  })

  test("ACP adapters support attachments", () => {
    for (const cli of EXPECTED_ACP_CLIS) {
      const info = getCliAdapterInfo(cli)
      expect(info.supportsAttachments).toBe(true)
    }
  })

  test("non-ACP adapters do not claim attachment support", () => {
    const nonAcpSupported = SUBPROCESS_PROVIDERS.filter((spec) => {
      const info = getCliAdapterInfo(spec.id)
      return info.supported && !EXPECTED_ACP_CLIS.includes(spec.id)
    })
    expect(nonAcpSupported.length).toBeGreaterThan(0)
    for (const spec of nonAcpSupported) {
      const info = getCliAdapterInfo(spec.id)
      expect(info.supportsAttachments).toBe(false)
    }
  })
})

test("Codex app-server thread/start uses result.thread.id", async () => {
  const { codexThreadId } = await import("@/runtime/drivers/local-cli-driver")
  expect(codexThreadId({ thread: { id: "thr_123", sessionId: "thr_123" } })).toBe("thr_123")
  expect(codexThreadId({ threadId: "old-shape" })).toBeUndefined()
})

test("Codex discovery uses the installed CLI model catalog", async () => {
  const { parseCodexModels } = await import("@/runtime/providers/discovery/subprocess")
  expect(parseCodexModels(JSON.stringify({ models: [
    { slug: "gpt-6-astra", display_name: "GPT-6-Astra", context_window: 272000 },
    { slug: "gpt-5.6-sol", display_name: "GPT-5.6-Sol", context_window: 200000 },
  ] }))).toEqual([
    { id: "gpt-6-astra", name: "GPT-6-Astra", context: 272000, output: 32768 },
    { id: "gpt-5.6-sol", name: "GPT-5.6-Sol", context: 200000, output: 32768 },
  ])
})

describe("acpPermissionFor — ACP tool → gizzi permission mapping", () => {
  test("read-only tools map to the read permission", async () => {
    const { acpPermissionFor } = await import("@/runtime/drivers/local-cli-driver")
    expect(acpPermissionFor({ kind: "read_file", title: "Read /tmp/x" }).permission).toBe("read")
    expect(acpPermissionFor({ kind: "", title: "grep pattern src/" }).permission).toBe("read")
  })

  test("edit/write tools map to the edit permission (acceptEdits-allowable)", async () => {
    const { acpPermissionFor } = await import("@/runtime/drivers/local-cli-driver")
    expect(acpPermissionFor({ kind: "edit_file", title: "Edit foo.ts" }).permission).toBe("edit")
    expect(acpPermissionFor({ kind: "", title: "Write /tmp/cowork-proof.txt" }).permission).toBe("edit")
  })

  test("shell/command tools map to the bash permission (asked in default, denied in plan)", async () => {
    const { acpPermissionFor } = await import("@/runtime/drivers/local-cli-driver")
    expect(acpPermissionFor({ kind: "run_command", title: "npm test" }).permission).toBe("bash")
    expect(acpPermissionFor({ kind: "mystery", title: "" }).permission).toBe("bash")
  })

  test("patterns carry the tool title for display and approval binding", async () => {
    const { acpPermissionFor } = await import("@/runtime/drivers/local-cli-driver")
    const mapped = acpPermissionFor({ kind: "edit_file", title: "Write /tmp/proof.txt" })
    expect(mapped.pattern).toBe("Write /tmp/proof.txt")
  })
})

describe("acpStderrLooksFatal — quota/auth must not look like a successful empty turn", () => {
  test("matches kimi-cli 5-hour and monthly quota 403s", async () => {
    const { acpStderrLooksFatal } = await import("@/runtime/drivers/local-cli-driver")
    expect(
      acpStderrLooksFatal(
        "error: failed to run prompt: provider.auth_error: 403 You've reached your 5-hour usage limit.",
      ),
    ).toBe(true)
    expect(
      acpStderrLooksFatal(
        "provider.auth_error: 403 You've reached your monthly usage limit for this billing cycle.",
      ),
    ).toBe(true)
  })

  test("does not flag ordinary agent stderr", async () => {
    const { acpStderrLooksFatal } = await import("@/runtime/drivers/local-cli-driver")
    expect(acpStderrLooksFatal("")).toBe(false)
    expect(acpStderrLooksFatal("warn: deprecated config key max_retries_per_step")).toBe(false)
  })
})

describe("streamJsonUserContent — where stream-json tool results live", () => {
  test("reads Claude Code's message.content", async () => {
    const { streamJsonUserContent } = await import("../../src/runtime/drivers/local-cli-driver")
    const part = { type: "tool_result", tool_use_id: "toolu_1", content: "ok" }
    expect(streamJsonUserContent({ type: "user", message: { role: "user", content: [part] } })).toEqual([part])
  })
  test("still reads a top-level content array", async () => {
    const { streamJsonUserContent } = await import("../../src/runtime/drivers/local-cli-driver")
    const part = { type: "tool_result", tool_use_id: "toolu_2", content: "ok" }
    expect(streamJsonUserContent({ type: "user", content: [part] })).toEqual([part])
  })
  test("ignores other events", async () => {
    const { streamJsonUserContent } = await import("../../src/runtime/drivers/local-cli-driver")
    expect(streamJsonUserContent({ type: "assistant", message: { content: [] } })).toBeNull()
  })
})

describe("StreamJsonBlocks (Claude stream-json text bookkeeping)", () => {
  const text = (id: string, t: string) => ({ id, content: [{ type: "text", text: t }] })

  test("a later, shorter text block is not dropped", () => {
    const b = new StreamJsonBlocks()
    expect(b.assistantParts(text("m1", "alpha"))).toEqual(["alpha"])
    expect(b.assistantParts({ id: "m1", content: [{ type: "tool_use", name: "Bash" }] })).toEqual([""])
    expect(b.assistantParts(text("m2", "beta"))).toEqual(["beta"])
    expect(b.assistantParts(text("m2", "longer final reply"))).toEqual(["longer final reply"])
  })

  test("partial deltas are sent live and the final block adds nothing twice", () => {
    const b = new StreamJsonBlocks()
    b.streamEvent({ type: "message_start", message: { id: "m1" } })
    expect(b.streamEvent({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hel" } })).toEqual({ kind: "text", delta: "Hel" })
    expect(b.streamEvent({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "lo" } })).toEqual({ kind: "text", delta: "lo" })
    expect(b.streamEvent({ type: "content_block_delta", index: 1, delta: { type: "thinking_delta", thinking: "" } })).toBeNull()
    expect(b.assistantParts(text("m1", "Hello"))).toEqual([""])
  })

  test("a cumulative snapshot only sends what's new", () => {
    const b = new StreamJsonBlocks()
    expect(b.assistantParts({ id: "m1", content: [{ type: "text", text: "a" }, { type: "text", text: "b" }] })).toEqual(["a", "b"])
    expect(b.assistantParts({ id: "m1", content: [{ type: "text", text: "a" }, { type: "text", text: "bc" }] })).toEqual(["", "c"])
  })
})

describe("streamJsonCutShort — a turn with no result event did not finish", () => {
  const base = { cliName: "claude-cli", stderr: "" }

  test("Claude killed mid-turn (Desktop quit → SIGTERM) is an error, not a finished turn", () => {
    const why = streamJsonCutShort({ ...base, endsWithResult: true, exitCode: null, signalCode: "SIGTERM" })
    expect(why).toBe("claude-cli was stopped (SIGTERM) before finishing the turn")
  })

  test("Claude exiting cleanly without its result event is still cut short", () => {
    expect(streamJsonCutShort({ ...base, endsWithResult: true, exitCode: 0, signalCode: null })).toContain("before finishing")
  })

  test("a failed exit carries the stderr tail", () => {
    const why = streamJsonCutShort({ cliName: "opencode", endsWithResult: false, exitCode: 1, signalCode: null, stderr: "boom\n" })
    expect(why).toBe("opencode exited with code 1 before finishing the turn: boom")
  })

  test("dialects without a result event finish normally on a clean exit", () => {
    expect(streamJsonCutShort({ ...base, cliName: "cursor-agent", endsWithResult: false, exitCode: 0, signalCode: null })).toBeUndefined()
  })
})
