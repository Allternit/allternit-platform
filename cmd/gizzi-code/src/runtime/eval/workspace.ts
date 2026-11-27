import { $ } from "bun"
import fs from "fs/promises"
import path from "path"
import { Snapshot } from "@/runtime/session/snapshot/snapshot"
import { Instance } from "@/runtime/context/project/instance"
import { Config } from "@/runtime/context/config/config"
import type { MessageV2 } from "@/runtime/session/message-v2"

export type WorkspaceRef =
  | { kind: "archive"; path: "workspace.tar.gz"; bytes: number; snapshot: string }
  | { kind: "none"; reason: "not_git" | "snapshot_off" | "pruned" | "too_large" | "no_project" }

const ARCHIVE = "workspace.tar.gz" as const

/** Tree hash of the first step-start part that carries a snapshot. */
export function firstSnapshot(messages: MessageV2.WithParts[]): string | undefined {
  for (const message of messages) {
    for (const part of message.parts) {
      if (part.type === "step-start" && part.snapshot) return part.snapshot
    }
  }
  return undefined
}

/** Must run inside Instance.provide for the session's project. */
export async function copyOut(
  sessionInfo: { projectID: string },
  messages: MessageV2.WithParts[],
  destDir: string,
  capBytes = 50 * 1024 * 1024,
): Promise<WorkspaceRef> {
  if (Instance.project.id !== sessionInfo.projectID) return { kind: "none", reason: "no_project" }
  const snapshot = firstSnapshot(messages)
  if (!snapshot) {
    // No step snapshot: say why, so the UI can tell "not a git repo" from "snapshots off".
    if (Instance.project.vcs !== "git") return { kind: "none", reason: "not_git" }
    if ((await Config.get()).snapshot === false) return { kind: "none", reason: "snapshot_off" }
    return { kind: "none", reason: "no_project" }
  }
  await fs.mkdir(destDir, { recursive: true })
  const dest = path.join(destDir, ARCHIVE)
  const result = await Snapshot.archive(snapshot, dest)
  if ("error" in result) return { kind: "none", reason: result.error }
  if (result.bytes > capBytes) {
    await fs.unlink(dest).catch(() => {})
    return { kind: "none", reason: "too_large" }
  }
  return { kind: "archive", path: ARCHIVE, bytes: result.bytes, snapshot }
}

/** Extracts the case workspace into tmpDir. A trace-only case gets an empty dir. */
export async function restoreTo(caseDir: string, workspace: WorkspaceRef, tmpDir: string): Promise<void> {
  await fs.mkdir(tmpDir, { recursive: true })
  if (workspace.kind === "none") return
  const result = await $`tar -xzf ${path.join(caseDir, workspace.path)} -C ${tmpDir}`.quiet().nothrow()
  if (result.exitCode !== 0) {
    throw new Error(`workspace extract failed: ${result.stderr.toString().trim()}`)
  }
}
