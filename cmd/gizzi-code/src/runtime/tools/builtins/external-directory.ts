import path from "path"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { SessionSandbox } from "@/runtime/context/sandbox/session-sandbox"
import { Sandbox } from "@/runtime/integrations/shell/sandbox"

type Kind = "file" | "directory"

type Options = {
  bypass?: boolean
  kind?: Kind
}

export async function assertExternalDirectory(ctx: Tool.Context, target?: string, options?: Options) {
  if (!target) return

  if (options?.bypass) return

  if (Instance.containsPath(target)) return

  const kind = options?.kind ?? "file"
  const parentDir = kind === "directory" ? target : path.dirname(target)
  const glob = path.join(parentDir, "*")

  await ctx.ask({
    permission: "external_directory",
    patterns: [glob],
    always: [glob],
    metadata: {
      filepath: target,
      parentDir,
    },
  })
}

/**
 * Hard rejection for file-write tools (write/edit/multiedit/apply_patch) when
 * the session has sandboxing enabled -- these tools call Filesystem/fs APIs
 * directly and never go through bwrap/Seatbelt the way the Bash tool does, so
 * this is the only enforcement they get. It's an application-layer path
 * check, not kernel isolation: still worth having, but it doesn't stop a
 * write issued through some other unsandboxed code path the way bwrap's
 * write-bind allowlist stops the Bash tool.
 */
export function assertSandboxWriteAllowed(ctx: Tool.Context, target?: string) {
  if (!target) return
  const state = SessionSandbox.ensureDefault(ctx.sessionID, [Instance.workdir])
  if (!state?.enabled) return
  if (Sandbox.isWriteAllowed(target, Instance.workdir, state.policy)) return
  throw new Error(
    `Sandbox policy blocked this write: ${target} is outside the allowed write paths for this session.`,
  )
}
