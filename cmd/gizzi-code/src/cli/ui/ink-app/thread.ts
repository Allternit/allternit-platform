import { cmd } from "@/cli/commands/cmd"
import { tui } from "@/cli/ui/ink-app/app"
import { Rpc } from "@/shared/util/rpc"
import { type rpc } from "@/cli/ui/ink-app/worker"
import path from "path"
import { fileURLToPath } from "url"
import { iife } from "@/shared/util/iife"
import { Log } from "@/shared/util/log"
import { withNetworkOptions, resolveNetworkOptions } from "@/cli/network"
import { Filesystem } from "@/shared/util/filesystem"
import { win32DisableProcessedInput, win32InstallCtrlCGuard } from "@/cli/ui/ink-app/win32"
import { getSessionId, setOriginalCwd, setProjectRoot } from "@/cli/ui/ink-app/bootstrap/state"
import { getCwd } from "@/cli/ui/ink-app/utils/cwd"
import { findCanonicalGitRoot, findGitRoot, getIsGit } from "@/cli/ui/ink-app/utils/git"
import { clearMemoryFileCaches } from "@/cli/ui/ink-app/utils/gizzimd"
import { captureHooksConfigSnapshot, updateHooksConfigSnapshot } from "@/cli/ui/ink-app/utils/hooks/hooksConfigSnapshot"
import { getPlanSlug } from "@/cli/ui/ink-app/utils/plans"
import { logEvent } from "@/cli/ui/ink-app/services/analytics/index"
import { saveWorktreeState } from "@/cli/ui/ink-app/utils/sessionStorage"
import { setCwd } from "@/cli/ui/ink-app/utils/Shell"
import { createWorktreeForSession, hasWorktreeCreateHook } from "@/cli/ui/ink-app/utils/worktree"
import { getInitialSettings } from "@/cli/ui/ink-app/utils/settings/settings"
import { enableConfigs } from "@/cli/ui/ink-app/utils/config"
import { enableConfigs as enableSharedConfigs } from "@/shared/utils/config"
import { resolveSessionWorktreeEnabled } from "@/cli/ui/ink-app/threadWorktree"

// Local Event type since SDK Event is now unknown
type Event = any

// Local EventSource contract — the old context/sdk module no longer exists;
// tui() takes options as any, so this structural shape is all that is needed.
interface EventSource {
  on: (handler: (event: Event) => void) => void
}

declare global {
  const GIZZI_WORKER_PATH: string
  const GIZZI_WORKER_CODE: string
}

type RpcClient = ReturnType<typeof Rpc.client<rpc>>

export const WORKER_ORIGIN = "http://gizzi.internal"

/**
 * fetch for the TUI process: requests to the in-process server
 * (WORKER_ORIGIN) go over RPC to the worker; everything else is a normal
 * fetch. tui() installs this as globalThis.fetch, and the RPC carries bodies
 * as text, so sending other hosts through it would garble binary responses
 * (e.g. a bot's avatar image) and buffer streamed ones.
 */
export function createWorkerFetch(client: Pick<RpcClient, "call">, direct: typeof fetch = globalThis.fetch): typeof fetch {
  const fn = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = new Request(input, init)
    if (new URL(request.url).origin !== WORKER_ORIGIN) return direct(request)
    const body = request.body ? await request.text() : undefined
    try {
      const result = await client.call("fetch", {
        url: request.url,
        method: request.method,
        headers: Object.fromEntries(request.headers.entries()),
        body,
      })
      return new Response(result.body, {
        status: result.status,
        headers: result.headers,
      })
    } catch (e) {
      // Worker threw — return a 503 so the SDK gets a real HTTP error response
      // instead of a hanging promise
      Log.Default.error("tui: worker fetch failed", { url: request.url, error: e })
      return new Response(JSON.stringify({ message: e instanceof Error ? e.message : String(e) }), {
        status: 503,
        headers: { "content-type": "application/json" },
      })
    }
  }
  return fn as typeof fetch
}

function createEventSource(client: RpcClient): EventSource {
  return {
    on: (handler) => client.on<Event>("event", handler),
  }
}

export const TuiThreadCommand = cmd({
  command: "$0 [project]",
  describe: "start gizzi tui",
  builder: (yargs) =>
    withNetworkOptions(yargs)
      .positional("project", {
        type: "string",
        describe: "path to start gizzi in",
      })
      .option("model", {
        type: "string",
        alias: ["m"],
        describe: "model to use in the format of provider/model",
      })
      .option("yolo", {
        type: "boolean",
        describe: "skip permission prompts (YOLO mode)",
      })
      .option("dangerously-skip-permissions", {
        type: "boolean",
        describe: "skip permission prompts",
      })
      .option("permission-mode", {
        type: "string",
        describe: "permission mode for the session (default, acceptEdits, plan, bypassPermissions)",
      })
      .option("dangerously-skip-sandbox", {
        type: "boolean",
        describe: "run commands unsandboxed",
      })
      .option("continue", {
        alias: ["c"],
        describe: "continue the last session",
        type: "boolean",
      })
      .option("session", {
        alias: ["s"],
        type: "string",
        describe: "session id to continue",
      })
      .option("fork", {
        type: "boolean",
        describe: "fork the session when continuing (use with --continue or --session)",
      })
      .option("prompt", {
        type: "string",
        describe: "prompt to use",
      })
      .option("agent", {
        type: "string",
        describe: "agent to use",
      })
      .option("worktree", {
        type: "boolean",
        describe:
          "create a new git worktree for this session (--no-worktree disables; defaults to the worktree.autoCreate setting)",
      }),
  handler: async (args) => {
    if (args.yolo || args["dangerously-skip-permissions"]) {
      process.env.GIZZI_PERMISSION_MODE = "yolo"
      process.env.GIZZI_DANGEROUSLY_SKIP_PERMISSIONS = "1"
    } else if (args["permission-mode"]) {
      // Resolved into the ink-app permission context by tui() in app.tsx
      // (yolo → bypassPermissions mapping lives in tuiPermissionStartup.ts).
      process.env.GIZZI_PERMISSION_MODE = args["permission-mode"]
    }
    if (args["dangerously-skip-sandbox"]) {
      process.env.GIZZI_SANDBOX_DISABLE = "1"
    }

    // Keep ENABLE_PROCESSED_INPUT cleared even if other code flips it.
    // (Important when running under `bun run` wrappers on Windows.)
    const unguard = win32InstallCtrlCGuard()
    try {
      // Must be the very first thing — disables CTRL_C_EVENT before any Worker
      // spawn or async work so the OS cannot kill the process group.
      win32DisableProcessedInput()

      if (args.fork && !args.continue && !args.session) {
        console.error("--fork requires --continue or --session")
        process.exitCode = 1
        return
      }

      // Resolve relative paths against PWD to preserve behavior when using --cwd flag
      const baseCwd = process.env.PWD ?? process.cwd()
      const cwd = args.project ? path.resolve(baseCwd, args.project) : process.cwd()
      const localWorker = new URL("./worker.ts", import.meta.url)
      const distWorker = new URL("./worker.js", import.meta.url)
      const workerSpec = await iife(async () => {
        if (typeof GIZZI_WORKER_CODE !== "undefined" && GIZZI_WORKER_CODE) {
          const blob = new Blob([GIZZI_WORKER_CODE], { type: "application/javascript" })
          return URL.createObjectURL(blob)
        }
        if (typeof GIZZI_WORKER_PATH !== "undefined") return GIZZI_WORKER_PATH
        if (await Filesystem.exists(fileURLToPath(distWorker))) return distWorker
        return localWorker
      })
      Log.Default.info("tui: using worker path", { workerPath: String(workerSpec) })
      try {
        process.chdir(cwd)
      } catch (e) {
        console.error("Failed to change directory to " + cwd)
        process.exitCode = 1
        return
      }

      // Project/local settings resolve against getOriginalCwd(), and
      // getCwd()-based helpers read cwd state — point both at the project
      // dir before resolving worktree creation. tui() sets the same state
      // to the same value at startup, so this only makes it visible earlier.
      setCwd(cwd)
      setOriginalCwd(cwd)

      // Worktree creation on the live path (W2b). The live path never calls
      // setup(), so when resolution enables it we run the same native worktree
      // branch setup() runs (src/cli/ui/ink-app/setup.ts) here — before the
      // worker spawns, so both the worker and the TUI start inside the
      // worktree. Resolution is identical to the commander path: --worktree
      // wins, --no-worktree beats the worktree.autoCreate setting, default off.
      if (resolveSessionWorktreeEnabled(args.worktree, getInitialSettings().worktree?.autoCreate)) {
        // Worktree creation reads config (symlink/sparse settings); tui()
        // enables configs idempotently later, so enable them here first.
        enableConfigs()
        enableSharedConfigs()
        captureHooksConfigSnapshot()

        // Mirrors setup.ts: hook-configured sessions can proceed without git
        // so createWorktreeForSession() can delegate to the hook.
        const hasHook = hasWorktreeCreateHook()
        const inGit = await getIsGit()
        if (!hasHook && !inGit) {
          console.error(
            `Error: Can only use --worktree in a git repository, but ${cwd} is not a git repository. ` +
              `Configure a WorktreeCreate hook in settings.json to use --worktree with other VCS systems.`,
          )
          process.exitCode = 1
          return
        }

        if (inGit) {
          // Resolve to the main repo root (handles being invoked from within
          // a worktree), same as setup().
          const mainRepoRoot = findCanonicalGitRoot(getCwd())
          if (!mainRepoRoot) {
            console.error("Error: Could not determine the main git repository root.")
            process.exitCode = 1
            return
          }
          if (mainRepoRoot !== (findGitRoot(getCwd()) ?? getCwd())) {
            process.chdir(mainRepoRoot)
            setCwd(mainRepoRoot)
          }
        }

        let worktreeSession: Awaited<ReturnType<typeof createWorktreeForSession>>
        try {
          worktreeSession = await createWorktreeForSession(getSessionId(), getPlanSlug())
        } catch (e) {
          console.error(`Error creating worktree: ${e instanceof Error ? e.message : String(e)}`)
          process.exitCode = 1
          return
        }
        logEvent("tengu_worktree_created", { tmux_enabled: false })

        process.chdir(worktreeSession.worktreePath)
        setCwd(worktreeSession.worktreePath)
        setOriginalCwd(getCwd())
        // --worktree means the worktree IS the session's project (same as
        // setup.ts): skills/hooks resolve here.
        setProjectRoot(getCwd())
        saveWorktreeState(worktreeSession)
        clearMemoryFileCaches()
        // Re-read hooks from the worktree, same as setup().
        updateHooksConfigSnapshot()
      }

      Log.Default.info("tui: spawning worker")
      const worker = new Worker(workerSpec, {
        env: Object.fromEntries(
          Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
        ),
      })
      Log.Default.info("tui: worker spawned")
      worker.onerror = (e: any) => {
        Log.Default.error("tui: worker error", {
          message: e?.message,
          filename: e?.filename,
          lineno: e?.lineno,
          colno: e?.colno,
          error: e?.error?.stack || e?.error?.message || String(e?.error || e),
        })
        console.error("Worker Error:", e?.message, e?.error || e)
      }
      const client = Rpc.client<rpc>(worker)
      Log.Default.info("tui: rpc client created")
      process.on("uncaughtException", (e) => {
        Log.Default.error(e)
      })
      process.on("unhandledRejection", (e) => {
        Log.Default.error(e)
      })
      process.on("SIGUSR2", async () => {
        await client.call("reload", undefined)
      })

      const prompt = await iife(async () => {
        if (process.stdin.isTTY) return args.prompt
        const readPipedStdin = async (): Promise<string | undefined> => {
          try {
            const text = await Bun.stdin.text()
            return text.trim() ? text : undefined
          } catch {
            return undefined
          }
        }
        const timeout = new Promise<undefined>((resolve) => setTimeout(() => resolve(undefined), 100))
        const piped = await Promise.race([readPipedStdin(), timeout])
        if (!args.prompt) return piped
        return piped ? piped + "\n" + args.prompt : args.prompt
      })

      // Check if server should be started (port or hostname explicitly set in CLI or config)
      const networkOpts = await resolveNetworkOptions(args)
      const shouldStartServer =
        process.argv.includes("--port") ||
        process.argv.includes("--hostname") ||
        process.argv.includes("--mdns") ||
        networkOpts.mdns ||
        networkOpts.port !== 0 ||
        networkOpts.hostname !== "127.0.0.1"

      let url: string
      let customFetch: typeof fetch | undefined
      let events: EventSource | undefined

      if (shouldStartServer) {
        // Start HTTP server for external access
        Log.Default.info("tui: starting http server via worker")
        const server = await client.call("server", networkOpts)
        url = server.url
        Log.Default.info("tui: http server started", { url })
      } else {
        // Use direct RPC communication (no HTTP)
        Log.Default.info("tui: using direct rpc")
        url = WORKER_ORIGIN
        customFetch = createWorkerFetch(client)
        events = createEventSource(client)
      }

      Log.Default.info("tui: calling tui() entry point")
      const tuiPromise = tui({
        url,
        fetch: customFetch,
        events,
        args: {
          continue: args.continue,
          sessionID: args.session,
          agent: args.agent,
          model: args.model,
          prompt,
          fork: args.fork,
        },
        onExit: async () => {
          await client.call("shutdown", undefined)
          // Note: Session exits have telemetry shown via exit.message in session/index.tsx
          // This is for non-session exits (home screen, etc)
        },
      })

      setTimeout(() => {
        client.call("checkUpgrade", { directory: cwd }).catch(() => {})
      }, 1000)

      await tuiPromise
    } finally {
      unguard?.()
    }
    process.exit(0)
  },
})
