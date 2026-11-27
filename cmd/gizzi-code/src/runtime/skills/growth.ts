import { createHash, randomUUID } from "crypto"
import fs from "fs/promises"
import path from "path"
import { Global } from "@/runtime/context/global"
import { Instance } from "@/runtime/context/project/instance"
import {
  GrowthServer,
  GrowthSyncRefused,
  GrowthSyncUnavailable,
  type ServerOp,
  type ServerProposal,
} from "@/runtime/skills/growth-server"

/**
 * Skill growth: versioned skill proposals that only become a SKILL.md through
 * the `skill_activate` tool (which asks for `skill_manage`).
 *
 * Every proposal is also a server knowledge proposal (`target_kind: "skill"`,
 * see ./growth-server) so the curator and the review screens see it. The
 * device file is the offline cache: a change the server did not get is kept
 * as `sync: "pending"` with its `pending_ops` and sent on the next
 * skill-growth call. Each call also pulls proposals the person approved on
 * the server (their own, or the curator's), which marks them `approved`
 * here. Pulling never activates anything.
 */
export namespace SkillGrowth {
  export type Status = "proposed" | "evaluated" | "approved" | "rejected" | "active" | "rolled_back"
  export interface Record {
    id: string
    name: string
    description: string
    content: string
    scope: "user" | "project"
    status: Status
    version: number
    digest: string
    score?: number
    evaluation?: string
    target?: string
    activatedAt?: string
    createdAt: string
    updatedAt: string
    /** Who wrote it: this device's skill growth, or the server curator (pulled once approved). */
    origin?: "skill_growth" | "curator"
    /** The server knowledge proposal id (`kprop_…`). */
    server_id?: string
    /** `pending`: the server has not got every change yet; `failed`: the server refused the proposal. */
    sync?: "pending" | "synced" | "failed"
    sync_error?: string
    /** Changes the server has not got yet, oldest first. */
    pending_ops?: ServerOp[]
    /** Where the approval came from. */
    approved_on?: "device" | "server"
  }

  const root = () => path.join(Global.Path.data, "skill-growth")
  const recordsDir = () => path.join(root(), "proposals")
  const historyDir = (name: string) => path.join(root(), "history", name)

  // One skill-growth operation at a time per process, so two tool calls never
  // create the same server proposal twice or race on a record file.
  let queue: Promise<unknown> = Promise.resolve()
  function exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = queue.then(fn, fn)
    queue = run.catch(() => {})
    return run
  }

  export function propose(input: {
    name: string
    description: string
    content: string
    scope: "user" | "project"
  }): Promise<Record> {
    return exclusive(async () => {
      assertName(input.name)
      if (!input.description.trim()) throw new Error("Skill description is required")
      if (!input.content.trim()) throw new Error("Skill content is required")
      await syncAll()
      const now = new Date().toISOString()
      const record: Record = {
        ...input,
        id: randomUUID(),
        status: "proposed",
        version: await nextVersion(input.name),
        digest: digest(input.content),
        createdAt: now,
        updatedAt: now,
        origin: "skill_growth",
        sync: "pending",
        pending_ops: [],
      }
      await save(record)
      return push(record)
    })
  }

  export function evaluate(id: string, input: { score: number; report: string }): Promise<Record> {
    return exclusive(async () => {
      await syncAll()
      const record = await requireRecord(id)
      if (record.status !== "proposed" && record.status !== "evaluated") {
        throw new Error(`Cannot evaluate a ${record.status} proposal`)
      }
      if (!Number.isFinite(input.score) || input.score < 0 || input.score > 1) throw new Error("Score must be between 0 and 1")
      const next = await update(record, { status: "evaluated", score: input.score, evaluation: input.report, ...queued(record, "eval") })
      return push(next)
    })
  }

  export function decide(id: string, decision: "approve" | "reject"): Promise<Record> {
    return exclusive(async () => {
      await syncAll()
      const record = await requireRecord(id)
      if (record.status !== "evaluated") throw new Error("A proposal must be evaluated before approval")
      if (decision === "approve" && (record.score ?? 0) < 0.7) {
        throw new Error("Evaluation score is below the 0.70 activation threshold")
      }
      const next = await update(record, {
        status: decision === "approve" ? "approved" : "rejected",
        ...(decision === "approve" ? { approved_on: "device" as const } : {}),
        ...queued(record, decision),
      })
      return push(next)
    })
  }

  /**
   * Write the approved version to SKILL.md. The caller (`skill_activate`) has
   * already asked for `skill_manage`; this is the only path that writes a
   * skill file, also for proposals approved on the server. `id` is the local
   * id or the server id (`kprop_…`).
   */
  export function activate(id: string): Promise<Record> {
    return exclusive(async () => {
      await syncAll()
      const record = await requireRecord(id)
      if (record.status !== "approved") throw new Error("Only an approved skill proposal can be activated")
      const target = targetFor(record.name, record.scope)
      const existing = await fs.readFile(target, "utf8").catch(() => undefined)
      const history = historyDir(record.name)
      await fs.mkdir(history, { recursive: true })
      const snapshot = {
        proposalID: record.id,
        version: record.version,
        target,
        previous: existing ?? null,
        activated: render(record),
        createdAt: new Date().toISOString(),
      }
      await atomicWrite(path.join(history, `${record.version}.json`), JSON.stringify(snapshot, null, 2))
      await atomicWrite(target, snapshot.activated)
      const next = await update(record, {
        status: "active",
        target,
        activatedAt: new Date().toISOString(),
        ...queued(record, "activated"),
      })
      return push(next)
    })
  }

  export function rollback(id: string): Promise<Record> {
    return exclusive(async () => {
      await syncAll()
      const record = await requireRecord(id)
      if (record.status !== "active" || !record.target) throw new Error("Only an active proposal can be rolled back")
      const raw = await fs.readFile(path.join(historyDir(record.name), `${record.version}.json`), "utf8")
      const snapshot = JSON.parse(raw) as { target: string; previous: string | null; activated: string }
      const current = await fs.readFile(snapshot.target, "utf8").catch(() => undefined)
      if (current !== snapshot.activated) throw new Error("Active skill changed since activation; refusing to overwrite it")
      if (snapshot.previous === null) await fs.rm(snapshot.target)
      else await atomicWrite(snapshot.target, snapshot.previous)
      const next = await update(record, { status: "rolled_back", ...queued(record, "rollback") })
      return push(next)
    })
  }

  /** Send pending changes and pull server approvals. Never throws and never activates. */
  export function sync(): Promise<void> {
    return exclusive(() => syncAll())
  }

  /** Proposals approved (here or on the server) that are not active yet: they wait for `skill_activate`. */
  export async function awaitingActivation(): Promise<Record[]> {
    return (await list()).filter((item) => item.status === "approved")
  }

  /** A proposal by its local id or its server id (`kprop_…`). */
  export async function find(id: string): Promise<Record | undefined> {
    return (await get(id)) ?? (await list()).find((item) => item.server_id === id)
  }

  export async function get(id: string): Promise<Record | undefined> {
    return read(path.join(recordsDir(), `${id}.json`))
  }

  export async function list(): Promise<Record[]> {
    const names = await fs.readdir(recordsDir()).catch(() => [])
    const records = await Promise.all(names.filter((name) => name.endsWith(".json")).map((name) => read(path.join(recordsDir(), name))))
    return records.filter((item): item is Record => Boolean(item)).toSorted((a, b) => b.createdAt.localeCompare(a.createdAt))
  }

  // ── Server sync ───────────────────────────────────────────────────────────

  function queued(record: Record, op: ServerOp): Partial<Record> {
    return { sync: "pending", pending_ops: [...(record.pending_ops ?? []), op] }
  }

  /** Send one record's pending changes. Offline keeps it pending; it never throws for sync reasons. */
  async function push(record: Record): Promise<Record> {
    try {
      return await pushOrThrow(record)
    } catch (error) {
      if (error instanceof GrowthSyncUnavailable) return (await get(record.id)) ?? record
      throw error
    }
  }

  async function pushOrThrow(record: Record): Promise<Record> {
    let current = record
    if (current.sync !== "pending") return current
    if (!current.server_id) {
      const evaluated = current.score !== undefined
      try {
        const created = await GrowthServer.create({
          target_kind: "skill",
          target_ref: current.name,
          title: `${current.name} v${current.version}: ${current.description}`.slice(0, 200),
          body: render(current),
          base: await currentSkill(current),
          origin: "skill_growth",
          eval: evaluated ? { score: current.score!, report: current.evaluation ?? "" } : undefined,
        })
        // The eval rode along with the create.
        const ops = (current.pending_ops ?? []).filter((op) => !(evaluated && op === "eval"))
        current = await update(current, { server_id: created.id, pending_ops: ops })
      } catch (error) {
        if (error instanceof GrowthSyncRefused) {
          return update(current, { sync: "failed", sync_error: error.message })
        }
        throw error
      }
    }
    while ((current.pending_ops ?? []).length > 0) {
      const op = current.pending_ops![0]!
      const evalBody = op === "eval" ? { score: current.score ?? 0, report: current.evaluation ?? "" } : undefined
      await GrowthServer.send(current.server_id!, op, evalBody)
      current = await update(current, { pending_ops: current.pending_ops!.slice(1) })
    }
    return update(current, { sync: "synced", sync_error: undefined })
  }

  async function syncAll(): Promise<void> {
    const pending = (await list()).filter((item) => item.sync === "pending").toReversed()
    for (const record of pending) {
      try {
        await pushOrThrow(record)
      } catch (error) {
        if (error instanceof GrowthSyncUnavailable) return // offline: no pull either; next call retries
        throw error
      }
    }
    await pullApproved().catch(() => {})
  }

  /**
   * Server approvals reach this device: a local proposal still `proposed` or
   * `evaluated` becomes `approved`; a curator skill proposal with no local
   * copy is cached from the server body as `approved`. Both then wait for
   * `skill_activate` and its `skill_manage` ask.
   */
  async function pullApproved(): Promise<void> {
    const approved = await GrowthServer.listApproved()
    if (approved.length === 0) return
    const local = await list()
    for (const proposal of approved) {
      const mine = local.find((item) => item.server_id === proposal.id)
      if (mine) {
        if (mine.status === "proposed" || mine.status === "evaluated") {
          await update(mine, {
            status: "approved",
            approved_on: "server",
            // The server already decided; only later changes still need sending.
            pending_ops: (mine.pending_ops ?? []).filter((op) => op !== "eval" && op !== "approve" && op !== "reject"),
          })
        }
        continue
      }
      if (proposal.origin === "curator") await cacheCuratorProposal(proposal)
    }
  }

  async function cacheCuratorProposal(proposal: ServerProposal): Promise<void> {
    const name = proposal.target_ref
    if (!/^[a-z0-9][a-z0-9-]{0,63}$/.test(name)) return // plugin skills (`plugin:skill`) are not written from here
    const parsed = parseSkill(proposal.body, proposal.title)
    if (!parsed.content.trim()) return
    const now = new Date().toISOString()
    const projectTarget = safeProjectTarget(name)
    const scope = projectTarget && (await exists(projectTarget)) ? "project" : "user"
    await save({
      id: randomUUID(),
      name,
      description: parsed.description,
      content: parsed.content,
      scope,
      status: "approved",
      version: await nextVersion(name),
      digest: digest(parsed.content),
      createdAt: now,
      updatedAt: now,
      origin: "curator",
      server_id: proposal.id,
      sync: "synced",
      pending_ops: [],
      approved_on: "server",
    })
  }

  // ── Local helpers ─────────────────────────────────────────────────────────

  async function requireRecord(id: string) {
    const record = await find(id)
    if (!record) {
      const waiting = (await list()).filter((item) => item.status === "approved")
      const hint = waiting.length
        ? ` Approved and waiting for skill_activate: ${waiting.map((item) => `${item.id} (${item.name})`).join(", ")}.`
        : ""
      throw new Error(`Skill proposal ${id} not found.${hint}`)
    }
    return record
  }

  async function nextVersion(name: string) {
    const previous = (await list()).filter((item) => item.name === name)
    return Math.max(0, ...previous.map((item) => item.version)) + 1
  }

  function targetFor(name: string, scope: Record["scope"]) {
    const dir = scope === "project" ? path.join(Instance.worktree, ".gizzi", "skills", name) : path.join(Global.Path.config, "skills", name)
    return path.join(dir, "SKILL.md")
  }

  function safeProjectTarget(name: string) {
    try {
      return targetFor(name, "project")
    } catch {
      return undefined // no project open (e.g. a background sync)
    }
  }

  /** The SKILL.md this proposal would replace, if there is one (the server proposal's `base`). */
  async function currentSkill(record: Record): Promise<string | undefined> {
    const target = record.scope === "project" ? safeProjectTarget(record.name) : targetFor(record.name, "user")
    return target ? fs.readFile(target, "utf8").catch(() => undefined) : undefined
  }

  async function exists(file: string) {
    return fs.stat(file).then(() => true, () => false)
  }

  async function update(record: Record, patch: Partial<Record>) {
    const next = { ...record, ...patch, updatedAt: new Date().toISOString() }
    await save(next)
    return next
  }

  async function save(record: Record) {
    await fs.mkdir(recordsDir(), { recursive: true })
    await atomicWrite(path.join(recordsDir(), `${record.id}.json`), JSON.stringify(record, null, 2))
  }

  async function read(file: string): Promise<Record | undefined> {
    return fs.readFile(file, "utf8").then((raw) => JSON.parse(raw) as Record).catch(() => undefined)
  }

  function render(record: Record) {
    return `---\nname: ${record.name}\ndescription: ${JSON.stringify(record.description)}\n---\n\n${record.content.trim()}\n`
  }

  /** Split a SKILL.md body into its description and content (the name always comes from `target_ref`). */
  function parseSkill(body: string, fallbackDescription: string) {
    const match = body.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/)
    if (!match) return { description: fallbackDescription, content: body }
    const line = match[1]!.match(/^description:\s*(.*)$/m)?.[1]?.trim()
    let description = fallbackDescription
    if (line) {
      try {
        description = line.startsWith('"') ? (JSON.parse(line) as string) : line.replace(/^'(.*)'$/, "$1")
      } catch {
        description = line
      }
    }
    return { description, content: match[2]! }
  }

  function assertName(name: string) {
    if (!/^[a-z0-9][a-z0-9-]{0,63}$/.test(name)) throw new Error("Skill name must be lowercase kebab-case")
  }

  function digest(content: string) {
    return createHash("sha256").update(content).digest("hex")
  }

  async function atomicWrite(file: string, content: string) {
    await fs.mkdir(path.dirname(file), { recursive: true })
    const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`
    await fs.writeFile(temporary, content, "utf8")
    await fs.rename(temporary, file)
  }
}
