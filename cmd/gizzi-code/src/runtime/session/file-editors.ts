import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { File } from "@/shared/file"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "session.file-editors" })

/**
 * Who is editing what, across the Code sessions in this process.
 *
 * Writes to one file are already serialized by FileTime.withLock, so two
 * sessions never overwrite each other mid-write. This registry only makes the
 * overlap visible: the app shows "Other tab is also editing <file>" from it.
 * In memory only; entries older than KEEP_MS are dropped.
 */
export namespace FileEditors {
  /** How long an edit is remembered at all. */
  export const KEEP_MS = 30 * 60_000
  /** How recent another session's edit must be to count as "also editing". */
  export const CONTENDED_MS = 10 * 60_000

  export const Event = {
    Contended: BusEvent.define(
      "file.contended",
      z.object({
        file: z.string(),
        sessionID: z.string(),
        otherSessionIDs: z.array(z.string()),
      }),
    ),
  }

  /** file → sessionID → last edit time (ms). */
  const byFile = new Map<string, Map<string, number>>()
  /** sessionID → files it edited. */
  const bySession = new Map<string, Set<string>>()

  function prune(now: number) {
    for (const [file, editors] of byFile) {
      for (const [sessionID, at] of editors) {
        if (now - at <= KEEP_MS) continue
        editors.delete(sessionID)
        const files = bySession.get(sessionID)
        files?.delete(file)
        if (files && files.size === 0) bySession.delete(sessionID)
      }
      if (editors.size === 0) byFile.delete(file)
    }
  }

  /** Other sessions that edited `file` within CONTENDED_MS of `now`. */
  function othersOn(file: string, sessionID: string, now: number) {
    const editors = byFile.get(file)
    if (!editors) return []
    return [...editors]
      .filter(([id, at]) => id !== sessionID && now - at <= CONTENDED_MS)
      .sort((a, b) => b[1] - a[1])
      .map(([id, at]) => ({ sessionID: id, lastEditAt: at }))
  }

  /** Record one edit. Returns the other sessions it now contends with (empty when none). */
  export function record(file: string, sessionID: string, now = Date.now()) {
    prune(now)
    let editors = byFile.get(file)
    if (!editors) byFile.set(file, (editors = new Map()))
    editors.set(sessionID, now)
    let files = bySession.get(sessionID)
    if (!files) bySession.set(sessionID, (files = new Set()))
    files.add(file)
    return othersOn(file, sessionID, now).map((o) => o.sessionID)
  }

  /** Forget a session (on delete). */
  export function forget(sessionID: string) {
    const files = bySession.get(sessionID)
    bySession.delete(sessionID)
    for (const file of files ?? []) {
      const editors = byFile.get(file)
      editors?.delete(sessionID)
      if (editors && editors.size === 0) byFile.delete(file)
    }
  }

  export type Entry = { path: string; otherSessions: { sessionID: string; lastEditAt: number }[] }

  /** The session's edited files (most recent first) and, per file, other sessions editing it now. */
  export function forSession(sessionID: string, now = Date.now()): { files: Entry[]; edited: string[] } {
    prune(now)
    const files = [...(bySession.get(sessionID) ?? [])]
    files.sort((a, b) => (byFile.get(b)?.get(sessionID) ?? 0) - (byFile.get(a)?.get(sessionID) ?? 0))
    return {
      edited: files,
      files: files.map((path) => ({ path, otherSessions: othersOn(path, sessionID, now) })),
    }
  }

  /** Test hook. */
  export function reset() {
    byFile.clear()
    bySession.clear()
  }

  /** Subscribe on the current Instance's bus (called from InstanceBootstrap). */
  export function init() {
    Bus.subscribe(File.Event.Edited, (payload) => {
      const { file, sessionID } = payload.properties
      if (!sessionID) return
      const others = record(file, sessionID)
      if (others.length === 0) return
      void Bus.publish(Event.Contended, { file, sessionID, otherSessionIDs: others }).catch((e) =>
        log.warn("file.contended publish failed", { error: e instanceof Error ? e.message : String(e) }),
      )
    })
    void import("@/runtime/session")
      .then(({ Session }) => Bus.subscribe(Session.Event.Deleted, (payload) => forget(payload.properties.info.id)))
      .catch((e) => log.warn("session delete hook failed", { error: e instanceof Error ? e.message : String(e) }))
  }
}
