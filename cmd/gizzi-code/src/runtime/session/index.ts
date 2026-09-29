import { Slug } from "@allternit/gizzi-util/slug.js"
import path from "path"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Decimal } from "decimal.js"
import z from "zod/v4"
import { type ProviderMetadata } from "ai"
import { Config } from "@/runtime/context/config/config"
import { Flag } from "@/runtime/context/flag/flag"
import { Identifier } from "@/shared/id/id"
import { Installation } from "@/shared/installation"

import { Database, NotFoundError, eq, and, or, gte, isNull, isNotNull, desc, like, inArray, lt } from "@/runtime/session/storage/db"
import type { SQL } from "@/runtime/session/storage/db"
import { SessionTable, MessageTable, PartTable } from "@/runtime/session/session.sql"
import { SessionTrace } from "@/runtime/session/trace"
import { ProjectTable } from "@/runtime/context/project/project.sql"
import { Storage } from "@/runtime/session/storage/storage"
import { Log } from "@/shared/util/log"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Instance } from "@/runtime/context/project/instance"
import { SessionPrompt } from "@/runtime/session/prompt"
import { fn } from "@/shared/util/fn"
import { Command } from "@/runtime/loop/command"
import { Snapshot } from "@/runtime/session/snapshot"

import type { Provider } from "@/runtime/providers/provider"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Global } from "@/runtime/context/global"
import type { LanguageModelV2Usage } from "@ai-sdk/provider"
import { iife } from "@/shared/util/iife"

export namespace Session {
  const log = Log.create({ service: "session" })

  const parentTitlePrefix = "New session - "
  const childTitlePrefix = "Child session - "

  function createDefaultTitle(isChild = false) {
    return (isChild ? childTitlePrefix : parentTitlePrefix) + new Date().toISOString()
  }

  export function isDefaultTitle(title: string) {
    return new RegExp(
      `^(${parentTitlePrefix}|${childTitlePrefix})\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}\\.\\d{3}Z$`,
    ).test(title)
  }

  type SessionRow = typeof SessionTable.$inferSelect

  export function fromRow(row: SessionRow): Info {
    const summary =
      row.summary_additions !== null || row.summary_deletions !== null || row.summary_files !== null
        ? {
            additions: row.summary_additions ?? 0,
            deletions: row.summary_deletions ?? 0,
            files: row.summary_files ?? 0,
            diffs: row.summary_diffs ?? undefined,
          }
        : undefined
    const share = row.share_url ? { url: row.share_url } : undefined
    const revert = row.revert ?? undefined
    return {
      id: row.id,
      slug: row.slug,
      projectID: row.project_id,
      directory: row.directory,
      parentID: row.parent_id ?? undefined,
      continuesFrom: row.continues_from ?? undefined,
      handoff: row.handoff ?? undefined,
      paused: row.paused ?? undefined,
      limit: row.limit_state ?? undefined,
      title: row.title,
      version: row.version,
      summary,
      share,
      revert,
      permission: row.permission ?? undefined,
      agentID: (row as any).agent_id ?? undefined,
      surface: ((row as any).surface as Info["surface"]) ?? undefined,
      harness: (row as any).harness ?? undefined,
      defaultModel: row.default_model
        ? {
            providerID: row.default_model.providerID,
            modelID: row.default_model.modelID,
            authProfileId: row.default_model.authProfileId,
          }
        : undefined,
      defaultModelSource: row.default_model?.source ?? (row.default_model ? "user" : undefined),
      sourceRef: row.source_harness && row.source_session_id && row.source_path && row.source_snapshot_hash
        ? {
            harness: row.source_harness,
            sessionId: row.source_session_id,
            path: row.source_path,
            snapshotHash: row.source_snapshot_hash,
            snapshotAt: row.source_snapshot_at ?? 0,
            eventId: row.source_event_id ?? undefined,
            nativeHash: row.source_native_hash ?? undefined,
            fetchedHash: row.source_fetched_hash ?? undefined,
          }
        : undefined,
      sourceExport: row.source_export ?? undefined,
      time: {
        created: row.time_created,
        updated: row.time_updated,
        compacting: row.time_compacting ?? undefined,
        archived: row.time_archived ?? undefined,
      },
    }
  }

  export function toRow(info: Info) {
    return {
      id: info.id,
      project_id: info.projectID,
      parent_id: info.parentID,
      continues_from: info.continuesFrom ?? null,
      handoff: info.handoff ?? null,
      paused: info.paused ?? null,
      limit_state: info.limit ?? null,
      slug: info.slug,
      directory: info.directory,
      title: info.title,
      version: info.version,
      share_url: info.share?.url,
      summary_additions: info.summary?.additions,
      summary_deletions: info.summary?.deletions,
      summary_files: info.summary?.files,
      summary_diffs: info.summary?.diffs,
      revert: info.revert ?? null,
      permission: info.permission,
      agent_id: info.agentID,
      surface: info.surface ?? null,
      harness: info.harness ?? null,
      default_model: info.defaultModel
        ? {
            ...info.defaultModel,
            source: info.defaultModelSource ?? "user",
          }
        : null,
      source_harness: info.sourceRef?.harness ?? null,
      source_session_id: info.sourceRef?.sessionId ?? null,
      source_path: info.sourceRef?.path ?? null,
      source_snapshot_hash: info.sourceRef?.snapshotHash ?? null,
      source_snapshot_at: info.sourceRef?.snapshotAt ?? null,
      source_event_id: info.sourceRef?.eventId ?? null,
      source_native_hash: info.sourceRef?.nativeHash ?? null,
      source_fetched_hash: info.sourceRef?.fetchedHash ?? null,
      source_export: info.sourceExport ?? null,
      time_created: info.time.created,
      time_updated: info.time.updated,
      time_compacting: info.time.compacting,
      time_archived: info.time.archived,
    }
  }

  function getForkedTitle(title: string): string {
    const match = title.match(/^(.+) \(fork #(\d+)\)$/)
    if (match) {
      const base = match[1]
      const num = parseInt(match[2], 10)
      return `${base} (fork #${num + 1})`
    }
    return `${title} (fork #1)`
  }

  export const Info = z
    .object({
      id: Identifier.schema("session"),
      slug: z.string(),
      projectID: z.string(),
      directory: z.string(),
      parentID: Identifier.schema("session").optional(),
      /** Lineage: the session this one continues after a context handoff. */
      continuesFrom: Identifier.schema("session").optional(),
      /** Set once this session handed off to a fresh context window. */
      handoff: z
        .object({
          sessionID: Identifier.schema("session"),
          reason: z.string(),
          at: z.number(),
          baton: z.unknown().optional(),
        })
        .optional(),
      /** Landed before a usage limit (P3.17); resumes on its own at `until`. */
      paused: z
        .object({
          until: z.number(),
          limit: z.string(),
          providerID: z.string().optional(),
          reason: z.enum(["quota", "rate_limit", "limit_hit", "budget"]),
          at: z.number(),
          /** The model with the most limit left, to continue on (P3.17). */
          suggest: z
            .object({ providerID: z.string(), modelID: z.string(), label: z.string(), headroom: z.number().optional() })
            .optional(),
        })
        .optional(),
      /** Where the session stands against its model's usage limit (wrap-up). */
      limit: z
        .object({
          state: z.enum(["ok", "approaching", "wrapping_up", "wrapped", "paused"]),
          providerID: z.string(),
          windowID: z.string(),
          label: z.string(),
          usedRatio: z.number(),
          resetAt: z.number().optional(),
          at: z.number(),
        })
        .optional(),
      summary: z
        .object({
          additions: z.number(),
          deletions: z.number(),
          files: z.number(),
          diffs: Snapshot.FileDiff.array().optional(),
        })
        .optional(),
      share: z
        .object({
          url: z.string(),
        })
        .optional(),
      title: z.string(),
      version: z.string(),
      time: z.object({
        created: z.number(),
        updated: z.number(),
        compacting: z.number().optional(),
        archived: z.number().optional(),
      }),
      permission: PermissionNext.Ruleset.optional(),
      revert: z
        .object({
          messageID: z.string(),
          partID: z.string().optional(),
          snapshot: z.string().optional(),
          diff: z.string().optional(),
        })
        .optional(),
      agentID: z.string().optional(),
      surface: z.enum(["chat", "cowork", "code", "browser", "design"]).optional(),
      defaultModel: z
        .object({
          providerID: z.string(),
          modelID: z.string(),
          authProfileId: z.string().optional(),
        })
        .optional(),
      defaultModelSource: z.enum(["user", "auto"]).optional(),
      harness: z
        .object({
          mode: z.enum(["byok", "cloud", "local", "subprocess"]),
          byok: z
            .object({
              anthropic: z
                .object({ apiKey: z.string(), baseURL: z.string().optional() })
                .optional(),
              openai: z
                .object({ apiKey: z.string(), baseURL: z.string().optional() })
                .optional(),
              google: z
                .object({ apiKey: z.string(), baseURL: z.string().optional() })
                .optional(),
            })
            .optional(),
          cloud: z
            .object({
              baseURL: z.string(),
              accessToken: z.string(),
              refreshToken: z.string().optional(),
            })
            .optional(),
          local: z.object({ baseURL: z.string() }).optional(),
          subprocess: z
            .object({
              command: z.string(),
              cwd: z.string().optional(),
              env: z.record(z.string(), z.string()).optional(),
            })
            .optional(),
        })
        .optional(),
      sourceRef: z
        .object({
          harness: z.string(),
          sessionId: z.string(),
          path: z.string(),
          snapshotHash: z.string(),
          snapshotAt: z.number(),
          eventId: z.string().optional(),
          nativeHash: z.string().optional(),
          fetchedHash: z.string().optional(),
        })
        .optional(),
      sourceExport: z
        .object({
          harness: z.string(),
          sessionId: z.string(),
          path: z.string(),
          resumeHint: z.string(),
          at: z.number(),
        })
        .optional(),
    })
    
  export type Info = z.output<typeof Info>

  export const ProjectInfo = z
    .object({
      id: z.string(),
      name: z.string().optional(),
      worktree: z.string(),
    })
    
  export type ProjectInfo = z.output<typeof ProjectInfo>

  export const GlobalInfo = Info.extend({
    project: ProjectInfo.nullable(),
  })
  export type GlobalInfo = z.output<typeof GlobalInfo>

  export const Event = {
    Created: BusEvent.define(
      "session.created",
      z.object({
        info: Info,
      }),
    ),
    Updated: BusEvent.define(
      "session.updated",
      z.object({
        info: Info,
      }),
    ),
    Deleted: BusEvent.define(
      "session.deleted",
      z.object({
        info: Info,
      }),
    ),
    Cleared: BusEvent.define(
      "session.cleared",
      z.object({
        sessionID: z.string(),
      }),
    ),
    Diff: BusEvent.define(
      "session.diff",
      z.object({
        sessionID: z.string(),
        diff: Snapshot.FileDiff.array(),
      }),
    ),
    Error: BusEvent.define(
      "session.error",
      z.object({
        sessionID: z.string().optional(),
        error: MessageV2.Assistant.shape.error,
      }),
    ),
    ModelFallback: BusEvent.define(
      "session.model_fallback",
      z.object({
        sessionID: z.string(),
        from: z.object({
          providerID: z.string(),
          modelID: z.string(),
        }),
        to: z.object({
          providerID: z.string(),
          modelID: z.string(),
        }),
        reason: z.string(),
      }),
    ),
  }

  export const create = fn(
    z
      .object({
        parentID: Identifier.schema("session").optional(),
        title: z.string().optional(),
        permission: Info.shape.permission,
        agentID: z.string().optional(),
        surface: Info.shape.surface,
        harness: Info.shape.harness,
        defaultModel: Info.shape.defaultModel,
        defaultModelSource: Info.shape.defaultModelSource,
      })
      .optional(),
    async (input) => {
      return createNext({
        parentID: input?.parentID,
        directory: Instance.directory,
        title: input?.title,
        permission: input?.permission,
        agentID: input?.agentID,
        surface: input?.surface,
        harness: input?.harness,
        defaultModel: input?.defaultModel,
        defaultModelSource: input?.defaultModelSource,
      })
    },
  )

  export const fork = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      messageID: Identifier.schema("message").optional(),
    }),
    async (input) => {
      const original = await get(input.sessionID)
      if (!original) throw new Error("session not found")
      const title = getForkedTitle(original.title)
      const session = await createNext({
        directory: Instance.directory,
        title,
      })
      const msgs = await messages({ sessionID: input.sessionID })
      const idMap = new Map<string, string>()

      for (const msg of msgs) {
        if (input.messageID && msg.info.id >= input.messageID) break
        const newID = Identifier.ascending("message")
        idMap.set(msg.info.id, newID)

        const parentID = msg.info.role === "assistant" && msg.info.parentID ? idMap.get(msg.info.parentID) : undefined
        const cloned = await updateMessage({
          ...msg.info,
          sessionID: session.id,
          id: newID,
          ...(parentID && { parentID }),
        })

        for (const part of msg.parts) {
          await updatePart({
            ...part,
            id: Identifier.ascending("part"),
            messageID: cloned.id,
            sessionID: session.id,
          })
        }
      }
      return session
    },
  )

  export const touch = fn(Identifier.schema("session"), async (sessionID) => {
    const now = Date.now()
    Database.use((db) => {
      const row = db
        .update(SessionTable)
        .set({ time_updated: now })
        .where(eq(SessionTable.id, sessionID))
        .returning()
        .get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${sessionID}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
    })
  })

  export async function createNext(input: {
    id?: string
    title?: string
    parentID?: string
    directory: string
    permission?: PermissionNext.Ruleset
    agentID?: string
    surface?: Info["surface"]
    harness?: Info["harness"]
    defaultModel?: Info["defaultModel"]
    defaultModelSource?: Info["defaultModelSource"]
    sourceRef?: Info["sourceRef"]
    continuesFrom?: string
  }) {
    const result: Info = {
      id: Identifier.descending("session", input.id),
      slug: Slug.create(),
      version: Installation.VERSION,
      projectID: Instance.project.id,
      directory: input.directory,
      parentID: input.parentID,
      title: input.title ?? createDefaultTitle(!!input.parentID),
      permission: input.permission,
      agentID: input.agentID,
      surface: input.surface,
      harness: input.harness,
      defaultModel: input.defaultModel,
      defaultModelSource: input.defaultModelSource,
      sourceRef: input.sourceRef,
      continuesFrom: input.continuesFrom,
      time: {
        created: Date.now(),
        updated: Date.now(),
      },
    }
    log.info("created", result)
    Database.use((db) => {
      db.insert(SessionTable).values(toRow(result)).run()
      Database.effect(() =>
        Bus.publish(Event.Created, {
          info: result,
        }),
      )
    })
    const cfg = await Config.get()
    if (!result.parentID && (Flag.GIZZI_AUTO_SHARE || cfg.share === "auto"))
      share(result.id).catch(() => {
        // Silently ignore sharing errors during session creation
      })
    Bus.publish(Event.Updated, {
      info: result,
    })
    return result
  }

  export function plan(input: { slug: string; time: { created: number } }) {
    const base = Instance.project.vcs
      ? path.join(Instance.worktree, ".gizzi", "plans")
      : path.join(Global.Path.data, "plans")
    return path.join(base, [input.time.created, input.slug].join("-") + ".md")
  }

  export const get = fn(Identifier.schema("session"), async (id) => {
    const row = Database.use((db) => db.select().from(SessionTable).where(eq(SessionTable.id, id)).get())
    if (!row) throw new NotFoundError({ message: `Session not found: ${id}` })
    return fromRow(row)
  })

  export const share = fn(Identifier.schema("session"), async (id) => {
    const cfg = await Config.get()
    if (cfg.share === "disabled") {
      throw new Error("Sharing is disabled in configuration")
    }
    // TODO(types): '@/share/share-next' is a local stub (ShareNext = () => {});
    // the real namespace lives at '@/runtime/session/share/share-next'. The
    // dynamic import is build-dependent ("Module may not exist in all
    // builds"), so the result is pinned to the real create/remove surface
    // without changing which module is loaded.
    const { ShareNext } = (await import("@/share/share-next")) as unknown as {
      ShareNext: {
        create: (
          sessionID: string,
        ) => Promise<{ id: string; url: string; secret: string }>
        remove: (sessionID: string) => Promise<void>
      }
    }
    const share = await ShareNext.create(id)
    Database.use((db) => {
      const row = db.update(SessionTable).set({ share_url: share.url }).where(eq(SessionTable.id, id)).returning().get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${id}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
    })
    return share
  })

  export const unshare = fn(Identifier.schema("session"), async (id) => {
    // Use ShareNext to remove the share (same as share function uses ShareNext to create)
    // TODO(types): '@/share/share-next' is a local stub (ShareNext = () => {});
    // the real namespace lives at '@/runtime/session/share/share-next'. The
    // dynamic import is build-dependent ("Module may not exist in all
    // builds"), so the result is pinned to the real create/remove surface
    // without changing which module is loaded.
    const { ShareNext } = (await import("@/share/share-next")) as unknown as {
      ShareNext: {
        create: (
          sessionID: string,
        ) => Promise<{ id: string; url: string; secret: string }>
        remove: (sessionID: string) => Promise<void>
      }
    }
    await ShareNext.remove(id)
    Database.use((db) => {
      const row = db.update(SessionTable).set({ share_url: null }).where(eq(SessionTable.id, id)).returning().get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${id}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
    })
  })

  export const setTitle = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      title: z.string(),
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({ title: input.title })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  export const setArchived = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      time: z.number().optional(),
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({ time_archived: input.time ?? null })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  export const setPermission = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      permission: PermissionNext.Ruleset,
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({ permission: input.permission, time_updated: Date.now() })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  export const setSurface = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      surface: Info.shape.surface,
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({ surface: input.surface ?? null, time_updated: Date.now() })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  /** Set or clear a session's paused state (P3.17 usage-limit landing). */
  export function setPaused(input: { sessionID: string; paused: Info["paused"] | null }) {
    return Database.use((db) => {
      const row = db
        .update(SessionTable)
        .set({ paused: input.paused ?? null, time_updated: Date.now() })
        .where(eq(SessionTable.id, input.sessionID))
        .returning()
        .get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
      return info
    })
  }

  /** Set or clear where a session stands against its usage limit (wrap-up). */
  export function setLimit(input: { sessionID: string; limit: Info["limit"] | null }) {
    return Database.use((db) => {
      const row = db
        .update(SessionTable)
        .set({ limit_state: input.limit ?? null, time_updated: Date.now() })
        .where(eq(SessionTable.id, input.sessionID))
        .returning()
        .get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
      return info
    })
  }

  /** Sessions currently paused before a usage limit, in this project. */
  export function listPaused(): Info[] {
    const project = Instance.project
    return Database.use((db) =>
      db
        .select()
        .from(SessionTable)
        .where(and(eq(SessionTable.project_id, project.id), isNotNull(SessionTable.paused)))
        .all(),
    ).map(fromRow)
  }

  /** Mark a session as handed off to its successor (context handoff). */
  export function setHandoff(input: { sessionID: string; handoff: NonNullable<Info["handoff"]> }) {
    return Database.use((db) => {
      const row = db
        .update(SessionTable)
        .set({ handoff: input.handoff, time_updated: Date.now() })
        .where(eq(SessionTable.id, input.sessionID))
        .returning()
        .get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
      return info
    })
  }

  export const setRevert = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      revert: Info.shape.revert,
      summary: Info.shape.summary,
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({
            revert: input.revert ?? null,
            summary_additions: input.summary?.additions,
            summary_deletions: input.summary?.deletions,
            summary_files: input.summary?.files,
            time_updated: Date.now(),
          })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  export const clearRevert = fn(Identifier.schema("session"), async (sessionID) => {
    return Database.use((db) => {
      const row = db
        .update(SessionTable)
        .set({
          revert: null,
          time_updated: Date.now(),
        })
        .where(eq(SessionTable.id, sessionID))
        .returning()
        .get()
      if (!row) throw new NotFoundError({ message: `Session not found: ${sessionID}` })
      const info = fromRow(row)
      Database.effect(() => Bus.publish(Event.Updated, { info }))
      return info
    })
  })

  export const setSummary = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      summary: Info.shape.summary,
    }),
    async (input) => {
      return Database.use((db) => {
        const row = db
          .update(SessionTable)
          .set({
            summary_additions: input.summary?.additions,
            summary_deletions: input.summary?.deletions,
            summary_files: input.summary?.files,
            time_updated: Date.now(),
          })
          .where(eq(SessionTable.id, input.sessionID))
          .returning()
          .get()
        if (!row) throw new NotFoundError({ message: `Session not found: ${input.sessionID}` })
        const info = fromRow(row)
        Database.effect(() => Bus.publish(Event.Updated, { info }))
        return info
      })
    },
  )

  export const diff = fn(Identifier.schema("session"), async (sessionID) => {
    try {
      return await Storage.read<Snapshot.FileDiff[]>(["session_diff", sessionID])
    } catch {
      return []
    }
  })

  export const messages = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      limit: z.number().optional(),
    }),
    async (input) => {
      const result = [] as MessageV2.WithParts[]
      for await (const msg of MessageV2.stream(input.sessionID)) {
        if (input.limit && result.length >= input.limit) break
        result.push(msg)
      }
      result.reverse()
      return result
    },
  )

  export function* list(input?: {
    directory?: string
    roots?: boolean
    start?: number
    search?: string
    limit?: number
    agentID?: string
  }) {
    const project = Instance.project
    const conditions = [eq(SessionTable.project_id, project.id)]

    if (input?.directory) {
      conditions.push(eq(SessionTable.directory, input.directory))
    }
    if (input?.roots) {
      conditions.push(isNull(SessionTable.parent_id))
      // A handed-off session lives on as its lineage head; list the head only.
      conditions.push(isNull(SessionTable.handoff))
    }
    if (input?.start) {
      conditions.push(gte(SessionTable.time_updated, input.start))
    }
    if (input?.search) {
      conditions.push(like(SessionTable.title, `%${input.search}%`))
    }
    if (input?.agentID) {
      conditions.push(eq((SessionTable as any).agent_id, input.agentID))
    }

    const limit = input?.limit ?? 100

    const rows = Database.use((db) =>
      db
        .select()
        .from(SessionTable)
        .where(and(...conditions))
        .orderBy(desc(SessionTable.time_updated))
        .limit(limit)
        .all(),
    )
    log.info("list", { projectId: project.id, count: rows.length, roots: input?.roots })
    for (const row of rows) {
      yield fromRow(row)
    }
  }

  export function* listGlobal(input?: {
    directory?: string
    roots?: boolean
    start?: number
    cursor?: number
    search?: string
    limit?: number
    archived?: boolean
  }) {
    const conditions: SQL[] = []

    if (input?.directory) {
      conditions.push(eq(SessionTable.directory, input.directory))
    }
    if (input?.roots) {
      conditions.push(isNull(SessionTable.parent_id))
      // A handed-off session lives on as its lineage head; list the head only.
      conditions.push(isNull(SessionTable.handoff))
    }
    if (input?.start) {
      conditions.push(gte(SessionTable.time_updated, input.start))
    }
    if (input?.cursor) {
      conditions.push(lt(SessionTable.time_updated, input.cursor))
    }
    if (input?.search) {
      conditions.push(like(SessionTable.title, `%${input.search}%`))
    }
    if (!input?.archived) {
      conditions.push(isNull(SessionTable.time_archived))
    }

    const limit = input?.limit ?? 100

    const rows = Database.use((db) => {
      const query =
        conditions.length > 0
          ? db
              .select()
              .from(SessionTable)
              .where(and(...conditions))
          : db.select().from(SessionTable)
      return query.orderBy(desc(SessionTable.time_updated), desc(SessionTable.id)).limit(limit).all()
    })

    const ids = [...new Set(rows.map((row) => row.project_id))]
    const projects = new Map<string, ProjectInfo>()

    if (ids.length > 0) {
      const items = Database.use((db) =>
        db
          .select({ id: ProjectTable.id, name: ProjectTable.name, worktree: ProjectTable.worktree })
          .from(ProjectTable)
          .where(inArray(ProjectTable.id, ids))
          .all(),
      )
      for (const item of items) {
        projects.set(item.id, {
          id: item.id,
          name: item.name ?? undefined,
          worktree: item.worktree,
        })
      }
    }

    for (const row of rows) {
      const project = projects.get(row.project_id) ?? null
      yield { ...fromRow(row), project }
    }
  }

  export const children = fn(Identifier.schema("session"), async (parentID) => {
    const project = Instance.project
    const rows = Database.use((db) =>
      db
        .select()
        .from(SessionTable)
        .where(and(eq(SessionTable.project_id, project.id), eq(SessionTable.parent_id, parentID)))
        .all(),
    )
    return rows.map(fromRow)
  })

  export const clear = fn(Identifier.schema("session"), async (sessionID) => {
    Database.use((db) => {
      db.delete(PartTable).where(eq(PartTable.session_id, sessionID)).run()
      db.delete(MessageTable).where(eq(MessageTable.session_id, sessionID)).run()
      Database.effect(() => Bus.publish(Event.Cleared, { sessionID }))
    })
  })

  export const remove = fn(Identifier.schema("session"), async (sessionID) => {
    const project = Instance.project
    try {
      const session = await get(sessionID)
      for (const child of await children(sessionID)) {
        await remove(child.id)
      }
      await import("@/runtime/session/scratchpad")
        .then(({ Scratchpad }) => Scratchpad.cleanup(sessionID, !session.parentID))
        .catch((error) => log.warn("failed to clean session scratchpad", { sessionID, error }))
      await unshare(sessionID).catch(() => {})
      // CASCADE delete handles messages and parts automatically
      Database.use((db) => {
        db.delete(SessionTable).where(eq(SessionTable.id, sessionID)).run()
        Database.effect(() =>
          Bus.publish(Event.Deleted, {
            info: session,
          }),
        )
      })
    } catch (e) {
      log.error(e)
    }
  })

  export const updateMessage = fn(MessageV2.Info, async (msg) => {
    const time_created = msg.time.created
    const { id, sessionID, ...data } = msg
    Database.use((db) => {
      db.insert(MessageTable)
        .values({
          id,
          session_id: sessionID,
          time_created,
          data,
        })
        .onConflictDoUpdate({ target: MessageTable.id, set: { data } })
        .run()
      Database.effect(() =>
        Bus.publish(MessageV2.Event.Updated, {
          info: msg,
        }),
      )
    })
    SessionTrace.append({ sessionID, kind: "message.updated", messageID: id, data: msg, time: Date.now() })
    return msg
  })

  export const removeMessage = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      messageID: Identifier.schema("message"),
    }),
    async (input) => {
      // CASCADE delete handles parts automatically
      Database.use((db) => {
        db.delete(MessageTable).where(eq(MessageTable.id, input.messageID)).run()
        Database.effect(() =>
          Bus.publish(MessageV2.Event.Removed, {
            sessionID: input.sessionID,
            messageID: input.messageID,
          }),
        )
      })
      SessionTrace.append({
        sessionID: input.sessionID,
        kind: "message.removed",
        messageID: input.messageID,
        data: { messageID: input.messageID },
      })
      return input.messageID
    },
  )

  export const removePart = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      messageID: Identifier.schema("message"),
      partID: Identifier.schema("part"),
    }),
    async (input) => {
      Database.use((db) => {
        db.delete(PartTable).where(eq(PartTable.id, input.partID)).run()
        Database.effect(() =>
          Bus.publish(MessageV2.Event.PartRemoved, {
            sessionID: input.sessionID,
            messageID: input.messageID,
            partID: input.partID,
          }),
        )
      })
      SessionTrace.append({
        sessionID: input.sessionID,
        kind: "part.removed",
        messageID: input.messageID,
        partID: input.partID,
        data: input,
      })
      return input.partID
    },
  )

  const UpdatePartInput = MessageV2.Part

  export const updatePart = fn(UpdatePartInput, async (part) => {
    const { id, messageID, sessionID, ...data } = part
    const time = Date.now()
    Database.use((db) => {
      db.insert(PartTable)
        .values({
          id,
          message_id: messageID,
          session_id: sessionID,
          time_created: time,
          data,
        })
        .onConflictDoUpdate({ target: PartTable.id, set: { data } })
        .run()
      // Note: Bus.publish is called directly by the caller (e.g., processor.ts) for immediate streaming.
      // Database.effect is NOT used here to avoid duplicate events.
    })
    SessionTrace.append({ sessionID, kind: "part.updated", messageID, partID: id, data: part, time })
    return part
  })

  export const updatePartDelta = fn(
    z.object({
      sessionID: z.string(),
      messageID: z.string(),
      partID: z.string(),
      field: z.string(),
      delta: z.string(),
    }),
    async (input) => {
      SessionTrace.append({
        sessionID: input.sessionID,
        kind: "part.delta",
        messageID: input.messageID,
        partID: input.partID,
        data: input,
      })
      Bus.publish(MessageV2.Event.PartDelta, input)
    },
  )

  export const getUsage = fn(
    z.object({
      model: z.custom<Provider.Model>(),
      usage: z.custom<LanguageModelV2Usage>(),
      metadata: z.custom<ProviderMetadata>().optional(),
    }),
    (input) => {
      const safe = (value: number) => {
        if (!Number.isFinite(value)) return 0
        return value
      }
      const inputTokens = safe(input.usage.inputTokens ?? 0)
      const outputTokens = safe(input.usage.outputTokens ?? 0)
      const reasoningTokens = safe(input.usage.reasoningTokens ?? 0)

      const cacheReadInputTokens = safe(input.usage.cachedInputTokens ?? 0)
      const cacheWriteInputTokens = safe(
        (input.metadata?.["anthropic"]?.["cacheCreationInputTokens"] ??
          input.metadata?.["bedrock"]?.["usage"]?.["cacheWriteInputTokens"] ??
          input.metadata?.["venice"]?.["usage"]?.["cacheCreationInputTokens"] ??
          0) as number,
      )

      // OpenRouter provides inputTokens as the total count of input tokens (including cached).
      // AFAIK other providers (OpenRouter/OpenAI/Gemini etc.) do it the same way e.g. vercel/ai#8794 (comment)
      // Anthropic does it differently though - inputTokens doesn't include cached tokens.
      // It looks like GIZZI's cost calculation assumes all providers return inputTokens the same way Anthropic does (I'm guessing getUsage logic was originally implemented with anthropic), so it's causing incorrect cost calculation for OpenRouter and others.
      const excludesCachedTokens = !!(input.metadata?.["anthropic"] || input.metadata?.["bedrock"])
      const adjustedInputTokens = safe(
        excludesCachedTokens ? inputTokens : inputTokens - cacheReadInputTokens - cacheWriteInputTokens,
      )

      const total = iife(() => {
        // Anthropic doesn't provide total_tokens, also ai sdk will vastly undercount if we
        // don't compute from components
        if (input.model.api.npm === "@ai-sdk/anthropic") {
          return adjustedInputTokens + outputTokens + cacheReadInputTokens + cacheWriteInputTokens
        }
        return input.usage.totalTokens
      })

      const tokens = {
        total,
        input: adjustedInputTokens,
        output: outputTokens,
        reasoning: reasoningTokens,
        cache: {
          write: cacheWriteInputTokens,
          read: cacheReadInputTokens,
        },
      }

      const costInfo =
        input.model.cost?.experimentalOver200K && tokens.input + tokens.cache.read > 200_000
          ? input.model.cost.experimentalOver200K
          : input.model.cost
      return {
        cost: safe(
          new Decimal(0)
            .add(new Decimal(tokens.input).mul(costInfo?.input ?? 0).div(1_000_000))
            .add(new Decimal(tokens.output).mul(costInfo?.output ?? 0).div(1_000_000))
            .add(new Decimal(tokens.cache.read).mul(costInfo?.cache?.read ?? 0).div(1_000_000))
            .add(new Decimal(tokens.cache.write).mul(costInfo?.cache?.write ?? 0).div(1_000_000))
            // Reasoning tokens are priced at output token rate (industry standard for o1, Claude, etc.)
            .add(new Decimal(tokens.reasoning).mul(costInfo?.output ?? 0).div(1_000_000))
            .toNumber(),
        ),
        tokens,
      }
    },
  )

  export class BusyError extends Error {
    constructor(public readonly sessionID: string) {
      super(`Session ${sessionID} is busy`)
    }
  }

  export const initialize = fn(
    z.object({
      sessionID: Identifier.schema("session"),
      modelID: z.string(),
      providerID: z.string(),
      messageID: Identifier.schema("message"),
    }),
    async (input) => {
      await SessionPrompt.command({
        sessionID: input.sessionID,
        messageID: input.messageID,
        model: input.providerID + "/" + input.modelID,
        command: Command.Default.INIT,
        arguments: "",
      })
    },
  )
}
