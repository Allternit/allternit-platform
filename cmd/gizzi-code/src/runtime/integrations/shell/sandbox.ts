/**
 * Shell Sandbox
 *
 * Wraps subprocess execution with platform-appropriate OS isolation:
 *   - Linux:  bubblewrap (bwrap) — same as gizzi-code
 *   - macOS:  sandbox-exec with a Seatbelt profile — same as gizzi-code
 *   - Windows: no-op (not supported)
 *
 * The wrapper gives the spawned process:
 *   - READ access to the entire host filesystem
 *   - WRITE access only to declared write paths (workdir + /tmp by default)
 *   - NETWORK access configurable (default: denied — set GIZZI_SANDBOX_ALLOW_NETWORK
 *     or an explicit policy override for npm/cargo/etc.)
 *
 * The agent's reasoning, tool dispatch, and API connections are completely
 * outside this boundary — only subprocesses spawned by the Bash tool are wrapped.
 */
import path from "path"
import os from "os"
import { writeFile, unlink } from "fs/promises"
import { Log } from "@/shared/util/log"
import { NetworkProxy, type NetworkProxyHandle } from "@/runtime/integrations/shell/network-proxy"

const log = Log.create({ service: "shell-sandbox" })

// ─────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────

export interface SandboxPolicy {
  /** Paths the subprocess may write to. Workdir + /tmp are always included. */
  allowWritePaths: string[]
  /** Allow outbound network. Default: false — deny unless explicitly opted in. */
  allowNetwork: boolean
  /**
   * When set (and allowNetwork is true), network is allowed ONLY to these
   * hostnames (suffix-matched, e.g. "npmjs.org" also matches
   * "registry.npmjs.org"), routed through a local allowlisting proxy instead
   * of a wholesale network allow. Undefined/empty means: if allowNetwork is
   * true, allow everything (unchanged prior behavior).
   */
  allowedDomains?: string[]
  /**
   * Extra paths the subprocess may READ but not write. Seatbelt already
   * allows reads everywhere; bwrap only sees what is bound, so these are
   * bound read-only (e.g. run_code's Python env under the gizzi data dir).
   */
  allowReadPaths?: string[]
}

export type SandboxDriver = "bwrap" | "sandbox-exec" | "none"

export interface WrappedCommand {
  /** Binary to spawn (bwrap, sandbox-exec, or the shell itself) */
  bin: string
  /** Argument list including the shell and the original command */
  args: string[]
}

// ─────────────────────────────────────────────────────────────
// Driver detection
// ─────────────────────────────────────────────────────────────

export namespace Sandbox {
  let _detected: SandboxDriver | undefined

  export function detect(): SandboxDriver {
    if (_detected !== undefined) return _detected

    if (process.platform === "win32") {
      _detected = "none"
      return _detected
    }

    if (process.platform === "darwin") {
      // sandbox-exec ships with every macOS install — always available
      _detected = "sandbox-exec"
      return _detected
    }

    // Linux: check for bwrap
    const bwrap = Bun.which("bwrap")
    _detected = bwrap ? "bwrap" : "none"
    if (_detected === "none") {
      log.warn("bwrap not found — sandbox disabled (install bubblewrap to enable)")
    }
    return _detected
  }

  // ─────────────────────────────────────────────────────────
  // Linux: bubblewrap
  // ─────────────────────────────────────────────────────────

  // ─────────────────────────────────────────────────────────
  // Shared write-path allowlist
  // ─────────────────────────────────────────────────────────

  /**
   * The full set of paths a sandboxed process may write to, given a cwd and
   * policy. Shared by bwrapArgs, writeSeatbeltProfile, and isWriteAllowed so
   * the app-layer check (used by file-write tools) can never drift from what
   * the OS-level sandbox actually permits.
   */
  function writeAllowlist(cwd: string, policy: SandboxPolicy): string[] {
    return [
      cwd,
      os.tmpdir(),
      "/var/tmp",
      "/private/tmp",           // macOS
      "/private/var/folders",   // macOS per-user temp area
      path.join(os.homedir(), ".cache"),
      path.join(os.homedir(), ".npm"),
      path.join(os.homedir(), ".cargo"),
      path.join(os.homedir(), ".pnpm-store"),
      path.join(os.homedir(), ".bun"),
      ...policy.allowWritePaths,
    ]
  }

  /**
   * App-layer check for tools (write/edit/multiedit/apply_patch) that write
   * files directly instead of spawning a shell command. This is NOT kernel
   * enforcement like bwrap/Seatbelt give the Bash tool -- Bun/Node's fs calls
   * never pass through bwrap/sandbox-exec -- it's a plain path check against
   * the same allowlist, enforced in application code.
   */
  export function isWriteAllowed(filePath: string, cwd: string, policy: SandboxPolicy): boolean {
    const resolved = path.resolve(filePath)
    return writeAllowlist(cwd, policy).some((allowed) => {
      const allowedResolved = path.resolve(allowed)
      return resolved === allowedResolved || resolved.startsWith(allowedResolved + path.sep)
    })
  }

  function bwrapArgs(
    command: string,
    shell: string,
    cwd: string,
    policy: SandboxPolicy,
    proxyHandle?: NetworkProxyHandle,
  ): WrappedCommand {
    const args: string[] = []

    // ── Filesystem binds ──────────────────────────────────
    // Bind the host's entire root read-only as the base layer,
    // then selectively override paths as read-write.
    // We bind real directories, skipping symlinks bwrap can't handle.
    const roBinds = ["/usr", "/etc", "/opt"]
    // /bin, /lib, /lib64 may be symlinks on modern distros — add conditionally
    for (const p of ["/bin", "/sbin", "/lib", "/lib64", "/lib32"]) {
      try {
        const stat = Bun.file(p)
        // Only bind if it exists as a real dir (not a symlink)
        roBinds.push(p)
      } catch {
        // skip
      }
    }

    for (const p of roBinds) {
      args.push("--ro-bind-try", p, p)
    }
    for (const p of policy.allowReadPaths ?? []) {
      args.push("--ro-bind-try", p, p)
    }

    // Write access to workdir and user home area
    const writePaths = writeAllowlist(cwd, policy)

    for (const p of writePaths) {
      args.push("--bind-try", p, p)
    }

    // ── Special filesystems ────────────────────────────────
    args.push("--proc", "/proc")
    args.push("--dev", "/dev")
    args.push("--tmpfs", "/run")

    // ── Working directory ─────────────────────────────────
    args.push("--chdir", cwd)

    // ── Process isolation ─────────────────────────────────
    args.push("--die-with-parent")
    // Don't unshare PID — child tool calls like `git` need to see the parent shell

    // ── Network ─────────────────────────────────────────────
    // Full block, or domain-allowlisted (proxy-only): both fully isolate the
    // network namespace. `--unshare-net` gives bwrap its own private loopback,
    // so the only way out is the bind-mounted proxy socket bridged by socat
    // below. Only a bare `allowNetwork: true` with no domain list skips this
    // and leaves networking untouched (host's namespace, unrestricted).
    let finalCommand = command
    if (!policy.allowNetwork) {
      args.push("--unshare-net")
    } else if (proxyHandle?.mode === "unix") {
      args.push("--unshare-net")
      args.push("--bind-try", proxyHandle.socketPath!, proxyHandle.socketPath!)
      finalCommand =
        `socat TCP-LISTEN:${NetworkProxy.LINUX_BRIDGE_PORT},bind=127.0.0.1,reuseaddr,fork ` +
        `UNIX-CONNECT:${proxyHandle.socketPath} >/dev/null 2>&1 & disown; ` +
        `export HTTP_PROXY=http://127.0.0.1:${NetworkProxy.LINUX_BRIDGE_PORT} ` +
        `HTTPS_PROXY=http://127.0.0.1:${NetworkProxy.LINUX_BRIDGE_PORT}; ` +
        command
    }

    // ── The actual command ────────────────────────────────
    args.push("--", shell, "-c", finalCommand)

    return { bin: "bwrap", args }
  }

  // ─────────────────────────────────────────────────────────
  // macOS: sandbox-exec + Seatbelt profile
  // ─────────────────────────────────────────────────────────

  function buildSeatbeltProfile(writePaths: string[], policy: SandboxPolicy, proxyHandle?: NetworkProxyHandle): string {
    const writeRules = writePaths
      .map((p) => `  (subpath "${p.replace(/"/g, '\\"')}")`)
      .join("\n")

    // Seatbelt doesn't isolate the network namespace, so a domain-allowlisted
    // policy is enforced by only allowing outbound to the local proxy's port
    // — direct connections everywhere else fall through to `(deny default)`.
    const networkRule = !policy.allowNetwork
      ? `; network blocked`
      : proxyHandle?.mode === "tcp"
        ? `(allow network-outbound (remote ip "localhost:${proxyHandle.port}"))`
        : `(allow network*)`

    return `(version 1)
(deny default)

; ── Process execution ───────────────────────────────────────
(allow process-exec*)
(allow process-fork)
(allow process-info*)
(allow signal (target self))
(allow signal (target children))

; ── IPC / Mach ──────────────────────────────────────────────
(allow ipc-posix*)
(allow mach-lookup)
(allow mach-priv-host-port)
(allow mach-task-name)

; ── System info ─────────────────────────────────────────────
(allow sysctl-read)
(allow system-socket)

; ── File reads: entire filesystem ───────────────────────────
(allow file-read*)
(allow file-test-existence)

; ── File writes: declared paths only ────────────────────────
(allow file-write*
${writeRules}
)

; ── Network ─────────────────────────────────────────────────
${networkRule}

; ── Devices / IOKit ─────────────────────────────────────────
(allow file-ioctl)
(allow iokit-open)
`
  }

  // Profile files are keyed by sessionID so concurrent sessions don't collide
  const profilePaths = new Map<string, string>()

  export async function writeSeatbeltProfile(
    sessionID: string,
    policy: SandboxPolicy,
    cwd: string,
    proxyHandle?: NetworkProxyHandle,
  ): Promise<string> {
    const existing = profilePaths.get(sessionID)
    if (existing) return existing

    const profile = buildSeatbeltProfile(writeAllowlist(cwd, policy), policy, proxyHandle)
    const profilePath = path.join(os.tmpdir(), `gizzi-sandbox-${sessionID}.sb`)
    await writeFile(profilePath, profile, "utf8")
    profilePaths.set(sessionID, profilePath)
    log.info("wrote sandbox profile", { sessionID, profilePath })
    return profilePath
  }

  export async function cleanupProfile(sessionID: string): Promise<void> {
    const p = profilePaths.get(sessionID)
    if (p) {
      profilePaths.delete(sessionID)
      await unlink(p).catch(() => {})
    }
    await NetworkProxy.stop(sessionID)
  }

  async function sandboxExecArgs(
    command: string,
    shell: string,
    cwd: string,
    sessionID: string,
    policy: SandboxPolicy,
    proxyHandle?: NetworkProxyHandle,
  ): Promise<WrappedCommand> {
    const profilePath = await writeSeatbeltProfile(sessionID, policy, cwd, proxyHandle)
    const finalCommand =
      proxyHandle?.mode === "tcp"
        ? `export HTTP_PROXY=http://127.0.0.1:${proxyHandle.port} HTTPS_PROXY=http://127.0.0.1:${proxyHandle.port}; ${command}`
        : command
    return {
      bin: "sandbox-exec",
      args: ["-f", profilePath, shell, "-c", finalCommand],
    }
  }

  // ─────────────────────────────────────────────────────────
  // Public: build the wrapped spawn args
  // ─────────────────────────────────────────────────────────

  export async function wrap(opts: {
    command: string
    shell: string
    cwd: string
    sessionID: string
    policy: SandboxPolicy
  }): Promise<WrappedCommand | null> {
    const driver = detect()

    if (driver === "none") {
      log.warn("sandbox requested but no driver available", { platform: process.platform })
      return null
    }

    log.info("wrapping command", { driver, sessionID: opts.sessionID, cwd: opts.cwd })

    let proxyHandle: NetworkProxyHandle | undefined
    if (opts.policy.allowNetwork && opts.policy.allowedDomains?.length) {
      if (driver === "bwrap" && !NetworkProxy.hasSocat()) {
        throw new Error(
          "Domain-allowlisted network access requires `socat` on Linux (bridges the sandbox's " +
            "isolated network namespace to the allowlisting proxy). Install it (e.g. `apt install socat`), " +
            "or drop allowedDomains to fall back to a full network block/allow.",
        )
      }
      proxyHandle = await NetworkProxy.start(opts.sessionID, opts.policy.allowedDomains)
    }

    if (driver === "bwrap") {
      return bwrapArgs(opts.command, opts.shell, opts.cwd, opts.policy, proxyHandle)
    }

    // sandbox-exec (macOS) — async because it writes a profile file
    return sandboxExecArgs(opts.command, opts.shell, opts.cwd, opts.sessionID, opts.policy, proxyHandle)
  }
}
