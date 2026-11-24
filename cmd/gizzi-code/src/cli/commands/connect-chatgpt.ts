/**
 * `gizzi connect chatgpt`: Sign in with ChatGPT on this computer
 * (CONTRACT-connections-2026-11 §9.9). Text mode for a person at the
 * terminal; `--json` for Desktop's runtime (allternit-api's `chatgpt` row,
 * login kind `browser_redirect`), which starts the sign-in, opens the URL
 * itself and polls `gizzi connect status chatgpt --json`.
 *
 * JSON lines from `gizzi connect chatgpt --json` (one object per line):
 *   {"state":"needs","reason_code":"disclosure_required","disclosure":{…}}   exit 2
 *   {"state":"connecting","authorize_url":"…","redirect_uri":"…","expires_at":"…"}
 *   {"state":"on","found":"…","models":["chatgpt/…"]}                          exit 0
 *   {"state":"needs","reason_code":"could_not_start","reason":"…"}             exit 1
 */
import fs from "fs"
import path from "path"
import { defaultCredentialDir } from "@/runtime/context/config/credential-store"
import {
  ChatgptAuthError,
  defaultDeps,
  loadSession,
  saveSession,
  signIn,
  signOut,
  type Session,
} from "@/runtime/providers/person-plan/chatgpt-auth"
import { fetchChatgptModels } from "@/runtime/providers/person-plan/turn"
import type { Disclosure } from "@/cli/commands/connect-run"
import type { Row } from "@/cli/commands/connect-model"

export const DISCLOSURE_VERSION = "2026-11-14.1"

/**
 * The §9.8 disclosure, vendor ChatGPT, worded for a sign-in that stays on
 * this computer (§9.9). Wording pending Eoj's approval with the rest of §9.8.
 */
export const DEVICE_DISCLOSURE: Disclosure = {
  version: DISCLOSURE_VERSION,
  title: "Before you connect your ChatGPT plan",
  body: [
    "Your plan runs from this computer. Gizzi Code talks to ChatGPT directly, and your messages to it do not go through Allternit.",
    "ChatGPT's terms may not allow using a plan this way. ChatGPT could limit or close your account. Allternit does not control that and cannot undo it.",
    "You sign in yourself. Your sign-in stays on this computer and Allternit does not copy it anywhere else. Disconnect signs you out and deletes it.",
    "Your plan only answers messages you send yourself, in chat. Bots, routines, schedules and agents never use it.",
  ],
  accept_label: "I understand, connect",
  decline_label: "Use a key instead",
}

// A subfolder: the credential store treats every `*.json` directly in ~/.gizzi as a legacy secrets file.
const dir = () => path.join(defaultCredentialDir(), "chatgpt")
const consentFile = () => path.join(dir(), "chatgpt-disclosure.json")
const markerFile = () => path.join(dir(), "chatgpt-signin.json")

function writePrivate(file: string, value: unknown) {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 })
  const tmp = `${file}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(value), { mode: 0o600 })
  fs.renameSync(tmp, file)
}

function readJson<T>(file: string): T | undefined {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8")) as T
  } catch {
    return undefined
  }
}

export function acceptedDisclosure(): { version: string; accepted_at: string } | undefined {
  const saved = readJson<{ version?: string; accepted_at?: string }>(consentFile())
  return saved?.version === DISCLOSURE_VERSION && saved.accepted_at ? { version: saved.version, accepted_at: saved.accepted_at } : undefined
}

export function recordDisclosure(version: string): void {
  if (version !== DISCLOSURE_VERSION) throw new Error(`Unknown disclosure version ${version}; the current one is ${DISCLOSURE_VERSION}.`)
  writePrivate(consentFile(), { version, accepted_at: new Date().toISOString() })
}

type Marker = { state: "connecting" | "failed"; pid: number; authorize_url?: string; started_at: string; reason?: string; reason_code?: string }

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

export type ChatgptStatus = {
  id: "chatgpt"
  state: "off" | "connecting" | "on" | "needs"
  found?: string
  email?: string
  reason_code?: string
  reason?: string
  authorize_url?: string
  models: string[]
  disclosure: { version: string; accepted_at: string | null }
}

function foundText(s: Session): string {
  return [s.plan, s.email].filter(Boolean).join(" · ") || "Signed in on this computer"
}

/** Status from what is stored here: no network, no refresh. */
export async function chatgptStatus(): Promise<ChatgptStatus> {
  const disclosure = { version: DISCLOSURE_VERSION, accepted_at: acceptedDisclosure()?.accepted_at ?? null }
  const session = await loadSession().catch(() => undefined)
  if (session) {
    return {
      id: "chatgpt",
      state: "on",
      found: foundText(session),
      email: session.email,
      models: (session.models ?? []).map((m) => `chatgpt/${m.slug}`),
      disclosure,
    }
  }
  const marker = readJson<Marker>(markerFile())
  if (marker?.state === "connecting" && alive(marker.pid))
    return { id: "chatgpt", state: "connecting", authorize_url: marker.authorize_url, models: [], disclosure }
  if (marker?.state === "failed" || (marker?.state === "connecting" && !alive(marker.pid)))
    return {
      id: "chatgpt",
      state: "needs",
      reason_code: marker.reason_code ?? "could_not_start",
      reason: marker.reason ?? "Could not start. Try again",
      models: [],
      disclosure,
    }
  return { id: "chatgpt", state: "off", models: [], disclosure }
}

/**
 * The hub's ChatGPT row in the terminal: `gizzi connect chatgpt` always signs
 * in on this computer, so this row answers unless it is off and the local
 * runtime reports Codex signed in (§9.9 precedence: device, then runtime, then runner).
 */
export async function localChatgptRow(): Promise<Row> {
  const s = await chatgptStatus()
  return {
    id: "chatgpt",
    group: "brains",
    state: s.state,
    found: s.found ? `${s.found} · signed in here` : null,
    reason: s.reason ?? null,
    reason_code: s.reason_code ?? null,
    runs_on: "device",
    available: true,
    source: "device",
    models: s.models,
    person_only: true,
    disclosure: s.disclosure,
  }
}

const say = (s = "") => process.stdout.write(s + "\n")
const emit = (o: unknown) => process.stdout.write(JSON.stringify(o) + "\n")

/** Asks `<accept_label>? [y/N]` and reads one line; only y/yes accepts. */
export async function askAccept(d: Disclosure): Promise<boolean> {
  say(d.title)
  say()
  for (const line of d.body) say(`  ${line}`)
  say()
  process.stdout.write(`${d.accept_label}? [y/N] `)
  const answer = await readLine()
  return /^y(es)?$/i.test(answer.trim())
}

let pending = ""
let ended = false

/** One line from stdin. Text after the line is kept for the next call (a piped "y\n<code>\n"). */
export async function readLine(): Promise<string> {
  const take = () => {
    const nl = pending.indexOf("\n")
    if (nl === -1) return undefined
    const line = pending.slice(0, nl)
    pending = pending.slice(nl + 1)
    return line.replace(/\r$/, "")
  }
  const ready = take()
  if (ready !== undefined) return ready
  if (ended) {
    const rest = pending
    pending = ""
    return rest
  }
  return new Promise((resolve) => {
    const onData = (chunk: Buffer | string) => {
      pending += chunk.toString()
      const line = take()
      if (line !== undefined) done(line)
    }
    const onEnd = () => {
      ended = true
      const rest = pending
      pending = ""
      done(rest)
    }
    const done = (line: string) => {
      process.stdin.off("data", onData)
      process.stdin.off("end", onEnd)
      process.stdin.pause()
      resolve(line)
    }
    process.stdin.on("data", onData)
    process.stdin.on("end", onEnd)
    process.stdin.resume()
  })
}

async function modelsFor(session: Session): Promise<string[]> {
  try {
    const fresh = await fetchChatgptModels(session.access_token)
    if (fresh.length) {
      await saveSession({ ...session, models: fresh, models_at: Date.now() })
      return fresh.map((m) => `chatgpt/${m.slug}`)
    }
  } catch {}
  return (session.models ?? []).map((m) => `chatgpt/${m.slug}`)
}

export async function runChatgptConnect(opts: { json: boolean; acceptDisclosure?: string; open?: boolean }): Promise<number> {
  // Disclosure first (version 2026-11-14.1, vendor ChatGPT).
  if (opts.acceptDisclosure) {
    try {
      recordDisclosure(opts.acceptDisclosure)
    } catch (e) {
      if (opts.json) emit({ state: "needs", reason_code: "disclosure_required", reason: (e as Error).message, disclosure: DEVICE_DISCLOSURE })
      else say((e as Error).message)
      return 2
    }
  }
  if (!acceptedDisclosure()) {
    if (opts.json) {
      emit({ state: "needs", reason_code: "disclosure_required", disclosure: DEVICE_DISCLOSURE })
      return 2
    }
    if (!(await askAccept(DEVICE_DISCLOSURE))) {
      say("Not connected. To use a key instead: gizzi connect openai-key")
      return 1
    }
    recordDisclosure(DISCLOSURE_VERSION)
  }

  const started_at = new Date().toISOString()
  const expires_at = new Date(Date.now() + 10 * 60_000).toISOString()
  try {
    const session = await signIn({
      openBrowser: opts.open ?? !opts.json,
      onAuthorizeUrl: (url) => {
        writePrivate(markerFile(), { state: "connecting", pid: process.pid, authorize_url: url, started_at } satisfies Marker)
        const redirect = new URL(url).searchParams.get("redirect_uri")
        if (opts.json) emit({ state: "connecting", authorize_url: url, redirect_uri: redirect, expires_at })
        else {
          say("ChatGPT")
          say(`Open: ${url}`)
          say("Sign in there. This finishes by itself. Press Ctrl+C to stop.")
        }
      },
    }, defaultDeps())
    fs.rmSync(markerFile(), { force: true })
    const models = await modelsFor(session)
    if (opts.json) emit({ state: "on", found: foundText(session), models })
    else say(`ChatGPT: Connected · ${foundText(session)}${models.length ? ` · ${models.length} models` : ""}`)
    return 0
  } catch (e) {
    const code = e instanceof ChatgptAuthError ? e.code : "could_not_start"
    const reason = e instanceof Error ? e.message : String(e)
    const reason_code = code === "timeout" || code === "aborted" ? "login_expired" : "could_not_start"
    writePrivate(markerFile(), { state: "failed", pid: process.pid, started_at, reason, reason_code } satisfies Marker)
    if (opts.json) emit({ state: "needs", reason_code, reason })
    else say(`ChatGPT: ${reason}`)
    return 1
  }
}

export async function runChatgptStatus(opts: { json: boolean }): Promise<number> {
  const s = await chatgptStatus()
  if (opts.json) {
    emit(s)
    return 0
  }
  if (s.state === "on") say(`ChatGPT: Connected · ${s.found}`)
  else if (s.state === "connecting") say("ChatGPT: Connecting…")
  else if (s.state === "needs") say(`ChatGPT: Needs you · ${s.reason}`)
  else say("ChatGPT: Not connected · Plus or Pro plan. Connect it: gizzi connect chatgpt")
  return 0
}

export async function runChatgptDisconnect(opts: { json: boolean }): Promise<number> {
  const r = await signOut()
  fs.rmSync(markerFile(), { force: true })
  if (opts.json) {
    emit({ id: "chatgpt", state: "off", revoked: r.revoked })
    return 0
  }
  if (!r.wasSignedIn) say("ChatGPT: Not connected")
  else if (r.revoked) say("ChatGPT: Not connected. You are signed out and the sign-in is deleted from this computer.")
  else
    say(
      "ChatGPT: Not connected. The sign-in is deleted from this computer, but ChatGPT did not confirm the sign-out. " +
        "You can remove Gizzi Code in ChatGPT Settings.",
    )
  return 0
}

