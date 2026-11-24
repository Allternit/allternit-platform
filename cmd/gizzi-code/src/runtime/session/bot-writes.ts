/**
 * Tells the Allternit runtime on this computer which files an agent's tools
 * wrote (write, edit, apply_patch, notebook, workspace-edit all publish
 * `File.Event.Edited`). allternit-api keeps a bounded ledger of these paths
 * and lets the signed-in owner's other devices (web, phone) READ exactly
 * those files over the relay, so a bot thread's file cards open there.
 *
 * Reports go to the local gateway only: when `ALLTERNIT_API_URL` points at
 * a remote host, nothing is sent. The runtime accepts reports from loopback
 * callers only, so no other device can add a path. Failures are dropped:
 * a missing report means the file stays closed to other devices, never open.
 */
import { gatewayBase } from "@/shared/constants/allternitGateway"
import { platformTokens } from "@/runtime/bots/platform-api"

export namespace BotWrites {
  /** True when `base` is this computer. */
  export function isLoopback(base: string): boolean {
    try {
      const host = new URL(base).hostname
      return host === "127.0.0.1" || host === "localhost" || host === "[::1]" || host === "::1"
    } catch {
      return false
    }
  }

  export async function report(file: string, sessionID: string | undefined, fetchImpl: typeof fetch = fetch): Promise<void> {
    const base = gatewayBase()
    if (!file || !isLoopback(base)) return
    const tokens = await platformTokens().catch(() => [] as string[])
    const headers: Record<string, string> = { "Content-Type": "application/json" }
    if (tokens[0]) headers.Authorization = `Bearer ${tokens[0]}`
    await fetchImpl(`${base}/api/v1/files/bot-writes`, {
      method: "POST",
      headers,
      body: JSON.stringify({ path: file, session_id: sessionID }),
      signal: AbortSignal.timeout(3000),
    }).catch(() => undefined)
  }
}
