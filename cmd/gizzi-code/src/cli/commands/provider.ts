/**
 * /provider — Provider management command
 *
 * Subcommands:
 *   gizzi provider list          — show all providers + status
 *   gizzi provider add           — interactive wizard to add any provider type
 *   gizzi provider add <id>      — add a specific known provider
 *   gizzi provider remove <id>   — remove a configured provider
 *   gizzi provider test <id>     — test a configured provider with a ping call
 */

import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { Instance } from "@/runtime/context/project/instance"
import { Provider } from "@/runtime/providers/provider"
import { Auth } from "@/runtime/integrations/auth"
import { Config } from "@/runtime/context/config/config"
import { Global } from "@/runtime/context/global"
import { UI } from "@/cli/ui"
import { Discovery } from "@/runtime/providers/discovery"
import { discoverSubprocessProviders } from "@/runtime/providers/discovery/subprocess"
import { discoverLocalProviders } from "@/runtime/providers/discovery/local"
import { Filesystem } from "@/shared/util/filesystem"
import path from "path"
import { EOL } from "os"

// ─── Auth type definitions for the wizard ────────────────────────────────────

const AUTH_TYPES = [
  {
    id: "api_key",
    label: "API Key",
    description: "Standard secret key (Allternit, OpenAI, Kimi, Groq, Together, …)",
    hint: "You'll be asked for the key. It's stored locally and never sent anywhere except the provider.",
  },
  {
    id: "none",
    label: "Local model (no auth)",
    description: "HTTP server running locally — Ollama, LM Studio, Jan, vLLM, llama.cpp",
    hint: "Just needs the local server URL. No key required.",
  },
  {
    id: "bearer",
    label: "Bearer / subscription token",
    description: "OAuth token, session token, or corporate SSO gateway",
    hint: "You'll be asked for the token. Sent as Authorization: Bearer on each request.",
  },
  {
    id: "subprocess",
    label: "CLI tool (already authenticated)",
    description: "Any CLI already logged in — claude, kimi, qwen, gemini, codex, llm, …",
    hint: "Just needs the command to run. The CLI handles its own auth.",
  },
] as const

// ─── Known provider catalog ───────────────────────────────────────────────────
// Lets the wizard pre-fill fields when the user picks a known provider name.
// To add a new known provider: append an entry here. That's all.

const KNOWN_PROVIDERS: Array<{
  id: string
  name: string
  auth_type: "api_key" | "none" | "bearer" | "subprocess"
  base_url?: string
  env?: string
  key_url?: string     // where to get the API key
  subprocess_cmd?: string
  models_hint?: string // example model IDs
}> = [
  // ── API key providers ────────────────────────────────────────────────────
  { id: "allternit",   name: "Allternit",    auth_type: "api_key", base_url: "https://api.allternit.com/v1",        env: "ALLTERNIT_API_KEY",   key_url: "https://platform.allternit.com/api-keys", models_hint: "see platform.allternit.com/models" },
  { id: "openai",      name: "OpenAI",       auth_type: "api_key", base_url: "https://api.openai.com/v1",           env: "OPENAI_API_KEY",      key_url: "https://platform.openai.com/api-keys",   models_hint: "gpt-5, gpt-5-mini, gpt-4o" },
  { id: "google",      name: "Google AI",    auth_type: "api_key", base_url: "https://generativelanguage.googleapis.com/v1beta", env: "GEMINI_API_KEY", key_url: "https://aistudio.google.com/app/apikey", models_hint: "gemini-2.5-pro" },
  { id: "moonshot",    name: "Kimi (Moonshot)", auth_type: "api_key", base_url: "https://api.moonshot.ai/v1",      env: "MOONSHOT_API_KEY",    key_url: "https://platform.moonshot.cn/console",   models_hint: "kimi-k2, moonshot-v1-128k" },
  { id: "qwen",        name: "Qwen (Alibaba)", auth_type: "api_key", base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1", env: "DASHSCOPE_API_KEY", key_url: "https://dashscope.console.aliyun.com", models_hint: "qwen-max, qwen3-235b-a22b" },
  { id: "groq",        name: "Groq",         auth_type: "api_key", base_url: "https://api.groq.com/openai/v1",     env: "GROQ_API_KEY",        key_url: "https://console.groq.com/keys",          models_hint: "llama-3.1-70b-versatile" },
  { id: "together",    name: "Together AI",  auth_type: "api_key", base_url: "https://api.together.xyz/v1",        env: "TOGETHER_API_KEY",    key_url: "https://api.together.ai/settings/api-keys", models_hint: "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo" },
  { id: "mistral",     name: "Mistral",      auth_type: "api_key", base_url: "https://api.mistral.ai/v1",          env: "MISTRAL_API_KEY",     key_url: "https://console.mistral.ai/api-keys",    models_hint: "mistral-large-latest" },
  { id: "cohere",      name: "Cohere",       auth_type: "api_key", base_url: "https://api.cohere.com/v1",          env: "COHERE_API_KEY",      key_url: "https://dashboard.cohere.com/api-keys",  models_hint: "command-r-plus" },
  { id: "perplexity",  name: "Perplexity",   auth_type: "api_key", base_url: "https://api.perplexity.ai",          env: "PERPLEXITY_API_KEY",  key_url: "https://www.perplexity.ai/settings/api",  models_hint: "sonar-pro" },
  { id: "deepseek",    name: "DeepSeek",     auth_type: "api_key", base_url: "https://api.deepseek.com/v1",        env: "DEEPSEEK_API_KEY",    key_url: "https://platform.deepseek.com/api-keys", models_hint: "deepseek-chat, deepseek-reasoner" },
  { id: "minimax",     name: "MiniMax",      auth_type: "api_key", base_url: "https://api.minimax.chat/v1",        env: "MINIMAX_API_KEY",     key_url: "https://platform.minimax.io",             models_hint: "abab6.5s-chat" },
  { id: "xai",         name: "xAI (Grok)",   auth_type: "api_key", base_url: "https://api.x.ai/v1",               env: "XAI_API_KEY",         key_url: "https://console.x.ai",                   models_hint: "grok-3" },
  // ── Local (no auth) ──────────────────────────────────────────────────────
  { id: "ollama",      name: "Ollama",       auth_type: "none",    base_url: "http://localhost:11434/v1",  models_hint: "llama3.2, qwen3.5:4b" },
  { id: "lmstudio",    name: "LM Studio",    auth_type: "none",    base_url: "http://localhost:1234/v1",   models_hint: "(auto-detected from server)" },
  { id: "jan",         name: "Jan",          auth_type: "none",    base_url: "http://localhost:1337/v1",   models_hint: "(auto-detected from server)" },
  { id: "vllm",        name: "vLLM",         auth_type: "none",    base_url: "http://localhost:8000/v1",   models_hint: "(auto-detected from server)" },
  { id: "llamacpp",    name: "llama.cpp server", auth_type: "none", base_url: "http://localhost:8080/v1", models_hint: "(server's loaded model)" },
  // ── CLI / subscription ────────────────────────────────────────────────────
  { id: "claude-cli",  name: "Claude CLI",   auth_type: "subprocess", subprocess_cmd: "claude -p",   models_hint: "claude-sonnet-5, claude-opus-5" },
  { id: "kimi-cli",    name: "Kimi CLI",     auth_type: "subprocess", subprocess_cmd: "kimi -p",     models_hint: "kimi-k3, kimi-k2.7-code" },
  { id: "qwen-cli",    name: "Qwen Code CLI",auth_type: "subprocess", subprocess_cmd: "qwen -p",     models_hint: "qwen-max, qwen-plus" },
  { id: "gemini-cli",  name: "Gemini CLI",   auth_type: "subprocess", subprocess_cmd: "gemini -p",   models_hint: "gemini-2.5-pro" },
  { id: "codex-cli",   name: "Codex CLI",    auth_type: "subprocess", subprocess_cmd: "codex",       models_hint: "codex-mini-latest, codex-latest" },
  { id: "opencode",    name: "OpenCode",     auth_type: "subprocess", subprocess_cmd: "opencode run", models_hint: "(per provider)" },
  { id: "droid",       name: "Droid (Factory)", auth_type: "subprocess", subprocess_cmd: "droid exec", models_hint: "(Factory default)" },
]

// ─── Helpers ──────────────────────────────────────────────────────────────────

async function selectFromList(prompt: string, items: string[]): Promise<string> {
  UI.println("")
  items.forEach((item, i) => {
    UI.println(`  ${UI.Style.TEXT_DIM}${i + 1}.${UI.Style.TEXT_NORMAL} ${item}`)
  })
  UI.println("")
  const raw = await UI.input(`${prompt} (1-${items.length}): `)
  const n = parseInt(raw, 10)
  if (isNaN(n) || n < 1 || n > items.length) {
    UI.println(UI.Style.TEXT_DANGER + "Invalid selection" + UI.Style.TEXT_NORMAL)
    return selectFromList(prompt, items)
  }
  return items[n - 1]
}

async function inputSecret(prompt: string): Promise<string> {
  // Use readline with muted echo for API keys/tokens
  const { createInterface } = await import("readline")
  const rl = createInterface({ input: process.stdin, output: process.stdout })
  return new Promise((resolve) => {
    process.stdout.write(prompt)
    ;(process.stdin as any).setRawMode?.(true)
    let value = ""
    process.stdin.resume()
    process.stdin.setEncoding("utf8")
    const onData = (ch: string) => {
      if (ch === "\n" || ch === "\r") {
        ;(process.stdin as any).setRawMode?.(false)
        process.stdin.removeListener("data", onData)
        rl.close()
        process.stdout.write(EOL)
        resolve(value)
      } else if (ch === "\u0003") {
        process.exit()
      } else if (ch === "\u007f" || ch === "\b") {
        value = value.slice(0, -1)
      } else {
        value += ch
        process.stdout.write("*")
      }
    }
    process.stdin.on("data", onData)
  })
}

function statusBadge(status: "ready" | "discovered" | "unconfigured" | "error"): string {
  switch (status) {
    case "ready":        return UI.Style.TEXT_SUCCESS_BOLD + "● ready" + UI.Style.TEXT_NORMAL
    case "discovered":   return UI.Style.TEXT_WARNING_BOLD + "○ discovered" + UI.Style.TEXT_NORMAL
    case "unconfigured": return UI.Style.TEXT_DIM        + "○ not configured" + UI.Style.TEXT_NORMAL
    case "error":        return UI.Style.TEXT_DANGER_BOLD + "✗ error" + UI.Style.TEXT_NORMAL
  }
}

function authBadge(authType: string): string {
  switch (authType) {
    case "api_key":    return UI.Style.TEXT_DIM + "[api key]" + UI.Style.TEXT_NORMAL
    case "none":       return UI.Style.TEXT_DIM + "[local]" + UI.Style.TEXT_NORMAL
    case "bearer":     return UI.Style.TEXT_DIM + "[bearer]" + UI.Style.TEXT_NORMAL
    case "subprocess": return UI.Style.TEXT_DIM + "[cli]" + UI.Style.TEXT_NORMAL
    default:           return UI.Style.TEXT_DIM + "[?]" + UI.Style.TEXT_NORMAL
  }
}

// ─── Wizard ───────────────────────────────────────────────────────────────────

async function runAddWizard(preselectedId?: string) {
  UI.println("")
  UI.println(UI.Style.TEXT_HIGHLIGHT_BOLD + "Add Provider" + UI.Style.TEXT_NORMAL)
  UI.println(UI.Style.TEXT_DIM + "Adds a new LLM provider to gizzi." + UI.Style.TEXT_NORMAL)

  let known: typeof KNOWN_PROVIDERS[number] | undefined

  if (preselectedId) {
    known = KNOWN_PROVIDERS.find((p) => p.id === preselectedId)
    if (!known) {
      UI.error(`Unknown provider ID: ${preselectedId}`)
      UI.println(UI.Style.TEXT_DIM + `Run 'gizzi provider list' to see available providers.` + UI.Style.TEXT_NORMAL)
      return
    }
  } else {
    // Step 1: pick from known list or custom
    UI.println("")
    UI.println(UI.Style.TEXT_NORMAL_BOLD + "Known providers:" + UI.Style.TEXT_NORMAL)
    const knownLabels = KNOWN_PROVIDERS.map((p) => `${p.name} ${authBadge(p.auth_type)}`)
    knownLabels.push("Custom (enter details manually)")
    const selected = await selectFromList("Select provider", knownLabels)
    const idx = knownLabels.indexOf(selected)
    known = idx < KNOWN_PROVIDERS.length ? KNOWN_PROVIDERS[idx] : undefined
  }

  // Step 2: determine auth type
  let auth_type: "api_key" | "none" | "bearer" | "subprocess" = known?.auth_type ?? "api_key"
  if (!known) {
    UI.println("")
    UI.println(UI.Style.TEXT_NORMAL_BOLD + "Authentication type:" + UI.Style.TEXT_NORMAL)
    const typeLabels = AUTH_TYPES.map((t) => `${t.label} — ${t.description}`)
    const selectedType = await selectFromList("Select auth type", typeLabels)
    auth_type = AUTH_TYPES[typeLabels.indexOf(selectedType)].id
  }

  const typeInfo = AUTH_TYPES.find((t) => t.id === auth_type)!
  UI.println("")
  UI.println(UI.Style.TEXT_DIM + typeInfo.hint + UI.Style.TEXT_NORMAL)

  // Step 3: collect fields based on auth type
  let providerId = known?.id ?? ""
  let name = known?.name ?? ""
  let base_url = known?.base_url ?? ""
  let api_key = ""
  let token = ""
  let subprocess_cmd = known?.subprocess_cmd ?? ""

  if (!providerId) {
    providerId = await UI.input("Provider ID (unique slug, e.g. my-ollama): ")
  }
  if (!name) {
    name = await UI.input(`Display name [${providerId}]: `) || providerId
  }

  switch (auth_type) {
    case "api_key": {
      if (known?.key_url) {
        UI.println(UI.Style.TEXT_DIM + `  Get your key at: ${known.key_url}` + UI.Style.TEXT_NORMAL)
      }
      if (!base_url) base_url = await UI.input("API base URL: ")
      api_key = await inputSecret("API key: ")
      if (!api_key) { UI.error("API key cannot be empty"); return }
      break
    }
    case "none": {
      if (!base_url) base_url = await UI.input("Local server URL [http://localhost:11434/v1]: ") || "http://localhost:11434/v1"
      break
    }
    case "bearer": {
      if (!base_url) base_url = await UI.input("API base URL: ")
      token = await inputSecret("Bearer token: ")
      if (!token) { UI.error("Token cannot be empty"); return }
      break
    }
    case "subprocess": {
      if (!subprocess_cmd) subprocess_cmd = await UI.input("CLI command (e.g. 'claude -p' or 'llm prompt'): ")
      if (!subprocess_cmd) { UI.error("CLI command cannot be empty"); return }
      break
    }
  }

  // Step 4: verify
  UI.println("")
  UI.println(UI.Style.TEXT_DIM + "Testing connection…" + UI.Style.TEXT_NORMAL)
  const testResult = await testProvider({ auth_type, base_url, api_key, token, subprocess_cmd, model: known?.models_hint?.split(",")[0]?.trim() ?? "test" })
  if (!testResult.ok) {
    UI.println(UI.Style.TEXT_WARNING + `Warning: ${testResult.error}` + UI.Style.TEXT_NORMAL)
    const proceed = await UI.input("Save anyway? (y/N): ")
    if (!proceed.toLowerCase().startsWith("y")) {
      UI.println("Cancelled.")
      return
    }
  } else {
    UI.println(UI.Style.TEXT_SUCCESS + "  ✓ Connection verified" + UI.Style.TEXT_NORMAL)
  }

  // Step 5: save
  await saveProvider({ providerId, name, auth_type, base_url, api_key, token, subprocess_cmd })

  UI.println("")
  UI.println(UI.Style.TEXT_SUCCESS_BOLD + `✓ Provider '${providerId}' added.` + UI.Style.TEXT_NORMAL)
  UI.println(UI.Style.TEXT_DIM + `Use it with: gizzi --model ${providerId}/<model>` + UI.Style.TEXT_NORMAL)
  if (known?.models_hint) {
    UI.println(UI.Style.TEXT_DIM + `Models: ${known.models_hint}` + UI.Style.TEXT_NORMAL)
  }
}

// ─── Test a provider with a minimal call ─────────────────────────────────────

async function testProvider(opts: {
  auth_type: string
  base_url?: string
  api_key?: string
  token?: string
  subprocess_cmd?: string
  model: string
}): Promise<{ ok: boolean; error?: string }> {
  try {
    if (opts.auth_type === "subprocess") {
      const cmd = opts.subprocess_cmd!
      const { spawn } = await import("child_process")
      const [bin, ...args] = cmd.split(" ")
      const child = spawn(bin, [...args, "Reply with: OK"], { stdio: ["pipe", "pipe", "pipe"] })
      const result = await new Promise<string>((resolve, reject) => {
        let out = ""
        child.stdout?.on("data", (d: Buffer) => (out += d.toString()))
        child.on("close", (code) => (code === 0 ? resolve(out) : reject(new Error(`exit ${code}`))))
        child.on("error", reject)
        setTimeout(() => { child.kill(); reject(new Error("timeout")) }, 15000)
      })
      return { ok: result.length > 0 }
    }

    if (opts.auth_type === "none" || opts.auth_type === "api_key" || opts.auth_type === "bearer") {
      const url = `${opts.base_url?.replace(/\/$/, "")}/models`
      const headers: Record<string, string> = { "Content-Type": "application/json" }
      if (opts.auth_type === "api_key" && opts.api_key) headers["Authorization"] = `Bearer ${opts.api_key}`
      if (opts.auth_type === "bearer" && opts.token) headers["Authorization"] = `Bearer ${opts.token}`
      const res = await fetch(url, { headers, signal: AbortSignal.timeout(5000) })
      if (res.status === 401 || res.status === 403) return { ok: false, error: "Authentication failed — check your key/token" }
      return { ok: res.ok }
    }

    return { ok: true }
  } catch (e: any) {
    return { ok: false, error: e?.message ?? "Connection failed" }
  }
}

// ─── Save provider to config + auth store ────────────────────────────────────

async function saveProvider(opts: {
  providerId: string
  name: string
  auth_type: string
  base_url?: string
  api_key?: string
  token?: string
  subprocess_cmd?: string
}) {
  const configPath = path.join(Global.Path.config, "config.json")
  const existing = await Filesystem.readJson<any>(configPath).catch(() => ({}))
  const providers = existing.provider ?? {}

  providers[opts.providerId] = {
    name: opts.name,
    auth_type: opts.auth_type,
    ...(opts.base_url ? { api: opts.base_url } : {}),
    ...(opts.subprocess_cmd ? { subprocess_cmd: opts.subprocess_cmd } : {}),
    ...(opts.token ? { token: opts.token } : {}),
  }

  await Filesystem.writeJson(configPath, { ...existing, provider: providers })

  // API keys go into the secure auth store, not plain config
  if (opts.auth_type === "api_key" && opts.api_key) {
    await Auth.set(opts.providerId, { type: "api", key: opts.api_key })
  }
}

// ─── List command ─────────────────────────────────────────────────────────────

async function runList() {
  await Instance.provide({
    directory: process.cwd(),
    async fn() {
      const [configured, discoveredSub, discoveredLocal] = await Promise.all([
        Provider.list(),
        discoverSubprocessProviders(),
        discoverLocalProviders(),
      ])

      UI.println("")
      UI.println(UI.Style.TEXT_HIGHLIGHT_BOLD + "Providers" + UI.Style.TEXT_NORMAL)

      // Configured (ready to use)
      const configuredIds = Object.keys(configured)
      if (configuredIds.length > 0) {
        UI.println("")
        UI.println(UI.Style.TEXT_NORMAL_BOLD + "Configured:" + UI.Style.TEXT_NORMAL)
        for (const id of configuredIds.sort()) {
          const p = configured[id]
          const atype = p.auth_type ?? (p.key ? "api_key" : "none")
          const modelCount = Object.keys(p.models).length
          UI.println(`  ${statusBadge("ready")}  ${UI.Style.TEXT_NORMAL_BOLD}${id}${UI.Style.TEXT_NORMAL}  ${authBadge(atype)}  ${UI.Style.TEXT_DIM}${modelCount} model(s)${UI.Style.TEXT_NORMAL}`)
        }
      }

      // Auto-discovered subprocess CLIs
      const newSub = discoveredSub.filter((d) => !configured[d.id])
      if (newSub.length > 0) {
        UI.println("")
        UI.println(UI.Style.TEXT_NORMAL_BOLD + "Discovered CLI tools (zero config):" + UI.Style.TEXT_NORMAL)
        for (const d of newSub) {
          UI.println(`  ${statusBadge("discovered")}  ${UI.Style.TEXT_NORMAL_BOLD}${d.id}${UI.Style.TEXT_NORMAL}  ${authBadge("subprocess")}  ${UI.Style.TEXT_DIM}${d.models.length} model(s) · ${d.subprocess_cmd}${UI.Style.TEXT_NORMAL}`)
        }
      }

      // Auto-discovered local servers
      const newLocal = discoveredLocal.filter((d) => !configured[d.id])
      if (newLocal.length > 0) {
        UI.println("")
        UI.println(UI.Style.TEXT_NORMAL_BOLD + "Discovered local servers (zero config):" + UI.Style.TEXT_NORMAL)
        for (const d of newLocal) {
          UI.println(`  ${statusBadge("discovered")}  ${UI.Style.TEXT_NORMAL_BOLD}${d.id}${UI.Style.TEXT_NORMAL}  ${authBadge("none")}  ${UI.Style.TEXT_DIM}${d.models.length} model(s) · ${d.base_url}${UI.Style.TEXT_NORMAL}`)
        }
      }

      // Known but not yet added
      const knownUnconfigured = KNOWN_PROVIDERS.filter(
        (k) => !configured[k.id] && !discoveredSub.find((d) => d.id === k.id) && !discoveredLocal.find((d) => d.id === k.id),
      )
      if (knownUnconfigured.length > 0) {
        UI.println("")
        UI.println(UI.Style.TEXT_DIM + "Not configured (run 'gizzi provider add <id>' to set up):" + UI.Style.TEXT_NORMAL)
        for (const k of knownUnconfigured) {
          UI.println(`  ${statusBadge("unconfigured")}  ${k.id}  ${authBadge(k.auth_type)}  ${UI.Style.TEXT_DIM}${k.name}${UI.Style.TEXT_NORMAL}`)
        }
      }

      UI.println("")
      UI.println(UI.Style.TEXT_DIM + "To add a provider:  gizzi provider add" + UI.Style.TEXT_NORMAL)
      UI.println(UI.Style.TEXT_DIM + "To add a known one: gizzi provider add allternit" + UI.Style.TEXT_NORMAL)
      UI.println(UI.Style.TEXT_DIM + "To test:            gizzi provider test <id>" + UI.Style.TEXT_NORMAL)
    },
  })
}

// ─── Test command ─────────────────────────────────────────────────────────────

async function runTest(providerId: string) {
  await Instance.provide({
    directory: process.cwd(),
    async fn() {
      const configured = await Provider.list()
      const provider = configured[providerId]
      if (!provider) {
        UI.error(`Provider '${providerId}' not found. Run 'gizzi provider list' to see available providers.`)
        return
      }
      UI.println(UI.Style.TEXT_DIM + `Testing ${providerId}…` + UI.Style.TEXT_NORMAL)
      const firstModel = Object.keys(provider.models)[0] ?? "test"
      const result = await testProvider({
        auth_type: provider.auth_type ?? "api_key",
        base_url: provider.models[firstModel]?.api?.url,
        api_key: provider.key,
        token: provider.token,
        subprocess_cmd: provider.subprocess_cmd,
        model: firstModel,
      })
      if (result.ok) {
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `✓ ${providerId} is working` + UI.Style.TEXT_NORMAL)
      } else {
        UI.println(UI.Style.TEXT_DANGER_BOLD + `✗ ${providerId} failed: ${result.error}` + UI.Style.TEXT_NORMAL)
      }
    },
  })
}

// ─── Remove command ───────────────────────────────────────────────────────────

async function runRemove(providerId: string) {
  await Instance.provide({
    directory: process.cwd(),
    async fn() {
      const configPath = path.join(Global.Path.config, "config.json")
      const existing = await Filesystem.readJson<any>(configPath).catch(() => ({}))
      const providers = existing.provider ?? {}
      if (!providers[providerId]) {
        UI.error(`Provider '${providerId}' not found in config.`)
        return
      }
      delete providers[providerId]
      await Filesystem.writeJson(configPath, { ...existing, provider: providers })
      await Auth.remove(providerId).catch((err) => {
        // Stale credentials may remain on disk; surface it so the user can
        // clean up rather than believing the removal was complete.
        UI.error(`Failed to remove stored credentials for '${providerId}': ${err instanceof Error ? err.message : String(err)}`)
      })
      UI.println(UI.Style.TEXT_SUCCESS + `✓ Provider '${providerId}' removed.` + UI.Style.TEXT_NORMAL)
    },
  })
}

// ─── Command export ───────────────────────────────────────────────────────────

export const ProviderCommand = cmd({
  command: "provider <subcommand>",
  describe: "Manage LLM providers — list, add, remove, test",
  builder: (yargs: Argv) =>
    yargs
      .command("list", "List all providers and their status", {}, async () => runList())
      .command(
        "add [id]",
        "Add a provider (interactive wizard, or specify a known provider ID)",
        (y) => y.positional("id", { type: "string", describe: "Known provider ID (optional)" }),
        async (args) => runAddWizard(args.id),
      )
      .command(
        "remove <id>",
        "Remove a configured provider",
        (y) => y.positional("id", { type: "string", describe: "Provider ID" }),
        async (args) => { if (args.id) await runRemove(args.id) },
      )
      .command(
        "test <id>",
        "Test a configured provider with a live ping",
        (y) => y.positional("id", { type: "string", describe: "Provider ID" }),
        async (args) => { if (args.id) await runTest(args.id) },
      )
      .demandCommand(1, "Specify a subcommand: list, add, remove, test"),
  handler: () => {},
})
