/**
 * Apply a cloud code environment when a session starts (Contract C).
 *
 * A session created with `environmentId` gets, before its first agent turn:
 *
 *   connecting   resolve the environment (secret values included) from
 *                allternit-cloud-api with the runtime's device token
 *   setting_up   clone its repos into the workspace (one folder each), load
 *                its .env text and secrets into the tool-process environment,
 *                start the network-policy proxy, run its setup script with
 *                bash (10 min cap) — all shown live as a "Setup" step
 *   ready        the agent runs
 *   failed       the turn ends with the error instead of running the agent
 *
 * Applying it never throws past the session: every failure is caught,
 * recorded on the session as `failed` with a readable error, and only that
 * session's turn stops. Local sessions (no environment) skip all of this.
 *
 * After a server restart the in-memory part (variables, secrets, proxy) is
 * rebuilt on the next turn without re-cloning existing folders or re-running
 * the setup script.
 */
import path from "path"
import { spawn, type ChildProcess } from "child_process"
import { existsSync, mkdirSync, readdirSync, statSync } from "fs"
import { writeFile, unlink } from "fs/promises"
import os from "os"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Identifier } from "@/shared/id/id"
import { Bus } from "@/shared/bus"
import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"
import { Config } from "@/runtime/context/config/config"
import { ShellEnv } from "@/runtime/integrations/shell/env"
import { Shell } from "@/runtime/integrations/shell/shell"
import { ProcessRegistry } from "@/runtime/process-registry"
import { SessionSandbox } from "@/runtime/context/sandbox/session-sandbox"
import { SecretRedactor } from "@/runtime/session/environment/redactor"
import { DotEnv } from "@/runtime/session/environment/dotenv"
import { EgressProxy } from "@/runtime/session/environment/egress-proxy"
import { EnvironmentResolve } from "@/runtime/session/environment/resolve"
import { EnvironmentRuntime } from "@/runtime/session/environment/runtime-state"

const log = Log.create({ service: "session-environment" })

export namespace SessionEnvironment {
  /** The assistant message holding the Setup step; never sent to the model. */
  export const SETUP_MODE = "environment-setup"
  export const SETUP_TOOL = "setup"
  export const SETUP_TIMEOUT_MS = 10 * 60 * 1000
  const CLONE_TIMEOUT_MS = 10 * 60 * 1000
  const MAX_OUTPUT = 30_000

  export type Outcome =
    | { ok: true; /** Synthetic note for the model on the turn the environment came up. */ note?: string }
    | { ok: false; error: string; message?: MessageV2.WithParts }

  const inflight = new Map<string, Promise<Outcome>>()

  /** Variables a tool process of this session adds ({} for local sessions). */
  export function toolEnv(sessionID: string | undefined): Record<string, string> {
    return EnvironmentRuntime.toolEnv(sessionID)
  }

  /**
   * Make sure the session's environment is applied before the agent runs.
   * Safe to call on every turn: a no-op for local sessions and once ready.
   */
  export async function ensure(input: {
    session: Session.Info
    user: MessageV2.User
    workspace: string
    abort?: AbortSignal
  }): Promise<Outcome> {
    const env = input.session.environment
    if (!env) return { ok: true }
    const applied = EnvironmentRuntime.get(input.session.id)
    if (env.status === "ready" && applied?.environmentID === env.id) return { ok: true }
    const running = inflight.get(input.session.id)
    if (running) return running
    const task = apply(input)
      .catch((error): Outcome => {
        const message = error instanceof Error ? error.message : String(error)
        log.error("environment apply crashed", { sessionID: input.session.id, error: message })
        return { ok: false, error: SecretRedactor.text(message) }
      })
      .finally(() => inflight.delete(input.session.id))
    inflight.set(input.session.id, task)
    return task
  }

  function status(
    session: Session.Info,
    patch: Partial<NonNullable<Session.Info["environment"]>> & { status: NonNullable<Session.Info["environment"]>["status"] },
  ) {
    const current = session.environment!
    const next = { ...current, ...patch, at: Date.now() }
    if (patch.status !== "failed") delete next.error
    try {
      const info = Session.setEnvironment({ sessionID: session.id, environment: next })
      session.environment = info.environment
    } catch (error) {
      log.warn("recording environment status failed", { sessionID: session.id, error })
      session.environment = next
    }
  }

  // ------------------------------------------------------------ setup step UI

  type Step = {
    message: MessageV2.Assistant
    part: MessageV2.ToolPart
    output: string
    append(text: string): void
    flush(): Promise<void>
    finish(result: { ok: true; title: string } | { ok: false; error: string }): Promise<MessageV2.WithParts>
  }

  async function openStep(session: Session.Info, user: MessageV2.User, workspace: string, name: string): Promise<Step> {
    const now = Date.now()
    const message: MessageV2.Assistant = {
      id: Identifier.ascending("message"),
      sessionID: session.id,
      role: "assistant",
      parentID: user.id,
      modelID: user.model.modelID,
      providerID: user.model.providerID,
      mode: SETUP_MODE,
      agent: user.agent,
      path: { cwd: workspace, root: workspace },
      cost: 0,
      tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
      time: { created: now },
    }
    await Session.updateMessage(message)
    const title = name ? `Setup · ${name}` : "Setup"
    const part: MessageV2.ToolPart = {
      id: Identifier.ascending("part"),
      sessionID: session.id,
      messageID: message.id,
      type: "tool",
      callID: `env-setup-${message.id}`,
      tool: SETUP_TOOL,
      state: {
        status: "running",
        input: { environment: session.environment!.id, name },
        title,
        metadata: { output: "", description: title, environment: session.environment!.id },
        time: { start: now },
      },
    }
    let timer: ReturnType<typeof setTimeout> | undefined
    const publish = async () => {
      timer = undefined
      if (part.state.status !== "running") return
      part.state.metadata = { ...(part.state.metadata ?? {}), output: tail(step.output) }
      const saved = await Session.updatePart(part)
      Bus.publish(MessageV2.Event.PartUpdated, { part: saved })
    }
    const step: Step = {
      message,
      part,
      output: "",
      append(text) {
        step.output += SecretRedactor.text(text)
        if (step.output.length > MAX_OUTPUT * 2) step.output = step.output.slice(-MAX_OUTPUT)
        if (!timer) timer = setTimeout(() => void publish().catch(() => {}), 250)
      },
      async flush() {
        if (timer) clearTimeout(timer)
        await publish()
      },
      async finish(result) {
        if (timer) clearTimeout(timer)
        timer = undefined
        const end = Date.now()
        const output = tail(step.output)
        part.state = result.ok
          ? {
              status: "completed",
              input: part.state.input,
              title: result.title,
              output,
              metadata: { output, description: result.title, environment: session.environment!.id, exit: 0 },
              time: { start: part.state.status === "running" ? part.state.time.start : now, end },
            }
          : {
              status: "error",
              input: part.state.input,
              error: SecretRedactor.text((result as { error: string }).error),
              metadata: { output, description: title, environment: session.environment!.id },
              time: { start: part.state.status === "running" ? part.state.time.start : now, end },
            }
        const savedPart = await Session.updatePart(part)
        Bus.publish(MessageV2.Event.PartUpdated, { part: savedPart })
        message.time.completed = end
        if (!result.ok) {
          message.error = { name: "EnvironmentSetupError", message: SecretRedactor.text((result as { error: string }).error) }
          message.finish = "error"
        }
        const savedMessage = (await Session.updateMessage(message)) as MessageV2.Assistant
        return { info: savedMessage, parts: [savedPart] }
      },
    }
    const saved = await Session.updatePart(part)
    Bus.publish(MessageV2.Event.PartUpdated, { part: saved })
    return step
  }

  function tail(text: string) {
    return text.length > MAX_OUTPUT ? "…\n" + text.slice(-MAX_OUTPUT) : text
  }

  // ---------------------------------------------------------------- apply

  async function apply(input: {
    session: Session.Info
    user: MessageV2.User
    workspace: string
    abort?: AbortSignal
  }): Promise<Outcome> {
    const session = input.session
    const sessionID = session.id
    const env = session.environment!
    // Ready before a restart: rebuild the in-memory part quietly.
    const firstRun = env.status !== "ready"
    const workspace = input.workspace

    if (firstRun) status(session, { status: "connecting" })

    let resolved: EnvironmentResolve.Resolved
    try {
      resolved = await EnvironmentResolve.resolve(env.id, input.abort)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      status(session, { status: "failed", error: message })
      const step = await openStep(session, input.user, workspace, env.name).catch(() => undefined)
      const msg = await step?.finish({ ok: false, error: message }).catch(() => undefined)
      return { ok: false, error: message, message: msg }
    }

    // Secrets are redacted from here on, before anything can print them.
    const secrets: Record<string, string> = {}
    for (const [name, value] of Object.entries(resolved.secret_values ?? {})) {
      if (/^[A-Z_][A-Z0-9_]*$/.test(name) && typeof value === "string") secrets[name] = value
    }
    SecretRedactor.register(sessionID, secrets)

    const name = resolved.name || env.name
    const repos = env.repos && env.repos.length > 0 ? env.repos : resolved.repos
    if (firstRun) status(session, { status: "setting_up", name })

    const step = firstRun ? await openStep(session, input.user, workspace, name) : undefined
    const say = (text: string) => {
      if (step) step.append(text)
      else log.info("environment re-apply", { sessionID, text: SecretRedactor.text(text).slice(0, 500) })
    }

    let proxy: EgressProxy.Handle | undefined
    try {
      mkdirSync(workspace, { recursive: true })

      // ── Variables ─────────────────────────────────────────────
      const parsed = DotEnv.parse(resolved.env_vars)
      for (const bad of parsed.invalid) say(`! Skipped .env line ${bad.line}: not NAME=value\n`)
      const short = Object.entries(secrets).filter(([, v]) => v.length < SecretRedactor.MIN_LENGTH)
      for (const [secret] of short)
        say(`! Secret ${secret} is shorter than ${SecretRedactor.MIN_LENGTH} characters, so it can't be hidden from output.\n`)

      // ── Network policy ────────────────────────────────────────
      const secretHosts = resolved.secrets.flatMap((s) => s.hosts ?? [])
      const hosts = EgressProxy.normalize([...resolved.allowed_hosts, ...secretHosts])
      const network = resolved.network_access
      if (network !== "full") {
        proxy = await EgressProxy.start({ sessionID, mode: network === "none" ? "deny" : "allowlist", hosts })
      }
      const proxyEnv: Record<string, string> = proxy
        ? {
            HTTP_PROXY: proxy.url,
            HTTPS_PROXY: proxy.url,
            ALL_PROXY: proxy.url,
            http_proxy: proxy.url,
            https_proxy: proxy.url,
            all_proxy: proxy.url,
            NO_PROXY: "localhost,127.0.0.1,::1",
            no_proxy: "localhost,127.0.0.1,::1",
          }
        : {}
      const toolVars = { ...parsed.vars, ...secrets, ...proxyEnv }

      // ── Repos ─────────────────────────────────────────────────
      const dirs: string[] = []
      if (repos.length > 0) say(`$ Cloning ${repos.length} repo${repos.length === 1 ? "" : "s"} into ${workspace}\n`)
      for (const repo of repos) {
        const dir = await cloneRepo({ repo, workspace, secrets, say, abort: input.abort })
        dirs.push(dir)
      }

      applySandbox(sessionID, network, hosts, [workspace, ...dirs])

      await EnvironmentRuntime.set(sessionID, { environmentID: env.id, network, env: toolVars, proxy, workspace, dirs })

      // ── Setup script ──────────────────────────────────────────
      if (firstRun && resolved.setup_script.trim()) {
        say(`$ Running the setup script\n`)
        await runSetupScript({ sessionID, script: resolved.setup_script, workspace, vars: toolVars, say, abort: input.abort })
      }

      const policy =
        network === "full"
          ? "full network access"
          : network === "none"
            ? "no network access (tool processes)"
            : `network limited to ${hosts.length} allowed host${hosts.length === 1 ? "" : "s"}`
      say(`✓ Environment ready · ${policy}\n`)
      status(session, { status: "ready", name, dirs: dirs.map((d) => path.relative(workspace, d) || ".") })
      if (step) await step.finish({ ok: true, title: name ? `Setup · ${name}` : "Setup" })

      if (!firstRun) return { ok: true }
      const lines = [
        `<cloud_environment name="${escapeAttr(name)}">`,
        `This session runs in the cloud environment "${name}". It was set up before this turn:`,
        dirs.length
          ? `- Repos cloned into the workspace: ${dirs.map((d) => path.relative(workspace, d) || ".").join(", ")}`
          : `- No repos were cloned.`,
        resolved.setup_script.trim() ? `- The environment's setup script ran successfully.` : `- No setup script.`,
        Object.keys(secrets).length
          ? `- Secrets available as environment variables in shell commands: ${Object.keys(secrets).join(", ")} (values are hidden from output as [secret:NAME]).`
          : undefined,
        `- Network: ${policy}.`,
        `</cloud_environment>`,
      ].filter(Boolean)
      return { ok: true, note: lines.join("\n") }
    } catch (error) {
      const message = SecretRedactor.text(error instanceof Error ? error.message : String(error))
      log.warn("environment setup failed", { sessionID, error: message })
      // Keep secrets registered (output already printed stays hidden), drop the
      // rest: tools of a failed session get no half-applied environment.
      const state = EnvironmentRuntime.get(sessionID)
      if (proxy && state?.proxy !== proxy) await proxy.close().catch(() => {})
      await EnvironmentRuntime.set(sessionID, {
        environmentID: "",
        network: resolved.network_access,
        env: {},
        workspace,
        dirs: [],
      }).catch(() => {})
      status(session, { status: "failed", name, error: message })
      const msg = step ? await step.finish({ ok: false, error: message }).catch(() => undefined) : undefined
      return { ok: false, error: message, message: msg }
    }
  }

  function escapeAttr(value: string) {
    return value.replace(/[&"<>]/g, (c) => ({ "&": "&amp;", '"': "&quot;", "<": "&lt;", ">": "&gt;" })[c]!)
  }

  // ---------------------------------------------------------------- sandbox

  /**
   * When tool processes run sandboxed (the default), carry the environment's
   * network policy into the sandbox: `none` blocks the network, `limited`
   * allows only the allowlist through the sandbox proxy, `full` allows all.
   * An explicitly disabled sandbox stays disabled.
   */
  function applySandbox(sessionID: string, network: "full" | "limited" | "none", hosts: string[], writePaths: string[]) {
    if (Flag.GIZZI_SANDBOX_DISABLE) return
    const effective = SessionSandbox.getEffective(sessionID)
    if (effective && !effective.enabled) return
    const allowNetwork = network !== "none"
    // The sandbox proxy suffix-matches domains: `*.x` and `.x` become `x`;
    // `*` means unrestricted.
    const domains =
      network === "limited" && !hosts.includes("*")
        ? [...new Set(hosts.map((h) => h.replace(/^\*\./, "").replace(/^\./, "")))]
        : []
    const own = SessionSandbox.get(sessionID)
    if (own) {
      for (const p of writePaths) SessionSandbox.allowWritePath(sessionID, p)
      SessionSandbox.setNetwork(sessionID, allowNetwork)
      SessionSandbox.setAllowedDomains(sessionID, network === "limited" ? (domains.length ? domains : ["invalid."]) : [])
      return
    }
    SessionSandbox.enable(sessionID, {
      allowWritePaths: writePaths,
      allowNetwork,
      // An empty allowlist under `limited` must not fall back to "allow all".
      allowedDomains: network === "limited" ? (domains.length ? domains : ["invalid."]) : undefined,
    })
  }

  // ---------------------------------------------------------------- processes

  function run(input: {
    command: string
    args: string[]
    cwd: string
    env: Record<string, string>
    timeoutMs: number
    label: string
    say: (text: string) => void
    abort?: AbortSignal
  }): Promise<{ code: number | null; timedOut: boolean; aborted: boolean; output: string }> {
    return new Promise((resolve, reject) => {
      let proc: ChildProcess
      try {
        proc = spawn(input.command, input.args, {
          cwd: input.cwd,
          env: input.env,
          stdio: ["ignore", "pipe", "pipe"],
          detached: process.platform !== "win32",
        })
      } catch (error) {
        reject(error)
        return
      }
      ProcessRegistry.track(proc, { label: input.label, group: process.platform !== "win32" })
      let output = ""
      let exited = false
      let timedOut = false
      let aborted = false
      const onData = (chunk: Buffer) => {
        const text = chunk.toString()
        output = (output + text).slice(-8_000)
        input.say(text)
      }
      proc.stdout?.on("data", onData)
      proc.stderr?.on("data", onData)
      const kill = () => void Shell.killTree(proc, { exited: () => exited })
      const timer = setTimeout(() => {
        timedOut = true
        kill()
      }, input.timeoutMs)
      const onAbort = () => {
        aborted = true
        kill()
      }
      input.abort?.addEventListener("abort", onAbort, { once: true })
      proc.once("error", (error) => {
        clearTimeout(timer)
        input.abort?.removeEventListener("abort", onAbort)
        reject(error)
      })
      proc.once("close", (code) => {
        exited = true
        clearTimeout(timer)
        input.abort?.removeEventListener("abort", onAbort)
        resolve({ code, timedOut, aborted, output })
      })
    })
  }

  async function baseEnv(vars: Record<string, string>) {
    const passthrough = (await Config.get().catch(() => undefined))?.bash?.env_passthrough ?? []
    return ShellEnv.child(process.env, vars, passthrough)
  }

  function repoDirName(url: string) {
    const cleaned = url.trim().replace(/[?#].*$/, "").replace(/\/+$/, "")
    const last = cleaned.split(/[/:]/).pop() ?? "repo"
    const base = last.replace(/\.git$/i, "").replace(/[^A-Za-z0-9._-]/g, "-").replace(/^[.-]+/, "")
    return base || "repo"
  }

  function isGitDir(dir: string) {
    return existsSync(path.join(dir, ".git"))
  }

  function isEmptyDir(dir: string) {
    try {
      return statSync(dir).isDirectory() && readdirSync(dir).length === 0
    } catch {
      return false
    }
  }

  async function remoteOf(dir: string): Promise<string | undefined> {
    const result = await run({
      command: "git",
      args: ["-C", dir, "remote", "get-url", "origin"],
      cwd: dir,
      env: await baseEnv({}),
      timeoutMs: 15_000,
      label: "env-git-remote",
      say: () => {},
    }).catch(() => undefined)
    return result?.code === 0 ? result.output.trim() : undefined
  }

  /** Git auth for GitHub https URLs from the environment's own token secret, passed by env (not argv). */
  function gitAuthEnv(url: string, secrets: Record<string, string>): Record<string, string> {
    const token = secrets.GITHUB_TOKEN || secrets.GH_TOKEN || secrets.GITHUB_PAT
    if (!token || !/^https:\/\/([^/@]+@)?github\.com\//i.test(url)) return {}
    const basic = Buffer.from(`x-access-token:${token}`, "utf8").toString("base64")
    return {
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "http.https://github.com/.extraheader",
      GIT_CONFIG_VALUE_0: `AUTHORIZATION: basic ${basic}`,
    }
  }

  async function cloneRepo(input: {
    repo: { url: string; branch?: string }
    workspace: string
    secrets: Record<string, string>
    say: (text: string) => void
    abort?: AbortSignal
  }): Promise<string> {
    const { repo, workspace, say } = input
    const url = repo.url.trim()
    if (!url || url.startsWith("-")) throw new Error(`Repo URL "${url}" isn't a git URL.`)
    const branch = repo.branch?.trim() || undefined
    if (branch && branch.startsWith("-")) throw new Error(`Branch "${branch}" isn't a valid branch name.`)

    const base = repoDirName(url)
    let dir = path.join(workspace, base)
    for (let n = 2; existsSync(dir); n++) {
      if (isGitDir(dir)) {
        const remote = await remoteOf(dir)
        if (remote && sameRepo(remote, url)) {
          say(`· ${base}: already cloned, keeping it\n`)
          return dir
        }
      } else if (isEmptyDir(dir)) break
      dir = path.join(workspace, `${base}-${n}`)
    }

    const env = {
      ...(await baseEnv({})),
      ...gitAuthEnv(url, input.secrets),
      GIT_TERMINAL_PROMPT: "0",
      GIT_ASKPASS: "echo",
    }
    const clone = (args: string[]) =>
      run({
        command: "git",
        args,
        cwd: workspace,
        env,
        timeoutMs: CLONE_TIMEOUT_MS,
        label: "env-git-clone",
        say,
        abort: input.abort,
      })

    say(`$ git clone ${redactUrl(url)}${branch ? ` (branch ${branch})` : ""} → ${path.relative(workspace, dir)}\n`)
    let result = await clone(["clone", ...(branch ? ["--branch", branch] : []), "--", url, dir])
    if (result.code !== 0 && branch && !result.timedOut && !result.aborted && /not found in upstream|Remote branch .* not found/i.test(result.output)) {
      // A new branch: clone the default branch and create it.
      say(`· Branch ${branch} doesn't exist yet; creating it from the default branch\n`)
      result = await clone(["clone", "--", url, dir])
      if (result.code === 0) {
        const sw = await run({
          command: "git",
          args: ["-C", dir, "switch", "-c", branch],
          cwd: dir,
          env,
          timeoutMs: 30_000,
          label: "env-git-switch",
          say,
        })
        if (sw.code !== 0) throw new Error(`Couldn't create branch ${branch} in ${redactUrl(url)}.`)
      }
    }
    if (result.aborted) throw new Error("Setup was cancelled.")
    if (result.timedOut) throw new Error(`Cloning ${redactUrl(url)} took longer than 10 minutes and was stopped.`)
    if (result.code !== 0) {
      const hint = /could not read Username|Authentication failed|terminal prompts disabled|Repository not found/i.test(result.output)
        ? " The repo may be private: add a GITHUB_TOKEN secret to the environment, or sign this runtime's git in to GitHub."
        : ""
      throw new Error(`Couldn't clone ${redactUrl(url)} (git exited with ${result.code}).${hint}`)
    }
    return dir
  }

  function sameRepo(a: string, b: string) {
    const norm = (u: string) =>
      u
        .trim()
        .replace(/^https?:\/\/[^@/]+@/, "https://")
        .replace(/^git@([^:]+):/, "https://$1/")
        .replace(/\.git$/i, "")
        .replace(/\/+$/, "")
        .toLowerCase()
    return norm(a) === norm(b)
  }

  function redactUrl(url: string) {
    return url.replace(/^(https?:\/\/)[^@/]+@/i, "$1")
  }

  async function runSetupScript(input: {
    sessionID: string
    script: string
    workspace: string
    vars: Record<string, string>
    say: (text: string) => void
    abort?: AbortSignal
  }) {
    const bash = Bun.which("bash")
    if (!bash) throw new Error("The setup script needs bash, and this runtime doesn't have it installed.")
    const file = path.join(os.tmpdir(), `gizzi-env-setup-${input.sessionID}-${Date.now()}.sh`)
    await writeFile(file, input.script.replace(/\r\n?/g, "\n"), { mode: 0o700 })
    try {
      const env = { ...(await baseEnv(input.vars)), GIZZI_ENVIRONMENT_SETUP: "1", TERM: "dumb" }
      const result = await run({
        command: bash,
        args: [file],
        cwd: input.workspace,
        env,
        timeoutMs: SETUP_TIMEOUT_MS,
        label: "env-setup-script",
        say: input.say,
        abort: input.abort,
      })
      if (result.aborted) throw new Error("Setup was cancelled.")
      if (result.timedOut) throw new Error("The setup script ran longer than 10 minutes and was stopped.")
      if (result.code !== 0) throw new Error(`The setup script failed (exit code ${result.code}).`)
    } finally {
      await unlink(file).catch(() => {})
    }
  }
}
