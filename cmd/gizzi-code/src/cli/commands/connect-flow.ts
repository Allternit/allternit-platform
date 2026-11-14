import * as prompts from "@clack/prompts"
import QRCode from "qrcode"
import { UI } from "@/cli/ui"
import {
  ConnectFailure,
  DisclosureRequired,
  copyToClipboard,
  disconnect,
  listRows,
  pollLogin,
  startConnect,
  submit,
  type Login,
} from "@/cli/commands/connect-run"
import { DEFAULT_ROWS, findEntry, nameOf, renderHub, resolve, statusText, unavailableText, type Row } from "@/cli/commands/connect-model"

const say = (s = "") => process.stdout.write(s + "\n")

async function readSecret(message: string, hidden: boolean): Promise<string> {
  if (!process.stdin.isTTY) {
    // Piped input: one line, sharing the reader the disclosure prompt uses.
    const { readLine } = await import("@/cli/commands/connect-chatgpt")
    return (await readLine()).trim()
  }
  const v = hidden ? await prompts.password({ message }) : await prompts.text({ message })
  if (prompts.isCancel(v)) throw new UI.CancelledError()
  return String(v).trim()
}

async function qr(url: string) {
  say(await QRCode.toString(url, { type: "terminal", small: true }))
}

async function waitForDone(id: string, row: Row, login: Login) {
  say("Waiting for you to finish. Press Ctrl+C to stop.")
  const r = await pollLogin(id, row.source, login.expires_at)
  if (r.status === "done") {
    const done = r.row ?? row
    say(statusText(findEntry(id) ?? { id, group: "brains", name: id }, { ...done, state: done.state ?? "on" }))
    return
  }
  throw new ConnectFailure(r.status === "expired" ? "The code expired. Run the command again." : "Could not connect. Try again.")
}

async function connectOne(id: string, rows: Row[]) {
  const entry = findEntry(id)
  if (!entry) throw new ConnectFailure(`No connection called "${id}". Run \`gizzi connect\` to see the list.`)
  const row = resolve(rows).find((x) => x.entry.id === entry.id)!.row
  if (row.available === false) {
    say(`${entry.name}: ${unavailableText(entry, row)}`)
    process.exitCode = 1
    return
  }
  const res = await startWithDisclosure(entry.id, row)
  if (!res) return
  const login = res.login
  if (!login) {
    say(`${entry.name}: ${res.state === "on" ? "Connected" : "Connecting…"}`)
    return
  }
  say(`${entry.name}`)
  switch (login.kind) {
    case "paste": {
      const key = await readSecret("Paste the key (hidden)", true)
      if (!key) throw new ConnectFailure("No key given.")
      const updated = await submit(entry.id, row.source, key)
      say(statusText(entry, { ...updated, state: updated.state ?? "on" }))
      return
    }
    case "device_code": {
      const url = login.url_with_code ?? login.url
      say(`Code: ${login.code}`)
      if (url) say(`Open: ${url}`)
      if (login.code && (await copyToClipboard(login.code))) say("The code is copied. Paste it on the page.")
      return waitForDone(entry.id, row, login)
    }
    case "link_paste": {
      if (login.url) say(`Open: ${login.url}`)
      say("Approve there, then copy the code it shows.")
      const code = await readSecret("Paste the code", false)
      if (!code) throw new ConnectFailure("No code given.")
      await submit(entry.id, row.source, code)
      return waitForDone(entry.id, row, login)
    }
    case "scan": {
      if (login.url) {
        await qr(login.url)
        say(`Or open: ${login.url}`)
      }
      return waitForDone(entry.id, row, login)
    }
    case "pair": {
      if (login.code) say(`Pairing code: ${login.code}`)
      if (login.url) say(`Open: ${login.url}`)
      return waitForDone(entry.id, row, login)
    }
    case "oauth":
    case "browser_redirect": {
      if (login.url) {
        say(`Open: ${login.url}`)
        const { default: open } = await import("open")
        await open(login.url).catch(() => undefined)
      }
      return waitForDone(entry.id, row, login)
    }
  }
}

/**
 * POST connect; a runner row answers 409 `disclosure_required` until this
 * account accepts the current version (§9.2). The text is the server's, shown
 * as given; only "y" accepts, and the connect is sent again with
 * `accept_disclosure`. Declining sends nothing and points at the key row.
 */
async function startWithDisclosure(id: string, row: Row) {
  try {
    return await startConnect(id, row.source)
  } catch (e) {
    if (!(e instanceof DisclosureRequired)) throw e
    const { askAccept } = await import("@/cli/commands/connect-chatgpt")
    if (!(await askAccept(e.disclosure))) {
      const fallback = row.fallbacks?.[0] ?? DEFAULT_ROWS[id]?.fallbacks?.[0]
      say(fallback ? `Not connected. ${e.disclosure.decline_label}: gizzi connect ${fallback}` : "Not connected.")
      process.exitCode = 1
      return undefined
    }
    return startConnect(id, row.source, { acceptDisclosure: e.disclosure.version })
  }
}

export async function runConnect(opts: {
  id?: string
  target?: string
  disconnect: boolean
  json: boolean
  acceptDisclosure?: string
  open?: boolean
}) {
  // ChatGPT signs in on this computer (§9.9): `gizzi connect chatgpt`,
  // `gizzi connect status chatgpt`, `gizzi connect disconnect chatgpt` (all take --json).
  const verb = opts.id === "status" || opts.id === "disconnect" ? opts.id : undefined
  const subject = (verb ? opts.target : opts.id)?.toLowerCase()
  if (subject === "chatgpt") {
    const chatgpt = await import("@/cli/commands/connect-chatgpt")
    if (verb === "status") process.exitCode = await chatgpt.runChatgptStatus({ json: opts.json })
    else if (verb === "disconnect" || opts.disconnect) process.exitCode = await chatgpt.runChatgptDisconnect({ json: opts.json })
    else process.exitCode = await chatgpt.runChatgptConnect({ json: opts.json, acceptDisclosure: opts.acceptDisclosure, open: opts.open })
    return
  }
  if (verb && !subject) {
    UI.error(`Name the connection: gizzi connect ${verb} <name>`)
    process.exitCode = 1
    return
  }
  if (verb === "disconnect") opts = { ...opts, id: subject, disconnect: true }
  try {
    const rows = await listRows()
    if (!opts.id) {
      if (opts.json) return say(JSON.stringify(rows, null, 2))
      say(renderHub(rows).join("\n"))
      say("Connect one: gizzi connect <name>   Remove one: gizzi connect <name> --disconnect")
      return
    }
    if (opts.disconnect) {
      const entry = findEntry(opts.id)
      if (!entry) throw new ConnectFailure(`No connection called "${opts.id}". Run \`gizzi connect\` to see the list.`)
      const row = resolve(rows).find((x) => x.entry.id === entry.id)!.row
      await disconnect(entry.id, row.source)
      say(`${nameOf(entry.id)}: Not connected`)
      return
    }
    if (verb === "status") {
      const entry = findEntry(subject!)
      if (!entry) throw new ConnectFailure(`No connection called "${subject}". Run \`gizzi connect\` to see the list.`)
      const row = resolve(rows).find((x) => x.entry.id === entry.id)!.row
      if (opts.json) return say(JSON.stringify(row))
      return say(`${entry.name}: ${row.available === false ? unavailableText(entry, row) : statusText(entry, row)}`)
    }
    await connectOne(opts.id, rows)
  } catch (e) {
    if (e instanceof UI.CancelledError) throw e
    UI.error((e as Error).message)
    process.exitCode = 1
  }
}
