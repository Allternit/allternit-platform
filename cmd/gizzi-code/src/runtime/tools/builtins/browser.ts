/**
 * Browser Tool - Allternit Computer Use Bridge
 *
 * Thin wrapper that bridges GIZZI runtime to Allternit Computer Use service.
 * Does NOT implement browser automation logic - only transports requests
 * to the Python Computer Use gateway and normalizes responses.
 *
 * Architecture:
 *   GIZZI planner -> browser tool -> HTTP -> Computer Use gateway -> adapters
 *
 * Responsibilities:
 *   - Input schema validation
 *   - Permission/policy hook
 *   - HTTP transport to gateway
 *   - Result normalization back to GIZZI format
 *
 * Does NOT:
 *   - Choose between Playwright/browser-use/CDP (gateway does this)
 *   - Handle browser lifecycle (gateway does this)
 *   - Execute browser actions directly (adapters do this)
 */

import z from "zod/v4"
import { Config } from "@/runtime/context/config/config"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/browser.txt"
import { Log } from "@/shared/util/log"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { Provider } from "@/runtime/providers/provider"
import { spawn } from "child_process"
import { existsSync } from "fs"
import path from "path"
import { ProcessRegistry } from "@/runtime/process-registry"

const log = Log.create({ service: "browser-tool" })

// ============================================================================
// Configuration
// ============================================================================

const GATEWAY_URL =
  process.env.ALLTERNIT_COMPUTER_USE_URL ??
  process.env.Allternit_COMPUTER_USE_URL ??
  "http://127.0.0.1:8760"
const GATEWAY_TOKEN = process.env.ALLTERNIT_COMPUTER_USE_TOKEN ?? process.env.Allternit_COMPUTER_USE_TOKEN

// ============================================================================
// Operator Auto-Start
// ============================================================================

let _operatorProc: ReturnType<typeof spawn> | null = null

function findOperatorDir(): string | null {
  if (process.env.ALLTERNIT_COMPUTER_USE_ENGINE_PATH) return process.env.ALLTERNIT_COMPUTER_USE_ENGINE_PATH
  if (process.env.Allternit_OPERATOR_PATH) return process.env.Allternit_OPERATOR_PATH
  // Walk up from __dirname to find the monorepo root, then look for the ACU engine
  const candidates = [
    path.join(__dirname, "../../../../../../domains/computer-use/core"),
    path.join(__dirname, "../../../../../domains/computer-use/core"),
    path.join(__dirname, "../../../../domains/computer-use/core"),
    path.join(process.cwd(), "domains/computer-use/core"),
  ]
  return candidates.find(existsSync) ?? null
}

async function waitForGateway(timeoutMs = 8000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`${GATEWAY_URL}/health`, {
        signal: AbortSignal.timeout(1000),
      })
      if (r.ok) return true
    } catch {
      // Operator down while polling — the retry loop handles refusal/timeouts.
    }
    await new Promise((r) => setTimeout(r, 500))
  }
  return false
}

async function autoStartOperator(): Promise<boolean> {
  // Already running?
  try {
    const r = await fetch(`${GATEWAY_URL}/health`, {
      signal: AbortSignal.timeout(1500),
    })
    if (r.ok) return true
  } catch {
    // Not running yet (or unreachable) — fall through and start it.
  }

  const operatorDir = findOperatorDir()
  if (!operatorDir) {
    log.warn("Cannot auto-start operator: service directory not found")
    return false
  }

  log.info("Auto-starting ACU computer-use engine", { operatorDir })

  _operatorProc = spawn(
    "python3",
    ["-m", "uvicorn", "gateway.main:app", "--host", "127.0.0.1", "--port", "8760", "--log-level", "warning"],
    {
      cwd: operatorDir,
      env: { ...process.env },
      stdio: "ignore",
      detached: process.platform !== "win32",
    },
  )
  ProcessRegistry.track(_operatorProc, { label: "computer-use-gateway", group: process.platform !== "win32" })

  const ready = await waitForGateway(10000)
  if (ready) {
    log.info("ACU computer-use engine started successfully")
  } else {
    log.warn("ACU computer-use engine did not become ready within 10s")
  }
  return ready
}

// ============================================================================
// Types - Aligned with Allternit Computer Use ResultEnvelope
// ============================================================================

const BrowserAction = z.enum([
  "execute",   // LLM-powered automation via browser-use
  "goto",      // Navigate to URL
  "click",     // Click element
  "fill",      // Fill input field
  "extract",   // Extract structured data
  "screenshot", // Capture screenshot
  "inspect",   // Get page structure
])

const ArtifactSchema = z.object({
  type: z.enum(["screenshot", "download", "json", "text", "html"]),
  path: z.string().optional(),
  url: z.string().optional(),
  mime: z.string().optional(),
  content: z.string().optional(),
})

const ReceiptSchema = z.object({
  action: z.string(),
  timestamp: z.string(),
  success: z.boolean(),
  details: z.record(z.string(), z.any()).optional(),
})

// Computer Use gateway response envelope
const ComputerUseResponse = z.object({
  run_id: z.string(),
  session_id: z.string(),
  adapter_id: z.string(),
  family: z.enum(["browser", "desktop", "retrieval", "hybrid"]),
  mode: z.enum(["assist", "execute", "inspect", "parallel", "desktop", "hybrid", "crawl"]),
  status: z.enum(["pending", "running", "completed", "failed", "cancelled"]),
  summary: z.string().nullish(),
  extracted_content: z.any().nullish(),
  artifacts: z.array(ArtifactSchema).default([]),
  receipts: z.array(ReceiptSchema).default([]),
  error: z.object({
    code: z.string(),
    message: z.string(),
  }).nullish(),
  trace_id: z.string().nullish(),
})

// ============================================================================
// Gateway Client
// ============================================================================

type LLMConfig =
  | { auth_type: "api_key"; base_url: string; api_key: string; model: string }
  | { auth_type: "none"; base_url: string; model: string }
  | { auth_type: "bearer"; base_url: string; token: string; model: string }
  | { auth_type: "subprocess"; subprocess_cmd: string; model: string }

async function callComputerUseGateway(
  request: {
    action: string
    session_id: string
    run_id: string
    target?: string
    goal?: string
    parameters?: Record<string, any>
    adapter_preference?: string
    llm_config?: LLMConfig
  }
): Promise<z.infer<typeof ComputerUseResponse>> {
  const url = `${GATEWAY_URL}/v1/execute`

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
  }

  if (GATEWAY_TOKEN) {
    headers["Authorization"] = `Bearer ${GATEWAY_TOKEN}`
  }

  log.info("Calling Computer Use gateway", {
    action: request.action,
    session_id: request.session_id,
    run_id: request.run_id,
  })

  let response: Response
  try {
    response = await fetch(url, {
      method: "POST",
      headers,
      body: JSON.stringify(request),
    })
  } catch (err: any) {
    log.warn("Computer Use gateway unreachable, attempting auto-start", { url, error: err?.message })
    const started = await autoStartOperator()
    if (started) {
      // Retry once now that the operator is running
      try {
        response = await fetch(url, { method: "POST", headers, body: JSON.stringify(request) })
      } catch (retryErr: any) {
        return {
          run_id: request.run_id,
          session_id: request.session_id,
          adapter_id: "unavailable",
          family: "browser",
          mode: "execute",
          status: "failed",
          summary: "Allternit Operator started but request still failed",
          extracted_content: null,
          artifacts: [],
          receipts: [],
          error: {
            code: "GATEWAY_RETRY_FAILED",
            message: retryErr?.message ?? "Unknown error after retry",
          },
          trace_id: crypto.randomUUID(),
        }
      }
    } else {
      return {
        run_id: request.run_id,
        session_id: request.session_id,
        adapter_id: "unavailable",
        family: "browser",
        mode: "execute",
        status: "failed",
        summary: "ACU computer-use engine is not running and could not be auto-started",
        extracted_content: null,
        artifacts: [],
        receipts: [],
        error: {
          code: "GATEWAY_UNREACHABLE",
          message: `Cannot connect to ACU computer-use engine at ${GATEWAY_URL}. Start manually: cd domains/computer-use/core && python3 -m uvicorn gateway.main:app --port 8760`,
        },
        trace_id: crypto.randomUUID(),
      }
    }
  }

  if (!response.ok) {
    const errorText = await response.text()
    log.error("Computer Use gateway error", { status: response.status, error: errorText })
    throw new Error(`Computer Use gateway error ${response.status}: ${errorText}`)
  }

  const data = await response.json()
  return ComputerUseResponse.parse(data)
}

// ============================================================================
// Result Normalization
// ============================================================================

function normalizeResult(
  envelope: z.infer<typeof ComputerUseResponse>
): {
  title: string
  output: string
  metadata: Record<string, any>
  attachments?: Omit<MessageV2.FilePart, "id" | "sessionID" | "messageID">[]
} {
  // Build text output summary
  let output = envelope.summary ?? ""

  if (envelope.status === "failed" && envelope.error) {
    output += `\n\nError: ${envelope.error.code} - ${envelope.error.message}`
  }

  // Include extracted content if present
  if (envelope.extracted_content != null) {
    if (typeof envelope.extracted_content === "string") {
      output += `\n\nExtracted content:\n${envelope.extracted_content}`
    } else {
      output += `\n\nExtracted content:\n${JSON.stringify(envelope.extracted_content, null, 2)}`
    }
  }

  // Convert artifacts to GIZZI attachments
  const attachments: Omit<MessageV2.FilePart, "id" | "sessionID" | "messageID">[] = []

  for (const artifact of envelope.artifacts) {
    if (artifact.type === "screenshot" && artifact.url) {
      attachments.push({
        type: "file",
        mime: artifact.mime || "image/png",
        url: artifact.url,
      })
    } else if (artifact.type === "download" && artifact.url) {
      attachments.push({
        type: "file",
        mime: artifact.mime || "application/octet-stream",
        url: artifact.url,
      })
    }
  }

  // Build metadata for GIZZI context
  const metadata: Record<string, any> = {
    session_id: envelope.session_id,
    run_id: envelope.run_id,
    adapter_id: envelope.adapter_id,
    family: envelope.family,
    mode: envelope.mode,
    status: envelope.status,
    trace_id: envelope.trace_id,
    artifact_count: envelope.artifacts.length,
    receipt_count: envelope.receipts.length,
  }

  // Include receipts for audit trail
  if (envelope.receipts.length > 0) {
    metadata.receipts = envelope.receipts.map(r => ({
      action: r.action,
      timestamp: r.timestamp,
      success: r.success,
    }))
  }

  const title = envelope.status === "completed"
    ? `Browser ${envelope.mode} completed via ${envelope.adapter_id}`
    : `Browser ${envelope.mode} ${envelope.status}`

  const trimmed = output.trim()
  return {
    title,
    output: trimmed || `Browser ${envelope.mode} ${envelope.status}`,
    metadata,
    attachments: attachments.length > 0 ? attachments : undefined,
  }
}

// ============================================================================
// Tool Definition
// ============================================================================

export const BrowserTool = Tool.define("browser", async (initCtx) => {
  // Resolve the active provider's connection details at init time so every
  // gateway request carries the correct LLM config without the operator
  // needing its own hardcoded credentials.
  //
  // Supports all four auth modes:
  //   api_key    — standard providers (Anthropic, OpenAI, Kimi, Groq, …)
  //   none       — local models with no auth (Ollama, LM Studio, vLLM)
  //   bearer     — subscription / OAuth token providers
  //   subprocess — CLI tools already authed in the OS (claude, llm, aichat)
  let llmConfig: LLMConfig | undefined

  const agentModel = initCtx?.agent?.model
  if (agentModel?.providerID && agentModel?.modelID) {
    try {
      const [model, provider] = await Promise.all([
        Provider.getModel(agentModel.providerID, agentModel.modelID),
        Provider.getProvider(agentModel.providerID),
      ])
      const modelId = model.api.id ?? agentModel.modelID
      const authType = provider.auth_type ?? "api_key"

      if (authType === "subprocess" && provider.subprocess_cmd) {
        llmConfig = {
          auth_type: "subprocess",
          subprocess_cmd: provider.subprocess_cmd,
          model: modelId,
        }
      } else if (authType === "bearer" && (provider.token ?? provider.key) && model.api.url) {
        llmConfig = {
          auth_type: "bearer",
          base_url: model.api.url,
          token: provider.token ?? provider.key!,
          model: modelId,
        }
      } else if (authType === "none" && model.api.url) {
        llmConfig = {
          auth_type: "none",
          base_url: model.api.url,
          model: modelId,
        }
      } else if (model.api.url && provider.key) {
        // api_key (default) — standard providers
        llmConfig = {
          auth_type: "api_key",
          base_url: model.api.url,
          api_key: provider.key,
          model: modelId,
        }
      }
    } catch {
      // Provider lookup failed — operator will use its own configured inference endpoint
    }
  }

  return {
    description: DESCRIPTION,
    parameters: z.object({
      action: BrowserAction.describe(
        "The browser action to perform: execute (LLM-powered), goto, click, fill, extract, screenshot, inspect"
      ),
      target: z
        .string()
        .optional()
        .describe("URL for goto, CSS selector for click/fill, or query for extract"),
      goal: z
        .string()
        .optional()
        .describe("High-level goal for 'execute' action - describe what you want to accomplish"),
      text: z
        .string()
        .optional()
        .describe("Text to fill for 'fill' action"),
      parameters: z
        .record(z.string(), z.unknown())
        .optional()
        .describe("Additional action-specific parameters (timeout, options, etc.)"),
      adapter_preference: z
        .enum(["extension-tab", "local-playwright", "browser-use", "stagehand", "playwright", "cdp", "desktop"])
        .optional()
        .describe("Optional provider preference. Prefer canonical providers: extension-tab, local-playwright, browser-use, stagehand. Legacy aliases are accepted."),
    }),
    async execute(params: { action: string; target?: string; goal?: string; text?: string; parameters?: Record<string, unknown>; adapter_preference?: string }, ctx) {
      if (params.action === "execute" && !params.goal) {
        throw new Error("The 'execute' action requires a 'goal' parameter describing what to accomplish")
      }
      if (["goto", "click", "fill"].includes(params.action) && !params.target) {
        throw new Error(`The '${params.action}' action requires a 'target' parameter`)
      }
      if (params.action === "fill" && !params.text) {
        throw new Error("The 'fill' action requires a 'text' parameter")
      }

      const actionDescription = params.action === "execute"
        ? `Execute browser automation: ${params.goal}`
        : `Browser ${params.action}: ${params.target || ""}`

      await ctx.ask({
        permission: "browser",
        patterns: params.target ? [params.target] : [actionDescription],
        always: ["browser *"],
        metadata: {
          action: params.action,
          target: params.target,
          goal: params.goal,
          adapter_preference: params.adapter_preference,
        },
      })

      // No adapter named: use the account default (Settings → Cowork →
      // Preferred browser, synced by the platform API into global config).
      const adapterPreference =
        params.adapter_preference ?? (await Config.get().catch(() => undefined))?.browser?.preferred_adapter

      const envelope = await callComputerUseGateway({
        action: params.action,
        session_id: ctx.sessionID,
        run_id: ctx.sessionID,
        target: params.target,
        goal: params.goal,
        parameters: {
          ...params.parameters,
          text: params.text,
          message_id: ctx.messageID,
          call_id: ctx.callID,
        },
        adapter_preference: normalizeProviderPreference(adapterPreference),
        llm_config: llmConfig,
      })

      return normalizeResult(envelope)
    },
  }
})

function normalizeProviderPreference(preference?: string): string | undefined {
  if (!preference) return undefined
  // ACU gateway accepts: playwright | browser-use | cdp | desktop
  if (preference === "local-playwright") return "playwright"
  if (preference === "extension-tab") return "desktop"
  if (preference === "stagehand") return "browser-use"
  return preference
}
