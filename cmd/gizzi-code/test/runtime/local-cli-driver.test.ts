/**
 * LocalCliDriver adapter coverage
 *
 * Ensures every CLI discovered by the subprocess provider scanner has a
 * deterministic adapter status in LocalCliDriver: either supported with a
 * concrete mode, or explicitly unsupported with a clear reason.
 */

import { describe, expect, test } from "bun:test"
import { getCliAdapterInfo } from "@/runtime/drivers/local-cli-driver"
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
