/**
 * Action classes (contracts/action-classes): every action an agent takes falls
 * into one of four fixed classes, and the class decides whether it runs, asks
 * first or never runs. A setting, a mode or a model cannot add a class or move
 * an action to a weaker one.
 *
 *   hand_off              never runs (disk wipes, fork bombs, human checks)
 *   confirm_now           asks every time, in every mode; refused unattended
 *                         (money, credentials, permanent delete, signing)
 *   confirm_unless_named  runs without asking only when a live task grant or
 *                         the person's own config rule names it
 *                         (send, upload, publish, deploy)
 *   free                  runs; the normal permission rules still apply
 *
 * Pure and deterministic: no I/O, no clock, no model. The shared table
 * `contracts/action-classes/vectors.json` is the test both this file and the
 * Rust reviewer (allternit-api computer_safety.rs / computer_access.rs) must
 * pass, so the two sides cannot drift.
 *
 * Rules (README): the stricter class wins; labels match whole words, and a
 * longer phrase beats a shorter one at the same place ("Send money" is money,
 * "Sign in" is not signing, "Postal code" is not "Post").
 */
import os from "os"
import path from "path"
import { Catastrophic } from "@/runtime/tools/guard/permission/catastrophic"
import { isCredentialPath } from "@/runtime/tools/guard/permission/credential-paths"

export namespace ActionClasses {
  export const CLASSES = ["hand_off", "confirm_now", "confirm_unless_named", "free"] as const
  export type Class = (typeof CLASSES)[number]

  export type Topic =
    | "disk"
    | "human_check"
    | "money"
    | "credential"
    | "delete"
    | "sign"
    | "access"
    | "send"
    | "upload"
    | "publish"
    | "deploy"
    | "none"

  export interface Input {
    /** computer | browser | pane_browser | bash | write | mcp */
    tool: string
    /** The member, op or verb: a computer/browser member, a pane_browser action, `run`, `write`, `call`. */
    action: string
    /** A summary of what the reviewer sees (label, role, secure, text, op, path, command, host, app, ref, name, …). */
    args?: Record<string, unknown>
  }

  export interface Context {
    /** The project worktree: deletes inside it are not "outside the project". */
    projectRoot?: string
    /** Override for tests; defaults to os.homedir(). */
    home?: string
  }

  export interface Result {
    class: Class
    topic: Topic
    /** A few plain words for the ask card and logs. Never carries typed secrets. */
    reason: string
  }

  const RANK: Record<Class, number> = { free: 0, confirm_unless_named: 1, confirm_now: 2, hand_off: 3 }

  /** Higher = stricter. */
  export function rank(c: Class): number {
    return RANK[c]
  }

  export function stricter(a: Result, b: Result): Result {
    return RANK[b.class] > RANK[a.class] ? b : a
  }

  const TOPIC_CLASS: Record<Topic, Class> = {
    disk: "hand_off",
    human_check: "hand_off",
    money: "confirm_now",
    credential: "confirm_now",
    delete: "confirm_now",
    sign: "confirm_now",
    access: "confirm_now",
    send: "confirm_unless_named",
    upload: "confirm_unless_named",
    publish: "confirm_unless_named",
    deploy: "confirm_unless_named",
    none: "free",
  }

  const TOPIC_WORD: Record<Topic, string> = {
    disk: "Destroys the machine's data",
    human_check: "A human check",
    money: "Money",
    credential: "Credentials",
    delete: "Permanent delete",
    sign: "Signing",
    access: "Grants computer access",
    send: "Send",
    upload: "Upload",
    publish: "Publish",
    deploy: "Deploy",
    none: "Routine",
  }

  function result(topic: Topic, detail?: string): Result {
    const head = TOPIC_WORD[topic]
    return { class: TOPIC_CLASS[topic], topic, reason: detail ? `${head}: ${detail}` : head }
  }

  const FREE: Result = { class: "free", topic: "none", reason: "Routine" }

  /** Classify one tool call. Deterministic; the stricter class wins. */
  export function classify(input: Input, context: Context = {}): Result {
    const tool = (input.tool ?? "").toLowerCase()
    const action = (input.action ?? "").toLowerCase()
    const args = input.args ?? {}
    switch (tool) {
      case "bash":
      case "shell":
        return classifyShell(str(args.command) ?? "", context)
      case "write":
      case "edit":
        return classifyWrite(str(args.path), str(args.text))
      case "computer":
      case "browser":
      case "pane_browser":
        return classifyControl(action, args)
      case "mcp":
        return classifyName(str(args.name) ?? action)
      default:
        return classifyName(str(args.name) ?? tool)
    }
  }

  function str(v: unknown): string | undefined {
    return typeof v === "string" ? v : undefined
  }

  /* ------------------------------------------------------------------ */
  /* Words and phrases                                                   */
  /* ------------------------------------------------------------------ */

  /** Lowercase whole words: camelCase and every non-alphanumeric split. */
  export function words(text: string): string[] {
    return text
      .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
      .toLowerCase()
      .split(/[^a-z0-9]+/)
      .filter(Boolean)
  }

  type Phrase = { words: string[]; topic: Topic }

  function phrases(topic: Topic, list: string[]): Phrase[] {
    return list.map((p) => ({ words: words(p), topic }))
  }

  /**
   * Control labels. At each word the longest phrase that matches is taken and
   * its words are consumed, so "Send money" is one money phrase and "Sign in"
   * one routine phrase; then the strictest topic found wins.
   */
  const LABEL_PHRASES: Phrase[] = [
    ...phrases("human_check", [
      "not a robot",
      "captcha",
      "recaptcha",
      "hcaptcha",
      "verify you are human",
      "verify you re human",
      "are you human",
      "i am human",
      "prove you re human",
      "prove you are human",
    ]),
    // Routine phrases that contain a stricter word.
    ...phrases("none", [
      "sign in",
      "sign out",
      "sign up",
      "sign on",
      "sign in with",
      "signin",
      "signout",
      "signup",
      "move to trash",
      "move to bin",
      "push notifications",
      "post code",
      "post office",
    ]),
    ...phrases("money", [
      "pay",
      "pay now",
      "send money",
      "send payment",
      "make payment",
      "make a payment",
      "confirm payment",
      "submit payment",
      "complete payment",
      "buy",
      "buy now",
      "purchase",
      "complete purchase",
      "confirm purchase",
      "checkout",
      "check out",
      "place order",
      "place your order",
      "confirm order",
      "submit order",
      "complete order",
      "subscribe",
      "start subscription",
      "transfer",
      "transfer money",
      "transfer funds",
      "wire",
      "wire transfer",
      "send invoice",
      "pay invoice",
      "payout",
      "request payout",
      "withdraw",
      "donate",
      "refund",
      "add funds",
      "top up",
    ]),
    ...phrases("credential", [
      "password",
      "passwords",
      "passcode",
      "change password",
      "reset password",
      "api key",
      "api keys",
      "apikey",
      "secret key",
      "access key",
      "access token",
      "personal access token",
      "private key",
      "client secret",
      "secret",
      "recovery codes",
      "credential",
      "credentials",
    ]),
    ...phrases("delete", [
      "delete",
      "delete permanently",
      "delete immediately",
      "delete forever",
      "permanently delete",
      "empty trash",
      "empty bin",
      "empty recycle bin",
      "erase",
      "destroy",
      "wipe",
      "purge",
      "delete account",
      "close account",
      "remove account",
      "deactivate account",
    ]),
    ...phrases("sign", ["sign", "sign document", "sign contract", "sign here", "adopt and sign", "esign", "e sign", "signature"]),
    ...phrases("send", ["send", "send now", "send message", "send email", "reply", "reply all", "forward"]),
    ...phrases("upload", ["upload", "upload file", "upload files"]),
    ...phrases("publish", [
      "publish",
      "post",
      "post comment",
      "tweet",
      "retweet",
      "repost",
      "merge",
      "merge pull request",
      "push",
      "go live",
      "make public",
    ]),
    ...phrases("deploy", ["deploy", "deploy now", "promote to production"]),
  ]

  const TOPIC_RANK = (t: Topic) => RANK[TOPIC_CLASS[t]]

  /** The strictest topic named in a label, by longest whole-word phrase match. */
  export function labelTopic(label: string): { topic: Topic; phrase?: string } {
    const w = words(label)
    let best: { topic: Topic; phrase?: string } = { topic: "none" }
    let i = 0
    while (i < w.length) {
      let hit: Phrase | undefined
      for (const p of LABEL_PHRASES) {
        if (p.words.length > w.length - i) continue
        if (hit && p.words.length <= hit.words.length) continue
        if (p.words.every((word, k) => w[i + k] === word)) hit = p
      }
      if (!hit) {
        i++
        continue
      }
      if (TOPIC_RANK(hit.topic) > TOPIC_RANK(best.topic)) best = { topic: hit.topic, phrase: hit.words.join(" ") }
      i += hit.words.length
    }
    return best
  }

  /* ------------------------------------------------------------------ */
  /* Secrets in text                                                     */
  /* ------------------------------------------------------------------ */

  const SECRET_LITERAL =
    /(\bsk[-_](?!learn\b)[A-Za-z0-9_<>-]{4,}|\b[pr]k_live_[A-Za-z0-9]{6,}|\bAKIA[0-9A-Z]{16}\b|\bgh[pousr]_[A-Za-z0-9]{20,}|\bgithub_pat_[A-Za-z0-9_]{20,}|\bxox[abprs]-[A-Za-z0-9-]{10,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|\bAIza[0-9A-Za-z_-]{30,})/
  const SECRET_ASSIGN =
    /\b([A-Za-z_][A-Za-z0-9_]*(?:API_?KEY|SECRET(?:_?KEY)?|ACCESS_?KEY|PRIVATE_?KEY|AUTH_?TOKEN|ACCESS_?TOKEN|_TOKEN|PASSWORD|PASSWD))\s*[=:]\s*['"]?([^\s'"]+)/i
  const PLACEHOLDER = /^(your|xxx|<your|changeme|placeholder|example|replace|todo|\.\.\.|\$\{?[A-Za-z_]+\}?$)/i

  /** Whether the text carries an API key, token, password or private key. */
  export function hasSecret(text: string | undefined): boolean {
    if (!text) return false
    if (SECRET_LITERAL.test(text)) return true
    const assign = SECRET_ASSIGN.exec(text)
    if (assign && assign[2] && !PLACEHOLDER.test(assign[2])) return true
    return false
  }

  /** Credential files, without the git-control paths the read/edit rules also ask about. */
  function credentialFile(p: string): boolean {
    const normalized = p.replaceAll("\\", "/")
    if (/(^|\/)\.git(\/|$)/.test(normalized)) return false
    return isCredentialPath(normalized)
  }

  /* ------------------------------------------------------------------ */
  /* File writes                                                         */
  /* ------------------------------------------------------------------ */

  function classifyWrite(filepath: string | undefined, text: string | undefined): Result {
    if (filepath && credentialFile(filepath)) return result("credential", `writes ${path.basename(filepath)}`)
    if (hasSecret(text)) return result("credential", "writes an API key or password to a file")
    return FREE
  }

  /* ------------------------------------------------------------------ */
  /* Computer, browser and pane_browser                                  */
  /* ------------------------------------------------------------------ */

  const READ_ACTIONS = new Set([
    "screenshot",
    "zoom",
    "read",
    "read_ui",
    "read_page",
    "find",
    "get_page_text",
    "get_text",
    "read_console",
    "list_tabs",
    "tabs",
    "tabs_context",
    "list_apps",
    "list_windows",
    "cursor_position",
    "mouse_move",
    "hover",
    "scroll",
    "scroll_to",
    "wait",
    "resolve_access",
    "navigate",
    "goto",
    "open_url",
    "go_back",
    "go_forward",
    "back",
    "forward",
    "reload",
    "new_tab",
    "switch_tab",
    "select_tab",
    "plan",
  ])
  const CREDENTIAL_ACTIONS = new Set(["use_credential", "fill_credential", "autofill_password"])
  const UPLOAD_ACTIONS = new Set(["file_upload", "upload_file", "upload"])
  const KEY_ACTIONS = new Set(["key", "key_press", "keypress", "press_key", "hotkey", "shortcut"])
  const TYPE_ACTIONS = new Set(["type", "type_text", "insert_text", "fill", "set_value", "input"])
  const TEXT_ROLES = /^(ax)?(text ?field|text ?box|text ?area|search ?box|search ?field|combo ?box|input|secure ?text ?field|textbox|searchbox|combobox|textarea)$/i

  const MODIFIERS = new Set(["cmd", "command", "meta", "super", "win", "ctrl", "control", "alt", "option", "opt", "shift"])

  /** Key chords: the delete chord and the send chord. Plain keys are typing. */
  function chordTopic(text: string): Topic {
    let topic: Topic = "none"
    for (const chord of text.split(/\s+/).filter(Boolean)) {
      const parts = chord.toLowerCase().split("+").filter(Boolean)
      const key = parts.filter((p) => !MODIFIERS.has(p)).pop() ?? ""
      const mods = new Set(parts.filter((p) => MODIFIERS.has(p)))
      const cmd = mods.has("cmd") || mods.has("command") || mods.has("meta") || mods.has("super")
      const ctrl = mods.has("ctrl") || mods.has("control")
      if ((key === "backspace" || key === "delete") && (cmd || mods.has("shift"))) return "delete"
      if ((key === "return" || key === "enter" || key === "kp_enter") && (cmd || ctrl)) topic = "send"
      if (key === "d" && cmd && mods.has("shift")) topic = "send"
    }
    return topic
  }

  function labelOf(args: Record<string, unknown>): string {
    const parts: string[] = []
    for (const key of ["label", "name", "title", "target"]) {
      const v = args[key]
      if (typeof v === "string") parts.push(v)
    }
    if (Array.isArray(args.path)) parts.push(args.path.filter((p) => typeof p === "string").join(" "))
    return parts.join(" ")
  }

  function quote(label: string): string {
    const short = label.trim().replace(/\s+/g, " ")
    return `'${short.length > 60 ? short.slice(0, 59) + "…" : short}'`
  }

  function classifyControl(action: string, args: Record<string, unknown>): Result {
    let verb = action
    if (verb === "act") {
      const op = (str(args.op) ?? "click").toLowerCase()
      verb = op === "set_value" || op === "type" || op === "fill" ? "type" : op === "key" ? "key" : op === "upload" ? "upload" : "click"
    }
    if (CREDENTIAL_ACTIONS.has(verb)) return result("credential", "fills a saved secret")
    if (verb === "request_access") return result("access", "asks for more control of an app")
    if (READ_ACTIONS.has(verb)) return FREE
    if (UPLOAD_ACTIONS.has(verb)) return result("upload", "sends a file to a site")

    const label = labelOf(args)
    const role = str(args.role) ?? ""
    const secure = args.secure === true || /secure|password/i.test(role)

    if (KEY_ACTIONS.has(verb)) {
      const topic = chordTopic(str(args.text) ?? str(args.key) ?? "")
      return topic === "none" ? FREE : result(topic, topic === "delete" ? "the delete chord" : "the send chord")
    }

    if (TYPE_ACTIONS.has(verb)) {
      if (secure) return result("credential", "types into a password field")
      if (hasSecret(str(args.text))) return result("credential", "types an API key or password")
      const hit = labelTopic(label)
      if (hit.topic === "credential") return result("credential", `types into ${quote(label)}`)
      if (/\b(card number|cvv|cvc|security code|expiry|expiration)\b/i.test(label)) return result("money", "types card details")
      return FREE
    }

    // Clicks and anything else that acts on a control.
    if (TEXT_ROLES.test(role.replace(/[_-]/g, " ").trim())) return FREE // focusing a field; typing is what's gated
    const hit = labelTopic(label)
    if (hit.topic === "none") return FREE
    return result(hit.topic, quote(label))
  }

  /* ------------------------------------------------------------------ */
  /* Tool names (MCP tools, other named tools)                           */
  /* ------------------------------------------------------------------ */

  const READ_VERBS = new Set([
    "list", "get", "read", "search", "find", "fetch", "query", "describe", "show", "view", "lookup", "count",
    "status", "check", "preview", "inspect", "retrieve", "download", "export", "summarize", "info",
  ])
  const MUTATING_VERBS = new Set([
    "create", "send", "pay", "finalize", "capture", "charge", "refund", "transfer", "issue", "submit", "confirm",
    "execute", "post", "make", "add", "update", "set", "write", "put", "reply", "forward", "upload", "publish",
    "deploy", "merge", "push", "delete", "remove", "destroy", "erase", "purge", "wipe", "trash", "unlink",
    "sign", "buy", "purchase", "subscribe", "wire", "void", "cancel", "approve", "release", "tweet", "share",
  ])
  const MONEY_WORDS = new Set([
    "pay", "payment", "payments", "payout", "payouts", "invoice", "invoices", "charge", "charges", "refund",
    "refunds", "transfer", "transfers", "purchase", "buy", "checkout", "subscribe", "subscription",
    "subscriptions", "wire", "order", "orders",
  ])
  const MONEY_VERB_WORDS = new Set(["pay", "charge", "refund", "transfer", "purchase", "buy", "checkout", "subscribe", "wire"])
  const CREDENTIAL_WORDS = new Set(["password", "passwords", "secret", "secrets", "credential", "credentials", "apikey", "apikeys"])
  const CREDENTIAL_PAIRS = [["api", "key"], ["api", "keys"], ["access", "key"], ["access", "token"], ["auth", "token"], ["private", "key"], ["secret", "key"]]
  const DELETE_VERBS = new Set(["delete", "remove", "destroy", "erase", "purge", "wipe", "trash", "unlink"])
  const SEND_VERBS = new Set(["send", "reply", "forward"])
  const PUBLISH_VERBS = new Set(["publish", "post", "tweet", "merge", "push"])

  /** A tool known only by its name (an MCP tool such as `stripe_send_invoice`). */
  export function classifyName(name: string): Result {
    const w = words(name)
    if (!w.length) return FREE
    const set = new Set(w)
    const has = (s: Set<string>) => w.some((x) => s.has(x))
    const pair = CREDENTIAL_PAIRS.some(([a, b]) => w.some((x, i) => x === a && w[i + 1] === b))
    if (has(CREDENTIAL_WORDS) || pair) return result("credential", `'${name}'`)
    const mutating = has(MUTATING_VERBS) || has(MONEY_VERB_WORDS)
    if (!mutating) return FREE // a read (list_*, get_*) or no recognisable verb
    if (has(MONEY_WORDS) && (has(MUTATING_VERBS) || has(MONEY_VERB_WORDS))) return result("money", `'${name}'`)
    if (has(DELETE_VERBS)) return result("delete", `'${name}'`)
    if (set.has("sign") && !(set.has("in") || set.has("out") || set.has("up"))) return result("sign", `'${name}'`)
    if (has(SEND_VERBS)) return result("send", `'${name}'`)
    if (set.has("upload")) return result("upload", `'${name}'`)
    if (has(PUBLISH_VERBS)) return result("publish", `'${name}'`)
    if (set.has("deploy")) return result("deploy", `'${name}'`)
    return FREE
  }

  /* ------------------------------------------------------------------ */
  /* Shell commands                                                      */
  /* ------------------------------------------------------------------ */

  /** Split a command into simple commands (words), honouring quotes. Rough on purpose: the floor has the strict parser. */
  function segments(command: string): string[][] {
    const out: string[][] = []
    let words: string[] = []
    let word = ""
    let quote: string | undefined
    let inWord = false
    const endWord = () => {
      if (inWord) words.push(word)
      word = ""
      inWord = false
    }
    const endSegment = () => {
      endWord()
      if (words.length) out.push(words)
      words = []
    }
    for (let i = 0; i < command.length; i++) {
      const c = command[i]
      if (quote) {
        if (c === quote) quote = undefined
        else if (c === "\\" && quote === '"' && i + 1 < command.length) word += command[++i]
        else word += c
        continue
      }
      if (c === "'" || c === '"') {
        quote = c
        inWord = true
        continue
      }
      if (c === "\\" && i + 1 < command.length) {
        word += command[++i]
        inWord = true
        continue
      }
      if (c === ";" || c === "\n" || c === "|" || c === "&" || c === "(" || c === ")" || c === "`") {
        endSegment()
        continue
      }
      if (/\s/.test(c)) {
        endWord()
        continue
      }
      if (c === "$" && command[i + 1] === "(") {
        endSegment()
        i++
        continue
      }
      word += c
      inWord = true
    }
    endSegment()
    return out
  }

  const WRAPPERS = new Set(["sudo", "env", "command", "nohup", "time", "nice", "exec", "xargs", "doas"])

  function unwrap(raw: string[]): string[] {
    let w = raw
    for (let guard = 0; guard < 8 && w.length; guard++) {
      const head = path.basename(w[0])
      if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(w[0])) {
        w = w.slice(1)
        continue
      }
      if (WRAPPERS.has(head)) {
        w = w.slice(1)
        while (w.length && (w[0].startsWith("-") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(w[0]))) w = w.slice(1)
        continue
      }
      if (head === "npx" || head === "bunx") {
        w = w.slice(1)
        while (w.length && w[0].startsWith("-")) w = w.slice(1)
        continue
      }
      if ((head === "pnpm" || head === "yarn" || head === "bun") && ["dlx", "exec", "x"].includes(w[1] ?? "")) {
        w = w.slice(2)
        continue
      }
      break
    }
    return w
  }

  const GIT_VALUE_OPTS = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"])
  const DEPLOY_PROGRAMS: Record<string, (args: string[]) => boolean> = {
    wrangler: (a) => a.includes("deploy") || a.includes("publish"),
    vercel: (a) => a.length === 0 || a.includes("deploy") || a.includes("--prod"),
    netlify: (a) => a.includes("deploy"),
    fly: (a) => a.includes("deploy"),
    flyctl: (a) => a.includes("deploy"),
    firebase: (a) => a.includes("deploy"),
    railway: (a) => a[0] === "up",
    serverless: (a) => a.includes("deploy"),
    sls: (a) => a.includes("deploy"),
    terraform: (a) => a[0] === "apply",
    tofu: (a) => a[0] === "apply",
    pulumi: (a) => a[0] === "up",
    cdk: (a) => a[0] === "deploy",
    sam: (a) => a[0] === "deploy",
    gcloud: (a) => a.includes("deploy"),
    kubectl: (a) => a[0] === "apply" || a[0] === "rollout",
    helm: (a) => a[0] === "install" || a[0] === "upgrade",
  }
  const PUBLISH_PROGRAMS: Record<string, (args: string[]) => boolean> = {
    npm: (a) => a[0] === "publish",
    pnpm: (a) => a[0] === "publish",
    yarn: (a) => a[0] === "publish" || (a[0] === "npm" && a[1] === "publish"),
    bun: (a) => a[0] === "publish",
    cargo: (a) => a[0] === "publish",
    gem: (a) => a[0] === "push",
    twine: (a) => a[0] === "upload",
    poetry: (a) => a[0] === "publish",
    docker: (a) => a[0] === "push",
    podman: (a) => a[0] === "push",
    vsce: (a) => a[0] === "publish",
    ovsx: (a) => a[0] === "publish",
    gh: (a) => (a[0] === "pr" && a[1] === "merge") || (a[0] === "release" && a[1] === "create"),
  }
  const SEND_PROGRAMS = new Set(["mail", "mailx", "sendmail", "msmtp", "mutt"])
  const PAYMENT_HOSTS = /(api\.stripe\.com|api\.paypal\.com|api-m\.paypal\.com|connect\.squareup\.com|api\.braintreegateway\.com|api\.wise\.com|api\.coinbase\.com)/i
  const STRIPE_MONEY_VERBS = new Set(["create", "pay", "finalize", "capture", "confirm", "send", "void", "refund"])
  const TEMP_DIRS = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"]

  function isRemote(arg: string | undefined): boolean {
    if (!arg) return false
    if (arg.startsWith("/") || arg.startsWith("./") || arg.startsWith("../") || arg.startsWith("~")) return false
    return /^[^\s/:]+:/.test(arg)
  }

  function lastOperand(args: string[]): string | undefined {
    return args.filter((a) => !a.startsWith("-")).pop()
  }

  function outsideProject(arg: string, context: Context): boolean {
    const home = context.home ?? os.homedir()
    let p = arg
    if (p === "~" || p.startsWith("~/")) p = home + p.slice(1)
    p = p.replace(/^\$\{?HOME\}?/, home)
    if (/^\$\{?TMPDIR\}?/.test(p)) return false
    if (p.startsWith("$")) return true // an unknown variable: can't tell, the stricter answer
    const tmp = os.tmpdir()
    const inside = (abs: string) => {
      if ([...TEMP_DIRS, tmp].some((t) => abs === t || abs.startsWith(t + "/"))) return true
      if (!context.projectRoot) return false
      const root = path.resolve(context.projectRoot)
      return abs === root || abs.startsWith(root + path.sep)
    }
    if (path.isAbsolute(p)) return !inside(path.resolve(p))
    if (p === ".." || p.startsWith("../")) {
      if (!context.projectRoot) return true
      return !inside(path.resolve(context.projectRoot, p))
    }
    return false
  }

  function shellSegment(raw: string[], context: Context, depth: number): Result {
    const w = unwrap(raw)
    if (!w.length) return FREE
    const prog = path.basename(w[0])
    const args = w.slice(1)

    if (["sh", "bash", "zsh", "dash", "ksh", "fish"].includes(prog)) {
      const c = args.findIndex((a) => a === "-c" || (/^-[a-z]*c[a-z]*$/.test(a) && !a.startsWith("--")))
      if (c >= 0 && args[c + 1] !== undefined && depth < 4) return shellCommand(args[c + 1], context, depth + 1)
      return FREE
    }
    if (prog === "eval" && depth < 4) return shellCommand(args.join(" "), context, depth + 1)

    // Credentials: keychain, password managers, credential files.
    if (prog === "security") return result("credential", "reads or writes the keychain")
    if (prog === "op" && ["read", "item", "inject", "run"].includes(args[0] ?? "")) return result("credential", "reads a saved secret")
    if (prog === "pass" && (args[0] === "show" || (args[0] && !args[0].startsWith("-")))) return result("credential", "reads a saved secret")
    if (prog === "gh" && args[0] === "auth" && args[1] === "token") return result("credential", "prints a GitHub token")
    const credFile = w.find((x) => credentialFile(x))
    if (credFile) return result("credential", `touches ${path.basename(credFile)}`)

    // Money.
    if (prog === "stripe" && args.some((a) => STRIPE_MONEY_VERBS.has(a))) return result("money", `stripe ${args.slice(0, 2).join(" ")}`)
    if ((prog === "curl" || prog === "wget" || prog === "http" || prog === "xh") && args.some((a) => PAYMENT_HOSTS.test(a))) {
      const writes = args.some((a) => /^(-X|--request)$/.test(a) || /^-X(POST|PUT|PATCH|DELETE)$/i.test(a) || /^(-d|--data.*|-F|--form|--json)$/.test(a) || /^(POST|PUT|PATCH|DELETE)$/i.test(a))
      if (writes) return result("money", "calls a payments API")
    }

    // Permanent delete outside the project.
    if (Catastrophic.deletes(w.join(" "))) {
      const target = args.find((a) => !a.startsWith("-") && outsideProject(a, context))
      if (target) return result("delete", `removes ${target} outside the project`)
    }

    // Send, upload, publish, deploy.
    if (SEND_PROGRAMS.has(prog)) return result("send", `${prog} sends mail`)
    if (prog === "git") {
      let k = 0
      while (k < args.length && args[k].startsWith("-")) k += GIT_VALUE_OPTS.has(args[k]) ? 2 : 1
      if (args[k] === "push") return result("publish", "git push")
    }
    const publish = PUBLISH_PROGRAMS[prog]
    if (publish?.(args)) return result("publish", `${prog} ${args.slice(0, 2).join(" ")}`.trim())
    const deploy = DEPLOY_PROGRAMS[prog]
    if (deploy?.(args)) return result("deploy", `${prog} ${args.slice(0, 2).join(" ")}`.trim())
    if ((prog === "scp" || prog === "rsync") && isRemote(lastOperand(args))) return result("upload", `${prog} to another machine`)
    if (prog === "aws" && args[0] === "s3" && ["cp", "mv", "sync"].includes(args[1] ?? "") && (lastOperand(args) ?? "").startsWith("s3://"))
      return result("upload", "copies files to S3")
    if (prog === "gsutil" && ["cp", "mv", "rsync"].includes(args[0] ?? "") && (lastOperand(args) ?? "").startsWith("gs://"))
      return result("upload", "copies files to Cloud Storage")
    if (prog === "rclone" && ["copy", "copyto", "move", "moveto", "sync"].includes(args[0] ?? "") && isRemote(lastOperand(args)))
      return result("upload", "copies files to a remote")
    if (prog === "curl" && args.some((a, i) => a === "-T" || a === "--upload-file" || ((a === "-F" || a === "--form") && (args[i + 1] ?? "").includes("@"))))
      return result("upload", "sends a file with curl")
    return FREE
  }

  function shellCommand(command: string, context: Context, depth: number): Result {
    let best: Result = FREE
    for (const raw of segments(command)) best = stricter(best, shellSegment(raw, context, depth))
    return best
  }

  function classifyShell(command: string, context: Context): Result {
    const floor = Catastrophic.check(command, { home: context.home })
    if (floor) return { class: "hand_off", topic: "disk", reason: `Never runs: ${floor.reason}` }
    let best = shellCommand(command, context, 0)
    if (RANK[best.class] < RANK.confirm_now && hasSecret(command)) best = result("credential", "handles an API key or password")
    return best
  }
}
