/**
 * Shell Sandbox
 *
 * Wraps subprocess execution with platform-appropriate OS isolation:
 *   - Linux:  bubblewrap (bwrap) — same as gizzi-code
 *   - macOS:  sandbox-exec with a Seatbelt profile — same as gizzi-code
 *   - Windows: no-op (not supported)
 *
 * The wrapper gives the spawned process (default policy):
 *   - READ access: macOS — the whole host filesystem; Linux — only what is
 *     mounted: /usr, /etc, /opt, /bin, /sbin, /lib*, plus the declared read and
 *     write paths
 *   - WRITE access only to declared write paths (workdir, temp dirs and the
 *     usual tool caches by default)
 *   - NETWORK access configurable (default: denied — set GIZZI_SANDBOX_ALLOW_NETWORK
 *     or an explicit policy override for npm/cargo/etc.)
 *
 * `strict: true` (run_code: untrusted scripts) narrows that: reads limited to
 * system dirs + declared paths (macOS denies home, /Users, /Volumes, per-user
 * temp and Keychains), writes to workdir + declared paths only, network always
 * off, and on Linux a private /tmp plus PID/IPC namespaces and a new session.
 *
 * The agent's reasoning, tool dispatch, and API connections are completely
 * outside this boundary — only subprocesses spawned by the Bash tool are wrapped.
 */
import path from "path"
import os from "os"
import { realpathSync } from "fs"
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
  /**
   * Untrusted-code mode (run_code). Reads: system files plus cwd and the
   * declared read/write paths only — on macOS $HOME, /Users, /Volumes,
   * /private/var/folders and /Library/Keychains are denied. Writes: cwd and
   * allowWritePaths only (no shared temp or cache dirs; point TMPDIR/HOME into
   * cwd). Network is always off, whatever allowNetwork says. Linux also gets a
   * private /tmp, its own PID and IPC namespaces and a new session.
   */
  strict?: boolean
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
   * policy. Shared by bwrapArgs, seatbeltProfile, and isWriteAllowed so
   * the app-layer check (used by file-write tools) can never drift from what
   * the OS-level sandbox actually permits.
   */
  function writeAllowlist(cwd: string, policy: SandboxPolicy): string[] {
    if (policy.strict) return [...new Set([cwd, ...policy.allowWritePaths])]
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

  /** Exported for tests; `wrap` is the entry point. */
  export function bwrapArgs(
    command: string,
    shell: string,
    cwd: string,
    policy: SandboxPolicy,
    proxyHandle?: NetworkProxyHandle,
  ): WrappedCommand {
    const args: string[] = []
    const strict = policy.strict === true

    // Strict: a private, empty /tmp. Mounted first so binds of paths under
    // /tmp (e.g. a run folder or read path there) land on top of it.
    if (strict) args.push("--tmpfs", "/tmp")

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

    // Write access to workdir and user home area (strict: workdir + declared only)
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
    // Don't unshare PID — child tool calls like `git` need to see the parent shell.
    // Strict runs untrusted code that needs no view of host processes.
    if (strict) args.push("--unshare-pid", "--unshare-ipc", "--new-session")

    // ── Network ─────────────────────────────────────────────
    // Full block, or domain-allowlisted (proxy-only): both fully isolate the
    // network namespace. `--unshare-net` gives bwrap its own private loopback,
    // so the only way out is the bind-mounted proxy socket bridged by socat
    // below. Only a bare `allowNetwork: true` with no domain list skips this
    // and leaves networking untouched (host's namespace, unrestricted).
    let finalCommand = command
    if (strict || !policy.allowNetwork) {
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

  const q = (p: string) => `"${p.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`

  /** The path as given plus its real path (Seatbelt matches resolved paths: /var → /private/var). */
  function realVariants(p: string): string[] {
    const abs = path.resolve(p)
    let real = abs
    try {
      real = realpathSync(abs)
    } catch {
      if (abs === "/var" || abs.startsWith("/var/") || abs === "/tmp" || abs.startsWith("/tmp/")) real = `/private${abs}`
    }
    return real === abs ? [abs] : [abs, real]
  }

  /**
   * Strict (untrusted-code) Seatbelt profile. SBPL is last-match-wins:
   * reads are allowed broadly so the dyld cache, /System and /usr load, then
   * the user's data locations are denied, then the run folder and declared
   * paths are re-allowed. Metadata (stat/readlink, not directory listings)
   * stays readable so path resolution through denied parents still works.
   */
  function buildStrictSeatbeltProfile(cwd: string, policy: SandboxPolicy): string {
    const uniq = (list: string[]) => [...new Set(list.flatMap(realVariants))]
    const subpaths = (list: string[]) => list.map((p) => `  (subpath ${q(p)})`).join("\n")
    const denied = uniq([os.homedir(), "/Users", "/Volumes", "/private/var/folders", "/Library/Keychains"])
    const readable = uniq([cwd, ...policy.allowWritePaths, ...(policy.allowReadPaths ?? [])])
    const writable = uniq(writeAllowlist(cwd, policy))
    return `(version 1)
(deny default)

; ── Process execution ───────────────────────────────────────
(allow process-exec*)
(allow process-fork)
(allow process-info* (target self))
(allow signal (target self))
(allow signal (target children))

; ── IPC / Mach: only what the Python stack needs ────────────
; libinfo answers getpwuid (getpass.getuser, pwd); pandas + matplotlib
; (fonts are files under mpl-data and /System/Library/Fonts) need no others.
(allow ipc-posix-sem)
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
)

; ── System info ─────────────────────────────────────────────
(allow sysctl-read)
(allow system-socket)

; ── File reads: system yes, user data no, run folder yes ────
(allow file-read*)
(allow file-test-existence)
(deny file-read*
${subpaths(denied)}
)
(allow file-read-metadata)
(allow file-read*
${subpaths(readable)}
)

; ── File writes: run folder (+ declared) only ───────────────
(allow file-write*
${subpaths(writable)}
)
(allow file-write-data (literal "/dev/null"))

; ── Network: none (strict) ──────────────────────────────────
; ── Devices: ioctl on already-open fds; no IOKit ────────────
(allow file-ioctl)
`
  }

  /**
   * The Seatbelt profile text for a policy. Exported for tests; the default
   * (non-strict) text is unchanged from when it was written to a file.
   */
  export function seatbeltProfile(cwd: string, policy: SandboxPolicy, proxyHandle?: NetworkProxyHandle): string {
    if (policy.strict) return buildStrictSeatbeltProfile(cwd, policy)
    return buildSeatbeltProfile(writeAllowlist(cwd, policy), policy, proxyHandle)
  }

  /**
   * Session teardown. Profiles are passed inline (`sandbox-exec -p`), so there
   * is no profile file to delete any more (it used to live in writable tmp,
   * where a sandboxed process could rewrite the next command's profile); this
   * stops the session's network proxy.
   */
  export async function cleanupProfile(sessionID: string): Promise<void> {
    await NetworkProxy.stop(sessionID)
  }

  function sandboxExecArgs(
    command: string,
    shell: string,
    cwd: string,
    policy: SandboxPolicy,
    proxyHandle?: NetworkProxyHandle,
  ): WrappedCommand {
    const profile = seatbeltProfile(cwd, policy, proxyHandle)
    const finalCommand =
      proxyHandle?.mode === "tcp"
        ? `export HTTP_PROXY=http://127.0.0.1:${proxyHandle.port} HTTPS_PROXY=http://127.0.0.1:${proxyHandle.port}; ${command}`
        : command
    return {
      bin: "sandbox-exec",
      args: ["-p", profile, shell, "-c", finalCommand],
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
    if (!opts.policy.strict && opts.policy.allowNetwork && opts.policy.allowedDomains?.length) {
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

    // sandbox-exec (macOS) — the profile is passed inline with -p
    return sandboxExecArgs(opts.command, opts.shell, opts.cwd, opts.policy, proxyHandle)
  }
}
