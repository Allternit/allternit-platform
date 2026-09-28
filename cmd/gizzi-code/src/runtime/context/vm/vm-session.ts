/**
 * VM Session Manager
 *
 * Manages per-gizzi-session VMs via the allternit-api /vm-session endpoints.
 * This implements the same model as gizzi-code cloud sessions:
 *
 *   Session start  → POST /vm-session   (provision a fresh VM, bootstrap tools + git clone)
 *   Bash tool call → POST /vm-session/:id/execute  (run inside the live VM)
 *   Session end    → DELETE /vm-session/:id  (destroy the VM)
 *
 * What the VM gets on boot:
 *   - Ubuntu 24.04-minimal base image
 *   - Full internet access (npm/pip/cargo/git all work)
 *   - git, curl, wget, build-essential, python3, Node 22, Bun, pnpm, Rust
 *   - The project workspace at /workspace (cloned from git or bind-mounted)
 *   - Project dependencies auto-installed (bun install / cargo fetch / pip install)
 *
 * Falls back to local (still OS-sandboxed, see sandbox.ts) execution when:
 *   - GIZZI_VM_SESSIONS is not set
 *   - GIZZI_VM_API_URL is not configured
 *   - Provisioning or exec throws (allternit-api fails closed — 503 — rather
 *     than silently running the command unsandboxed on the API host when no
 *     VM driver is available; see vm_session_routes.rs)
 */

import { Log } from "@/shared/util/log"
import { Flag } from "@/runtime/context/flag/flag"
import { Instance } from "@/runtime/context/project/instance"
import { Server } from "@/runtime/server/server"
import { $ } from "bun"
import os from "os"
import path from "path"

const log = Log.create({ service: "vm-session" })

export interface VmSessionState {
  /** Session ID returned by allternit-api */
  sessionId: string
  /** True when a real microVM is backing this session */
  vmBacked: boolean
  /** Host working directory (source) */
  workdir: string
  /** Path inside the VM where workspace lives — always /workspace */
  workspacePath: string
  /** True when workspace was git-cloned into the VM (vs bind-mounted) */
  gitCloned: boolean
  /** allternit-api base URL used for this session */
  apiUrl: string
  createdAt: Date
  /** Bootstrap log from VM provisioning */
  bootstrapLog: string
}

export interface VmExecResult {
  exitCode: number
  stdout: string
  stderr: string
  durationMs: number
  vmBacked: boolean
}

// In-memory map: gizziSessionID → VmSessionState
const sessions = new Map<string, VmSessionState>()

export namespace VmSession {
  /**
   * True when VM session mode is globally enabled via GIZZI_VM_SESSIONS=true
   * and the API URL is configured.
   */
  export function isEnabled(): boolean {
    return Flag.GIZZI_VM_SESSIONS && !!Flag.GIZZI_VM_API_URL
  }

  /**
   * Get the active VM session for a gizzi session, if any.
   */
  export function get(gizziSessionID: string): VmSessionState | null {
    return sessions.get(gizziSessionID) ?? null
  }

  /**
   * Provision a new VM for the given gizzi session.
   *
   * On first call:
   *   1. Detects git remote + branch from the project directory
   *   2. Reads the SSH key if present (for private repos)
   *   3. Posts to allternit-api which spawns the VM and runs the bootstrap:
   *      - Installs Node 22, Bun, pnpm, Python 3, Rust, build tools
   *      - Clones the repo at /workspace (if git remote found)
   *        OR bind-mounts the host dir via VirtioFS (if no remote / local)
   *      - Runs `bun install` / `cargo fetch` / `pip install` as needed
   *      - Sets up git config and SSH known_hosts
   *
   * Idempotent — returns existing state if a VM was already provisioned.
   */
  export async function provision(
    gizziSessionID: string,
    opts?: {
      workdir?: string
      env?: Record<string, string>
      networkEnabled?: boolean
      cpuCores?: number
      memoryMb?: number
    },
  ): Promise<VmSessionState> {
    const existing = sessions.get(gizziSessionID)
    if (existing) return existing

    const apiUrl = Flag.GIZZI_VM_API_URL!
    const workdir = opts?.workdir ?? Instance.directory

    log.info("Provisioning VM session", { gizziSessionID, workdir, apiUrl })

    // ── Detect git context ───────────────────────────────────────────────────
    const gitRemote = await detectGitRemote(workdir)
    const gitBranch = gitRemote ? await detectGitBranch(workdir) : undefined
    const sshKeyB64 = gitRemote ? await tryReadSshKey() : undefined

    log.info("Git context", { gitRemote: gitRemote ?? "none", gitBranch: gitBranch ?? "HEAD" })

    // ── Tool integration fields ──────────────────────────────────────────────
    // EXA_API_KEY: used by WebSearch tool (and agent bash scripts that call Exa API directly)
    const exaApiKey = process.env.EXA_API_KEY ?? undefined

    // GIZZI_SERVER_URL: the gizzi-code HTTP server URL, reachable from inside the VM.
    // Inside the VM, localhost is the VM itself — so we derive the host gateway IP.
    // Firecracker: host is typically 172.16.0.1; Apple VF: 192.168.64.1.
    // We use the actual bound server URL and swap localhost/127.0.0.1 for the gateway.
    const gizziServerUrl = detectHostGatewayUrl(Server.url())

    // GIZZI_CONFIG_DIR: gizzi config directory on the host.
    // Bind-mounted read-only at /gizzi-config inside the VM so agent scripts can
    // read hooks, settings, etc. Defaults to ~/.openclaw or ~/.gizzi.
    const configDir = Flag.GIZZI_CONFIG_DIR ?? detectGizziConfigDir()

    log.info("Tool integration fields", {
      exaApiKey: exaApiKey ? "set" : "not set",
      gizziServerUrl,
      configDir,
    })

    const body: Record<string, unknown> = {
      workdir,
      env: opts?.env ?? {},
      network_enabled: opts?.networkEnabled ?? true,
      cpu_cores: opts?.cpuCores,
      memory_mb: opts?.memoryMb,
      git_remote: gitRemote,
      git_branch: gitBranch,
      ssh_key_b64: sshKeyB64,
      exa_api_key: exaApiKey,
      gizzi_server_url: gizziServerUrl,
      config_dir: configDir,
      extra_env: {},
    }

    const url = apiUrl.endsWith("/") ? `${apiUrl}vm-session` : `${apiUrl}/vm-session`
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })

    if (!response.ok) {
      const text = await response.text().catch(() => response.statusText)
      throw new Error(`VM session provision failed (${response.status}): ${text}`)
    }

    const data = (await response.json()) as {
      session_id: string
      status: string
      workspace_path: string
      vm_backed: boolean
      git_cloned: boolean
      bootstrap_log: string
    }

    const state: VmSessionState = {
      sessionId: data.session_id,
      vmBacked: data.vm_backed,
      workdir,
      workspacePath: data.workspace_path,
      gitCloned: data.git_cloned,
      apiUrl,
      createdAt: new Date(),
      bootstrapLog: data.bootstrap_log,
    }

    sessions.set(gizziSessionID, state)

    log.info("VM session provisioned", {
      gizziSessionID,
      vmSessionId: state.sessionId,
      vmBacked: state.vmBacked,
      gitCloned: state.gitCloned,
      workspacePath: state.workspacePath,
    })

    return state
  }

  // ── Private helpers ──────────────────────────────────────────────────────────

  async function detectGitRemote(cwd: string): Promise<string | undefined> {
    try {
      const result = await $`git remote get-url origin`.cwd(cwd).quiet().nothrow().text()
      const remote = result.trim()
      return remote.length > 0 ? remote : undefined
    } catch {
      return undefined
    }
  }

  async function detectGitBranch(cwd: string): Promise<string | undefined> {
    try {
      const result = await $`git rev-parse --abbrev-ref HEAD`.cwd(cwd).quiet().nothrow().text()
      const branch = result.trim()
      return branch && branch !== "HEAD" ? branch : undefined
    } catch {
      return undefined
    }
  }

  async function tryReadSshKey(): Promise<string | undefined> {
    const home = process.env.HOME ?? process.env.USERPROFILE ?? ""
    const candidates = [
      `${home}/.ssh/id_ed25519`,
      `${home}/.ssh/id_rsa`,
      `${home}/.ssh/id_ecdsa`,
    ]
    for (const keyPath of candidates) {
      try {
        const { readFile } = await import("fs/promises")
        const data = await readFile(keyPath)
        // Only pass SSH keys for non-trivially sized files (i.e. real keys)
        if (data.length > 100) {
          return Buffer.from(data).toString("base64")
        }
      } catch {
        // file doesn't exist or no permission — try next
      }
    }
    return undefined
  }

  /**
   * Derive the host gateway URL that the VM can reach.
   *
   * Inside the VM, `localhost` resolves to the VM itself — not the host.
   * Firecracker VMs use 172.16.0.1 as the host gateway (mmds interface).
   * Apple Virtualization Framework uses 192.168.64.1.
   * When the server is listening on 0.0.0.0 / 127.0.0.1 we swap in the
   * most common gateway IP so the VM's bash commands can call back home.
   */
  function detectHostGatewayUrl(serverUrl: URL): string {
    const local = new Set(["localhost", "127.0.0.1", "0.0.0.0", "::1"])
    if (!local.has(serverUrl.hostname)) {
      // Server is already on a routable interface — use as-is
      return serverUrl.toString()
    }
    // Prefer an explicit env override, then fallback to Firecracker default
    const gateway =
      process.env.GIZZI_VM_HOST_GATEWAY ??
      (process.platform === "darwin" ? "192.168.64.1" : "172.16.0.1")
    const url = new URL(serverUrl.toString())
    url.hostname = gateway
    return url.toString()
  }

  /**
   * Detect the gizzi-code config directory on the host.
   * Checks standard locations used by openclaw / gizzi-code.
   */
  function detectGizziConfigDir(): string | undefined {
    const home = os.homedir()
    const candidates = [
      path.join(home, ".openclaw"),
      path.join(home, ".gizzi"),
      path.join(home, ".config", "gizzi-code"),
      path.join(home, ".config", "gizzi"),
      path.join(home, ".config", "openclaw"),
    ]
    for (const dir of candidates) {
      try {
        const stat = require("fs").statSync(dir)
        if (stat.isDirectory()) return dir
      } catch {
        // not found — try next
      }
    }
    return undefined
  }

  /**
   * Execute a shell command inside the VM session.
   * Throws if no VM session exists for gizziSessionID.
   */
  export async function exec(
    gizziSessionID: string,
    command: string,
    opts?: {
      env?: Record<string, string>
      timeoutSecs?: number
      workdir?: string
    },
    signal?: AbortSignal,
  ): Promise<VmExecResult> {
    const state = sessions.get(gizziSessionID)
    if (!state) {
      throw new Error(`No VM session for gizzi session ${gizziSessionID}`)
    }

    const body = {
      command,
      env: opts?.env ?? {},
      timeout_secs: opts?.timeoutSecs ?? 300,
      workdir: opts?.workdir,
    }

    const url = state.apiUrl.endsWith("/")
      ? `${state.apiUrl}vm-session/${state.sessionId}/execute`
      : `${state.apiUrl}/vm-session/${state.sessionId}/execute`

    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal,
    })

    if (!response.ok) {
      const text = await response.text().catch(() => response.statusText)
      throw new Error(`VM exec failed (${response.status}): ${text}`)
    }

    const result = (await response.json()) as {
      exit_code: number
      stdout: string
      stderr: string
      duration_ms: number
      vm_backed: boolean
    }

    return {
      exitCode: result.exit_code,
      stdout: result.stdout,
      stderr: result.stderr,
      durationMs: result.duration_ms,
      vmBacked: result.vm_backed,
    }
  }

  /**
   * Destroy the VM session and clean up.
   * Safe to call even if no VM session exists (no-op).
   */
  export async function destroy(gizziSessionID: string): Promise<void> {
    const state = sessions.get(gizziSessionID)
    if (!state) return

    sessions.delete(gizziSessionID)
    log.info("Destroying VM session", {
      gizziSessionID,
      vmSessionId: state.sessionId,
    })

    try {
      const url = state.apiUrl.endsWith("/")
        ? `${state.apiUrl}vm-session/${state.sessionId}`
        : `${state.apiUrl}/vm-session/${state.sessionId}`

      const response = await fetch(url, { method: "DELETE" })
      if (!response.ok) {
        const text = await response.text().catch(() => response.statusText)
        log.warn("VM session destroy returned error", {
          status: response.status,
          text,
          vmSessionId: state.sessionId,
        })
      }
    } catch (err) {
      log.warn("VM session destroy fetch failed", {
        error: err instanceof Error ? err.message : String(err),
        vmSessionId: state.sessionId,
      })
    }
  }
}
