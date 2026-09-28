import z from "zod/v4"
import { spawn } from "child_process"
import { Tool } from "@/runtime/tools/builtins/tool"
import path from "path"
import DESCRIPTION from "@/runtime/tools/builtins/bash.txt"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { lazy } from "@/shared/util/lazy"
import { Language } from "web-tree-sitter"

import { $ } from "bun"
import { Filesystem } from "@/shared/util/filesystem"
import { fileURLToPath } from "url"
import { Flag } from "@/runtime/context/flag/flag.ts"
import { Shell } from "@/runtime/integrations/shell/shell"
import { ProcessRegistry } from "@/runtime/process-registry"

import { BashArity } from "@/runtime/tools/guard/permission/arity"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { Plugin } from "@/runtime/integrations/plugin"
import { SessionSandbox } from "@/runtime/context/sandbox/session-sandbox"
import { Sandbox } from "@/runtime/integrations/shell/sandbox"
import { VmSession } from "@/runtime/context/vm/vm-session"

const MAX_METADATA_LENGTH = 30_000
const DEFAULT_TIMEOUT = Flag.GIZZI_EXPERIMENTAL_BASH_DEFAULT_TIMEOUT_MS || 2 * 60 * 1000

export const log = Log.create({ service: "bash-tool" })

const resolveWasm = (asset: string) => {
  if (asset.startsWith("file://")) return fileURLToPath(asset)
  if (asset.startsWith("/") || /^[a-z]:/i.test(asset)) return asset
  const url = new URL(asset, import.meta.url)
  return fileURLToPath(url)
}

const parser = lazy(async () => {
  const { Parser } = await import("web-tree-sitter")
  const { default: treeWasm } = await import("web-tree-sitter/tree-sitter.wasm" as string, {
    with: { type: "wasm" },
  })
  const treePath = resolveWasm(treeWasm)
  await Parser.init({
    locateFile() {
      return treePath
    },
  })
  const { default: bashWasm } = await import("tree-sitter-bash/tree-sitter-bash.wasm" as string, {
    with: { type: "wasm" },
  })
  const bashPath = resolveWasm(bashWasm)
  const bashLanguage = await Language.load(bashPath)
  const p = new Parser()
  p.setLanguage(bashLanguage)
  return p
})

// Tool is named "bash" for LLM compatibility (models are trained on this name).
// The actual shell used is determined by Shell.acceptable() and may be zsh, fish, etc.
export const BashTool = Tool.define("bash", async () => {
  const shell = Shell.acceptable()
  log.info("bash tool using shell", { shell })

  return {
    description: DESCRIPTION.replaceAll("${directory}", Instance.workdir)
      .replaceAll("${maxLines}", String(Truncate.MAX_LINES))
      .replaceAll("${maxBytes}", String(Truncate.MAX_BYTES)),
    parameters: z.object({
      command: z.string().describe("The command to execute"),
      timeout: z.number().describe("Optional timeout in milliseconds").optional(),
      workdir: z
        .string()
        .describe(
          `The working directory to run the command in. Defaults to ${Instance.workdir}. Use this instead of 'cd' commands.`,
        )
        .optional(),
      description: z
        .string()
        .describe(
          "Clear, concise description of what this command does in 5-10 words. Examples:\nInput: ls\nOutput: Lists files in current directory\n\nInput: git status\nOutput: Shows working tree status\n\nInput: npm install\nOutput: Installs package dependencies\n\nInput: mkdir foo\nOutput: Creates directory 'foo'",
        ),
    }),
    async execute(params, ctx) {
      const cwd = params.workdir || Instance.workdir
      if (params.timeout !== undefined && params.timeout < 0) {
        throw new Error(`Invalid timeout value: ${params.timeout}. Timeout must be a positive number.`)
      }
      const timeout = params.timeout ?? DEFAULT_TIMEOUT
      const tree = await parser().then((p) => p.parse(params.command))
      if (!tree) {
        throw new Error("Failed to parse command")
      }
      const directories = new Set<string>()
      if (!Instance.containsPath(cwd)) directories.add(cwd)
      const patterns = new Set<string>()
      const always = new Set<string>()

      for (const node of tree.rootNode.descendantsOfType("command")) {
        if (!node) continue

        // Get full command text including redirects if present
        let commandText = node.parent?.type === "redirected_statement" ? node.parent.text : node.text

        const command = []
        for (let i = 0; i < node.childCount; i++) {
          const child = node.child(i)
          if (!child) continue
          if (
            child.type !== "command_name" &&
            child.type !== "word" &&
            child.type !== "string" &&
            child.type !== "raw_string" &&
            child.type !== "concatenation"
          ) {
            continue
          }
          command.push(child.text)
        }

        // not an exhaustive list, but covers most common cases
        if (["cd", "rm", "cp", "mv", "mkdir", "touch", "chmod", "chown", "cat"].includes(command[0])) {
          for (const arg of command.slice(1)) {
            if (arg.startsWith("-") || (command[0] === "chmod" && arg.startsWith("+"))) continue
            const resolved = await $`realpath ${arg}`
              .cwd(cwd)
              .quiet()
              .nothrow()
              .text()
              .then((x) => x.trim())
            log.info("resolved path", { arg, resolved })
            if (resolved) {
              // Git Bash on Windows returns Unix-style paths like /c/Users/...
              const normalized =
                process.platform === "win32" && resolved.match(/^\/[a-z]\//)
                  ? resolved.replace(/^\/([a-z])\//, (_, drive) => `${drive.toUpperCase()}:\\`).replace(/\//g, "\\")
                  : resolved
              if (!Instance.containsPath(normalized)) {
                const dir = (await Filesystem.isDir(normalized)) ? normalized : path.dirname(normalized)
                directories.add(dir)
              }
            }
          }
        }

        // cd covered by above check
        if (command.length && command[0] !== "cd") {
          patterns.add(commandText)
          always.add(BashArity.prefix(command).join(" ") + " *")
        }
      }

      if (directories.size > 0) {
        const globs = Array.from(directories).map((dir) => path.join(dir, "*"))
        await ctx.ask({
          permission: "external_directory",
          patterns: globs,
          always: globs,
          metadata: {},
        })
      }

      if (patterns.size > 0) {
        await ctx.ask({
          permission: "bash",
          patterns: Array.from(patterns),
          always: Array.from(always),
          metadata: {},
        })
      }

      const shellEnv = await Plugin.trigger(
        "shell.env",
        { cwd, sessionID: ctx.sessionID, callID: ctx.callID },
        { env: {} },
      )

      // ── VM Session execution ───────────────────────────────────────────────
      // When GIZZI_VM_SESSIONS is enabled (or the session has an active VM),
      // route ALL bash execution through the provisioned VM instead of spawning
      // a local subprocess. This matches gizzi-code's cloud session model where
      // the entire agent session runs inside a dedicated VM.
      //
      // Auto-provision on first bash call when GIZZI_VM_SESSIONS is set.
      if (!VmSession.get(ctx.sessionID) && VmSession.isEnabled()) {
        try {
          await VmSession.provision(ctx.sessionID, {
            workdir: cwd,
            networkEnabled: true,
          })
        } catch (err) {
          log.warn("VM auto-provision failed, falling back to local execution", {
            error: err instanceof Error ? err.message : String(err),
            sessionID: ctx.sessionID,
          })
        }
      }
      const vmState = VmSession.get(ctx.sessionID)
      if (vmState) {
        try {
          log.info("routing through VM session", {
            sessionID: ctx.sessionID,
            vmSessionId: vmState.sessionId,
            vmBacked: vmState.vmBacked,
            command: params.command.slice(0, 100),
          })

          ctx.metadata({ metadata: { output: "", description: params.description } })

          // Translate the host absolute cwd to a VM-relative path inside /workspace.
          // git-cloned: /host/project/src  →  /workspace/src
          // bind-mounted: path is the same inside the VM
          const vmWorkdir = params.workdir
            ? (() => {
                const rel = path.relative(vmState.workdir, params.workdir)
                // If relative escapes the workdir root, fall back to workspace root
                return rel.startsWith("..") ? vmState.workspacePath : path.join(vmState.workspacePath, rel)
              })()
            : undefined

          const vmResult = await VmSession.exec(
            ctx.sessionID,
            params.command,
            {
              env: shellEnv.env as Record<string, string>,
              timeoutSecs: Math.ceil(timeout / 1000),
              workdir: vmWorkdir,
            },
            ctx.abort,
          )

          const combinedOutput = vmResult.stderr
            ? `${vmResult.stdout}\n[STDERR]\n${vmResult.stderr}`
            : vmResult.stdout

          ctx.metadata({
            metadata: {
              output:
                combinedOutput.length > MAX_METADATA_LENGTH
                  ? combinedOutput.slice(0, MAX_METADATA_LENGTH) + "\n\n..."
                  : combinedOutput,
              exit: vmResult.exitCode,
              description: params.description,
              vm_backed: vmResult.vmBacked,
            },
          })

          return {
            title: params.description,
            metadata: {
              output:
                combinedOutput.length > MAX_METADATA_LENGTH
                  ? combinedOutput.slice(0, MAX_METADATA_LENGTH) + "\n\n..."
                  : combinedOutput,
              exit: vmResult.exitCode,
              description: params.description,
            },
            output: combinedOutput,
          }
        } catch (err) {
          // The API host may now be refusing to execute unsandboxed (see
          // vm_session_routes.rs's fail-closed change) if its VM driver died
          // mid-session, or the request failed outright. Never treat this as
          // "ran fine" — fall through to the local, OS-sandboxed path below
          // rather than surfacing a bare crash or (worse) trusting a result
          // that might have run unsandboxed on a shared host.
          log.warn("VM session exec failed, falling back to local sandboxed execution", {
            error: err instanceof Error ? err.message : String(err),
            sessionID: ctx.sessionID,
          })
        }
      }
      // ── End VM Session execution ───────────────────────────────────────────

      // ── Sandbox wrapping ───────────────────────────────────────────────────
      // Sandboxed by default (bwrap on Linux, sandbox-exec on macOS) — same as
      // gizzi-code. GIZZI_SANDBOX_DISABLE (or an explicit per-session /sandbox
      // toggle) is the only opt-out. If sandboxing is enabled but no isolation
      // driver is available on a platform that's supposed to support one
      // (Linux without bwrap), this fails closed — it throws rather than
      // silently running the command unsandboxed.
      const sandboxState = SessionSandbox.ensureDefault(ctx.sessionID, [cwd])
      const sandboxEnabled = sandboxState?.enabled ?? false

      let proc: ReturnType<typeof spawn>

      if (sandboxEnabled && process.platform !== "win32") {
        const driver = Sandbox.detect()
        if (driver === "none") {
          throw new Error(
            "Sandbox is enabled by default but no isolation driver is available on this platform. " +
              "Install bubblewrap (e.g. `apt install bubblewrap`) to sandbox Bash, or set " +
              "GIZZI_SANDBOX_DISABLE=1 / pass --dangerously-skip-sandbox to run unsandboxed intentionally.",
          )
        }

        const wrapped = await Sandbox.wrap({
          command: params.command,
          shell,
          cwd,
          sessionID: ctx.sessionID,
          policy: sandboxState!.policy,
        })
        if (!wrapped) {
          throw new Error("Sandbox driver became unavailable while wrapping the command.")
        }

        log.info("sandbox active", { driver, sessionID: ctx.sessionID })
        proc = spawn(wrapped.bin, wrapped.args, {
          cwd,
          env: { ...process.env, ...shellEnv.env },
          stdio: ["ignore", "pipe", "pipe"],
          // Don't use detached with bwrap/sandbox-exec — --die-with-parent handles cleanup
          detached: false,
        })
        ProcessRegistry.track(proc, { label: "bash-sandbox" })
      } else {
        proc = spawn(params.command, {
          shell,
          cwd,
          env: { ...process.env, ...shellEnv.env },
          stdio: ["ignore", "pipe", "pipe"],
          detached: process.platform !== "win32",
        })
        ProcessRegistry.track(proc, { label: "bash", group: process.platform !== "win32" })
      }
      // ── End sandbox wrapping ───────────────────────────────────────────────

      let output = ""

      // Initialize metadata with empty output
      ctx.metadata({
        metadata: {
          output: "",
          description: params.description,
        },
      })

      const append = (chunk: Buffer) => {
        output += chunk.toString()
        ctx.metadata({
          metadata: {
            // truncate the metadata to avoid GIANT blobs of data (has nothing to do w/ what agent can access)
            output: output.length > MAX_METADATA_LENGTH ? output.slice(0, MAX_METADATA_LENGTH) + "\n\n..." : output,
            description: params.description,
          },
        })
      }

      proc.stdout?.on("data", append)
      proc.stderr?.on("data", append)

      let timedOut = false
      let aborted = false
      let exited = false

      const kill = () => Shell.killTree(proc, { exited: () => exited })

      if (ctx.abort.aborted) {
        aborted = true
        await kill()
      }

      const abortHandler = () => {
        aborted = true
        void kill()
      }

      ctx.abort.addEventListener("abort", abortHandler, { once: true })

      const timeoutTimer = setTimeout(() => {
        timedOut = true
        void kill()
      }, timeout + 100)

      await new Promise<void>((resolve, reject) => {
        const cleanup = () => {
          clearTimeout(timeoutTimer)
          ctx.abort.removeEventListener("abort", abortHandler)
        }

        proc.once("exit", () => {
          exited = true
          cleanup()
          resolve()
        })

        proc.once("error", (error) => {
          exited = true
          cleanup()
          reject(error)
        })
      })

      const resultMetadata: string[] = []

      if (timedOut) {
        resultMetadata.push(`bash tool terminated command after exceeding timeout ${timeout} ms`)
      }

      if (aborted) {
        resultMetadata.push("User aborted the command")
      }

      if (resultMetadata.length > 0) {
        output += "\n\n<bash_metadata>\n" + resultMetadata.join("\n") + "\n</bash_metadata>"
      }

      return {
        title: params.description,
        metadata: {
          output: output.length > MAX_METADATA_LENGTH ? output.slice(0, MAX_METADATA_LENGTH) + "\n\n..." : output,
          exit: proc.exitCode,
          description: params.description,
        },
        output,
      }
    },
  }
})
