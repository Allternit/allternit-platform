import * as prompts from "@clack/prompts"
import QRCode from "qrcode"
import { UI } from "@/cli/ui"
import {
  ConnectFailure,
  copyToClipboard,
  disconnect,
  listRows,
  pollLogin,
  startConnect,
  submit,
  type Login,
} from "@/cli/commands/connect-run"
import { findEntry, nameOf, renderHub, resolve, statusText, unavailableText, type Row } from "@/cli/commands/connect-model"

const say = (s = "") => process.stdout.write(s + "\n")

async function readSecret(message: string, hidden: boolean): Promise<string> {
  if (!process.stdin.isTTY) {
    const text = await new Response(Bun.stdin.stream()).text()
    return text.split("\n")[0].trim()
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
  const res = await startConnect(entry.id, row.source)
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
    case "oauth": {
      if (login.url) say(`Open: ${login.url}`)
      return waitForDone(entry.id, row, login)
    }
  }
}

export async function runConnect(opts: { id?: string; disconnect: boolean; json: boolean }) {
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
    await connectOne(opts.id, rows)
  } catch (e) {
    if (e instanceof UI.CancelledError) throw e
    UI.error((e as Error).message)
    process.exitCode = 1
  }
}
