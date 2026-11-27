/**
 * "Needs you" prompts for agent browser runs, through gizzi's one permission
 * prompt (ctx.ask), never a second prompt system.
 *
 * Three things can hold or refuse a toolset call until a person answers:
 * - a credential approval (409/403 `kind: "credential"`): handled in index.ts
 *   with `credentialAsk` below; Approve / Allow for this run / Deny;
 * - `request_human` (`computer.human_requested`, and the masked box
 *   `computer.secret_requested`): Continue or Stop; the secret is typed into a
 *   masked field and goes straight to POST /computers/:id/secret;
 * - a held block (`computer.block_detected`): Continue / Re-run on my Mac /
 *   Stop through POST /computers/:id/block-resolve.
 *
 * The human and block holds arrive on the toolset events stream while the call
 * is waiting inside the executor, so a per-computer watcher listens for the
 * calls in flight and routes each event to the call whose run_id it names.
 *
 * Buttons map onto the permission reply: Approve / Continue = once, the second
 * button (Allow for this run / Re-run on my Mac) = task (surfaces as
 * ReplyData.alt), Deny / Stop = reject. A secret's value rides ReplyData.answer
 * and is never logged, put in metadata, or sent to the model.
 */
import { Log } from "@/shared/util/log"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import type { Tool } from "@/runtime/tools/builtins/tool"
import type { ComputerEvent, ExecutorClient } from "./executor-client"

const log = Log.create({ service: "computer-needs-you" })

export const CREDENTIAL_PERMISSION = "computer.credential"
export const HUMAN_PERMISSION = "computer.human"
export const BLOCK_PERMISSION = "computer.block"

type AskCtx = Pick<Tool.Context, "sessionID" | "messageID" | "abort" | "ask">

interface ButtonOption {
  reply: "once" | "task" | "reject"
  label: string
}

/** Why a person has to answer, for a run nobody can ask (gizzi run without a TTY). */
export function credentialWaitingMessage(approvalId: string, name?: string, domain?: string): string {
  const login = name && domain ? `${name} login on ${domain}` : "login"
  return [
    `Filling the ${login} needs a person's approval (approval id ${approvalId}) and this run has no one to ask.`,
    "Nothing was filled. Approve or deny it from the Allternit app (the needs-you card for this run) or with POST /api/aci/handoff/" +
      `${approvalId}/approve, then run again. gizzi never approves a credential on its own.`,
  ].join(" ")
}

/** The same notice for a human or block hold that came up with nobody to ask. */
export function holdWaitingMessage(summary: string): string {
  return `${summary} This run has no one to ask, so it was left for a person: answer it from the Allternit app (the needs-you card for this run), then run again.`
}

export function credentialOf(body: { kind?: string; credential?: { name?: string; domain?: string } | null }) {
  if (body.kind !== "credential") return undefined
  return { name: body.credential?.name?.trim() || "saved", domain: body.credential?.domain?.trim() || "this site" }
}

export type CredentialDecision = { approved: false } | { approved: true; scope?: "run" }

/**
 * Ask "Fill your <name> login on <domain>?". Approve = once, Allow for this
 * run = scope "run" (this credential name + domain, this run only), Deny =
 * posts /deny and rethrows. A run nobody can ask throws a clear error naming
 * the approval id and leaves the approval pending.
 */
export async function credentialAsk(
  cred: { name: string; domain: string },
  approvalId: string,
  info: { toolset: string; member: string; computer: string },
  ctx: AskCtx,
  executor: ExecutorClient,
): Promise<CredentialDecision> {
  const summary = `Fill your ${cred.name} login on ${cred.domain}?`
  const options: ButtonOption[] = [
    { reply: "once", label: "Approve" },
    { reply: "task", label: "Allow for this run" },
    { reply: "reject", label: "Deny" },
  ]
  try {
    const data = await ctx.ask({
      permission: CREDENTIAL_PERMISSION,
      patterns: [`${cred.domain}:${cred.name}`],
      always: [],
      metadata: { summary, kind: "credential", credential: cred, approval_id: approvalId, options, ...info },
    })
    return { approved: true, scope: data && (data as PermissionNext.ReplyData).alt ? "run" : undefined }
  } catch (err) {
    // Nobody to ask (scheduled turn, dontAsk, run without a TTY): leave the approval pending for the app.
    if (err instanceof PermissionNext.UnattendedError || err instanceof PermissionNext.DeniedError) {
      throw new Error(credentialWaitingMessage(approvalId, cred.name, cred.domain))
    }
    // The person's own Deny (no message): tell the executor so the step is refused and audited.
    if (err instanceof PermissionNext.RejectedError) await executor.deny?.(approvalId, ctx.abort).catch(() => false)
    throw err
  }
}

/** A call in flight that the watcher can route an event to. */
export interface Listener {
  ctx: AskCtx
  executor: ExecutorClient
  /** Who is driving, for "<bot> needs you". */
  botName: string
  /** Ends the waiting call with this error (the person chose Stop). */
  stop(err: Error): void
  /** The person chose "Re-run on my Mac" for this run: where, and the held run's id. */
  rerun?: { computerId: string; fromRunId: string; runtimeId?: string }
}

interface Watch {
  listeners: Map<string, Listener>
  abort: AbortController
  seen: Set<string>
  inflight: number
  linger?: ReturnType<typeof setTimeout>
}

const watches = new Map<string, Watch>()
const LINGER_MS = 30_000

/**
 * Listen for this computer's needs-you events while `listener`'s call is in
 * flight. One stream per computer, kept 30 s after the last call so a run of
 * clicks does not reconnect each time. Returns the release function.
 */
export function watchNeedsYou(computerId: string, listener: Listener): () => void {
  if (!listener.executor.events) return () => {}
  let w = watches.get(computerId)
  if (!w) {
    w = { listeners: new Map(), abort: new AbortController(), seen: new Set(), inflight: 0 }
    watches.set(computerId, w)
    void pump(computerId, w, listener.executor)
  }
  if (w.linger) clearTimeout(w.linger)
  w.linger = undefined
  w.inflight++
  w.listeners.set(listener.ctx.sessionID, listener)
  const watch = w
  return () => {
    if (watch.listeners.get(listener.ctx.sessionID) === listener) watch.listeners.delete(listener.ctx.sessionID)
    if (--watch.inflight > 0) return
    watch.linger = setTimeout(() => {
      watch.abort.abort()
      if (watches.get(computerId) === watch) watches.delete(computerId)
    }, LINGER_MS)
    watch.linger.unref?.()
  }
}

async function pump(computerId: string, w: Watch, executor: ExecutorClient) {
  while (!w.abort.signal.aborted) {
    try {
      for await (const ev of executor.events!(computerId, w.abort.signal)) {
        void route(computerId, w, ev)
      }
    } catch (err) {
      if (w.abort.signal.aborted) return
      log.info("events stream dropped", { computerId, error: err instanceof Error ? err.message : String(err) })
    }
    // The stream ended (server restart, proxy cut): reconnect after a beat.
    await new Promise((r) => setTimeout(r, 2_000))
  }
}

async function route(computerId: string, w: Watch, ev: ComputerEvent) {
  const runId = typeof ev.data?.run_id === "string" ? ev.data.run_id : undefined
  if (!runId) return
  const listener = w.listeners.get(runId)
  if (!listener) return
  const key = `${ev.type}:${runId}:${ev.ts ?? ""}`
  if (w.seen.has(key)) return
  w.seen.add(key)
  if (w.seen.size > 200) w.seen.delete(w.seen.values().next().value!)
  try {
    if (ev.type === "computer.block_detected") await onBlock(computerId, listener, ev)
    else if (ev.type === "computer.secret_requested") await onSecret(computerId, listener, ev)
    else if (ev.type === "computer.human_requested") await onHuman(computerId, listener, ev)
  } catch (err) {
    // Never include event data that could carry a value: type and message only.
    log.info("needs-you prompt failed", { type: ev.type, error: err instanceof Error ? err.message : String(err) })
  }
}

/** Ask; true = the person answered, false = refused. Nobody-to-ask is not a refusal: the hold is left for the app. */
async function askButtons(
  listener: Listener,
  permission: string,
  pattern: string,
  summary: string,
  options: ButtonOption[],
  metadata: Record<string, unknown>,
): Promise<{ reply: "once" | "task" | "reject"; answer?: string } | "nobody"> {
  try {
    const data = (await listener.ctx.ask({
      permission,
      patterns: [pattern],
      always: [],
      metadata: { summary, options, ...metadata },
    })) as PermissionNext.ReplyData | undefined
    return { reply: data?.alt ? "task" : "once", answer: data?.answer }
  } catch (err) {
    if (err instanceof PermissionNext.UnattendedError || err instanceof PermissionNext.DeniedError) return "nobody"
    if (err instanceof PermissionNext.RejectedError || err instanceof PermissionNext.CorrectedError) return { reply: "reject" }
    throw err
  }
}

function stopped(what: string) {
  return new PermissionNext.CorrectedError(what)
}

async function onHuman(computerId: string, l: Listener, ev: ComputerEvent) {
  const reason = String(ev.data.reason ?? "").trim().slice(0, 300) || "it needs a hand"
  // Safari setup on the Mac: show the driver's steps; Continue after the person has done them.
  const steps: string[] = ev.data.kind === "safari_profile" && Array.isArray(ev.data.steps)
    ? ev.data.steps.map((x: unknown) => (typeof x === "string" ? x : String((x as any)?.text ?? (x as any)?.title ?? ""))).filter(Boolean)
    : []
  const summary =
    `${l.botName} needs you: ${reason}` +
    (steps.length ? `\n${steps.map((t, i) => `${i + 1}. ${t}`).join("\n")}\nDo these on your Mac, then choose Continue.` : "")
  const answer = await askButtons(
    l,
    HUMAN_PERMISSION,
    `human:${ev.data.run_id}`,
    summary,
    [
      { reply: "once", label: "Continue" },
      { reply: "reject", label: "Stop" },
    ],
    { kind: "human", reason, computer: computerId, live_view: true, ...(steps.length ? { human_kind: "safari_profile", steps } : {}) },
  )
  if (answer === "nobody") return
  // Continue and Stop both release the waiting call; Stop then ends it.
  await l.executor.humanDone?.(computerId, l.ctx.abort).catch(() => false)
  if (answer.reply === "reject") l.stop(stopped("The person stopped the run at a human hand-off."))
}

async function onSecret(computerId: string, l: Listener, ev: ComputerEvent) {
  const label = String(ev.data.label ?? ev.data.reason ?? "a value").trim().slice(0, 200)
  const domain = String(ev.data.domain ?? "").trim()
  const field = String(ev.data.field ?? "").trim()
  const summary = `${l.botName} needs you: ${label}${domain ? ` (${domain})` : ""}`
  const answer = await askButtons(
    l,
    HUMAN_PERMISSION,
    `secret:${ev.data.run_id}`,
    summary,
    [
      { reply: "once", label: "Send" },
      { reply: "reject", label: "Cancel" },
    ],
    // `secret: true` asks the client for a masked input; the typed value comes back as the reply's answer.
    { kind: "secret", secret: true, mask: true, label, field, domain, computer: computerId },
  )
  if (answer === "nobody") return
  if (answer.reply === "reject") {
    await l.executor.supplySecret?.(computerId, { cancel: true }, l.ctx.abort).catch(() => false)
    return
  }
  if (!answer.answer) {
    // This client has no masked input: the box stays open in the live view until it times out.
    log.info("secret requested; no masked input in this client, left for the live view")
    return
  }
  // The value goes straight to the executor's masked-box route; it is not kept anywhere here.
  await l.executor.supplySecret?.(computerId, { text: answer.answer }, l.ctx.abort).catch(() => false)
}

async function onBlock(computerId: string, l: Listener, ev: ComputerEvent) {
  const domain = String(ev.data.domain ?? "this site")
  const targets: Array<{ computer_id: string; name?: string; runtime_id?: string }> = Array.isArray(ev.data.rerun_targets) ? ev.data.rerun_targets : []
  const target = targets[0]
  const options: ButtonOption[] = [
    { reply: "once", label: "Continue" },
    ...(target ? [{ reply: "task" as const, label: "Re-run on my Mac" }] : []),
    { reply: "reject", label: "Stop" },
  ]
  const answer = await askButtons(l, BLOCK_PERMISSION, `block:${domain}`, `${domain} blocked ${l.botName}`, options, {
    kind: "block",
    domain,
    block_kind: ev.data.kind,
    rerun_targets: targets,
    computer: computerId,
  })
  if (answer === "nobody") return
  if (answer.reply === "task" && target) {
    const res = await l.executor.blockResolve?.(computerId, { action: "rerun", rerun_target: target.computer_id }, l.ctx.abort)
    // Gizzi reaches only the runtime behind its own API base (no per-runtime relay), so the
    // run starts there addressed by the target's computer id; `runtime_id` is kept for when
    // gizzi can relay to a Mac's own runtime. A 404 computer_unavailable surfaces as the
    // toolset call's error below.
    if (res?.ok) {
      l.rerun = { computerId: target.computer_id, fromRunId: res.run_id ?? String(ev.data.run_id), runtimeId: target.runtime_id }
    }
    return
  }
  const action = answer.reply === "reject" ? "stop" : "continue"
  await l.executor.blockResolve?.(computerId, { action }, l.ctx.abort)
  if (action === "stop") l.stop(stopped(`The person stopped the run: ${domain} blocked it.`))
}
