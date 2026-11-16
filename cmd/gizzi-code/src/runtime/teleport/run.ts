/**
 * Resolve a session reference and run one Teleport. Shared by `gizzi teleport`
 * and the native-session route the apps call, so both behave the same.
 */
import { Session } from "@/runtime/session"
import { VendorSession } from "@/runtime/session/vendor-session"
import { platformRequest } from "@/runtime/bots/platform-api"
import { workspaceListing, type WorkspaceListing } from "@/runtime/teleport/bundle"
import {
  reachableBase,
  teleportToCloud,
  teleportToLocal,
  type TeleportDirection,
  type TeleportHarness,
  type TeleportResult,
} from "@/runtime/teleport/teleport"

interface Place {
  id: string | null
  label: string
  ready: boolean
}

export interface RunTeleportArgs {
  /** gizzi session id, or the harness's own session id. */
  session: string
  to: TeleportDirection
  harness: TeleportHarness
  targetId?: string
  model?: string
  effort?: string
  /** With `to: "local"`: the commit the session left this Mac on. */
  base?: string
  cwd?: string
}

export async function pickTarget(explicit?: string): Promise<string> {
  if (explicit) return explicit
  const { places } = await platformRequest<{ places: Place[] }>("GET", "/api/v1/placements")
  const cloud = places.filter((p) => p.id && p.ready)
  if (cloud.length === 0) throw new Error("No cloud computer is connected. Connect one in Allternit, then retry.")
  if (cloud.length > 1) {
    throw new Error(`More than one computer is connected; choose one. Options: ${cloud.map((p) => `${p.id} (${p.label})`).join(", ")}`)
  }
  return cloud[0]!.id!
}

export async function runTeleport(args: RunTeleportArgs): Promise<TeleportResult> {
  let nativeId = args.session
  let model = args.model
  let cwd = args.cwd ?? process.cwd()
  const known = await Session.get(args.session).catch(() => undefined)
  if (known) {
    cwd = known.directory
    nativeId = (await VendorSession.get(known.id, args.harness)) ?? nativeId
    model ??= known.defaultModel?.modelID
  }
  if (!model) throw new Error("This session has no model recorded; pass one.")
  const input = {
    sessionID: known?.id ?? args.session,
    harness: args.harness,
    nativeId,
    cwd,
    model,
    reasoning: args.effort,
    targetId: await pickTarget(args.targetId),
  }
  return args.to === "cloud" ? teleportToCloud(input) : teleportToLocal({ ...input, gitBase: args.base })
}

/**
 * The approval listing for a move to the cloud, without sending anything:
 * which changed and untracked files would travel and which are left out as secrets.
 */
export async function listTeleport(args: Pick<RunTeleportArgs, "session" | "cwd">): Promise<WorkspaceListing & { cwd: string; base: string }> {
  const known = await Session.get(args.session).catch(() => undefined)
  const cwd = known?.directory ?? args.cwd ?? process.cwd()
  const base = reachableBase(cwd)
  if (!base) throw new Error("This workspace has no reachable git remote with your current branch on it, so nothing would be sent.")
  return { ...workspaceListing(cwd, base), cwd, base }
}
