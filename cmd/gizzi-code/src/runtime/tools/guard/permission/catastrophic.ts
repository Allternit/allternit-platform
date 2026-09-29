import os from "os"
import path from "path"

/**
 * Hard catastrophic floor for shell commands.
 *
 * A short list of commands that no mode can approve — not bypassPermissions,
 * not GIZZI_SKIP_PERMISSIONS, not yolo, not a configured `allow`. They destroy
 * the machine, the user's home, a shared branch, or dump credentials, and none
 * of them is ever the right move for an agent to make unattended:
 *
 *   - recursive `rm` of `/`, `/*`, `~`, `$HOME` (and the literal home path)
 *   - `mkfs*` and `diskutil erase…` (formatting a disk)
 *   - `dd of=/dev/disk*` / `/dev/rdisk*` / `/dev/sd*` / `/dev/nvme*` …
 *   - fork bombs
 *   - shutdown / reboot / halt / poweroff
 *   - a force push (`--force`, `-f`, `--force-with-lease`, `+ref`, `--mirror`)
 *     to main or master — or a force push whose target branch isn't named
 *   - `security find-generic-password` / `find-internet-password` /
 *     `dump-keychain` (macOS keychain reads)
 *
 * It sees through the usual wrappers (`sudo`, `env`, `command`, `nohup`,
 * `xargs`, `VAR=x …`, `bash -c "…"`, `eval`, `$(…)`) and through quoting
 * (`rm -rf "$HOME"`, `\rm -rf /`). A command it can't parse (unbalanced
 * quotes, an unclosed substitution — or a tree-sitter parse error reported by
 * the bash tool) is denied when it mentions any guarded program, so a parser
 * gap can't be used to slip one through.
 *
 * `evaluatePolicy` consults this before the bypass early-return. It is
 * synchronous on purpose: `evaluatePolicy` is synchronous, and tree-sitter's
 * parser only loads asynchronously, so the floor carries its own small POSIX
 * word splitter. The bash tool additionally runs the floor over the whole
 * command (not just each tree-sitter `command` node) and passes tree-sitter's
 * `hasError` through as `parseError`.
 */
export namespace Catastrophic {
  export interface Verdict {
    reason: string
  }

  export interface CheckOptions {
    /** The caller's own parser (tree-sitter) failed on this command. */
    parseError?: boolean
    /** Override for tests; defaults to os.homedir(). */
    home?: string
  }

  /** Programs whose presence in an unparseable command means deny. */
  const GUARDED =
    /(^|[^\w.-])(rm|mkfs(\.\w+)?|newfs\w*|dd|diskutil|shutdown|reboot|halt|poweroff|systemctl|security)([^\w.-]|$)|(^|[^\w.-])git([^\w.-]).*\bpush\b|(^|[^\w.-])init\s+[06]\b/
  const FORK_BOMB = /([\w:.-]+)\s*\(\s*\)\s*\{[^}]*?\1\s*\|\s*&?\s*\1\s*&/

  export function check(command: string, options: CheckOptions = {}): Verdict | undefined {
    return inspect(command, options.home ?? os.homedir(), options.parseError ?? false, 0)
  }

  /** True when this permission's patterns are shell commands the floor guards. */
  export function applies(permission: string): boolean {
    return permission === "bash"
  }

  function inspect(command: string, home: string, parseError: boolean, depth: number): Verdict | undefined {
    if (depth > 4) return { reason: "command nests shells too deeply to inspect" }
    if (FORK_BOMB.test(command)) return { reason: "fork bomb" }

    const parsed = split(command)
    if (parsed.error || parseError) {
      if (GUARDED.test(command) || /\(\s*\)\s*\{/.test(command)) {
        return { reason: "command could not be parsed and touches a guarded operation" }
      }
      if (parsed.error) return
    }

    for (const sub of parsed.substitutions) {
      const verdict = inspect(sub, home, false, depth + 1)
      if (verdict) return verdict
    }
    for (const words of parsed.commands) {
      const verdict = simple(words, home, depth)
      if (verdict) return verdict
    }
    return
  }

  // ── Word splitting ─────────────────────────────────────────────────────

  interface Parsed {
    commands: string[][]
    substitutions: string[]
    error: boolean
  }

  /**
   * Split a shell command into simple commands of unquoted words. Handles
   * single/double quotes, backslash escapes, `$(…)`, backticks, `${…}`, and
   * the control operators ; & | && || ( ) { } and newlines. Redirections are
   * dropped (`>file`, `2>&1`). Substitution bodies are returned separately so
   * the caller can inspect them as commands of their own.
   */
  function split(input: string): Parsed {
    const commands: string[][] = []
    const substitutions: string[] = []
    let words: string[] = []
    let word = ""
    let inWord = false
    let i = 0

    const endWord = () => {
      if (inWord) words.push(word)
      word = ""
      inWord = false
    }
    const endCommand = () => {
      endWord()
      if (words.length) commands.push(words)
      words = []
    }
    /** Read a balanced `$(…)` body starting after the opening paren. */
    const readParen = (start: number): number => {
      let level = 1
      let j = start
      let quote: string | undefined
      while (j < input.length) {
        const c = input[j]
        if (quote) {
          if (c === "\\" && quote === '"') j++
          else if (c === quote) quote = undefined
        } else if (c === "\\") j++
        else if (c === "'" || c === '"') quote = c
        else if (c === "(") level++
        else if (c === ")") {
          level--
          if (level === 0) return j
        }
        j++
      }
      return -1
    }

    while (i < input.length) {
      const c = input[i]
      if (c === "\\") {
        if (input[i + 1] === "\n") {
          i += 2
          continue
        }
        if (i + 1 >= input.length) return { commands, substitutions, error: true }
        word += input[i + 1]
        inWord = true
        i += 2
        continue
      }
      if (c === "'") {
        const end = input.indexOf("'", i + 1)
        if (end < 0) return { commands, substitutions, error: true }
        word += input.slice(i + 1, end)
        inWord = true
        i = end + 1
        continue
      }
      if (c === '"') {
        let j = i + 1
        let closed = false
        while (j < input.length) {
          const d = input[j]
          if (d === "\\" && j + 1 < input.length && '$`"\\\n'.includes(input[j + 1])) {
            word += input[j + 1]
            j += 2
            continue
          }
          if (d === '"') {
            closed = true
            break
          }
          if (d === "$" && input[j + 1] === "(") {
            const end = readParen(j + 2)
            if (end < 0) return { commands, substitutions, error: true }
            substitutions.push(input.slice(j + 2, end))
            word += input.slice(j, end + 1)
            j = end + 1
            continue
          }
          if (d === "`") {
            const end = input.indexOf("`", j + 1)
            if (end < 0) return { commands, substitutions, error: true }
            substitutions.push(input.slice(j + 1, end))
            word += input.slice(j, end + 1)
            j = end + 1
            continue
          }
          word += d
          j++
        }
        if (!closed) return { commands, substitutions, error: true }
        inWord = true
        i = j + 1
        continue
      }
      if (c === "$" && input[i + 1] === "(") {
        const end = readParen(i + 2)
        if (end < 0) return { commands, substitutions, error: true }
        substitutions.push(input.slice(i + 2, end))
        word += input.slice(i, end + 1)
        inWord = true
        i = end + 1
        continue
      }
      if (c === "$" && input[i + 1] === "{") {
        const end = input.indexOf("}", i + 2)
        if (end < 0) return { commands, substitutions, error: true }
        word += input.slice(i, end + 1)
        inWord = true
        i = end + 1
        continue
      }
      if (c === "`") {
        const end = input.indexOf("`", i + 1)
        if (end < 0) return { commands, substitutions, error: true }
        substitutions.push(input.slice(i + 1, end))
        word += input.slice(i, end + 1)
        inWord = true
        i = end + 1
        continue
      }
      if (c === "#" && !inWord) {
        const end = input.indexOf("\n", i)
        i = end < 0 ? input.length : end
        continue
      }
      if (c === " " || c === "\t") {
        endWord()
        i++
        continue
      }
      // `{`/`}` group commands only as words of their own; inside a word they
      // are brace expansion (`rm -rf /tmp/{a,b}`), not control operators.
      if ((c === "{" || c === "}") && (inWord || !(i + 1 >= input.length || /[\s;&|)]/.test(input[i + 1])))) {
        word += c
        inWord = true
        i++
        continue
      }
      if (";&|\n(){}".includes(c)) {
        // `2>&1`, `&>file`, `>&2` are redirections, not control operators.
        if (c === "&" && (input[i - 1] === ">" || input[i - 1] === "<" || input[i + 1] === ">")) {
          word += c
          inWord = true
          i++
          continue
        }
        endCommand()
        i++
        continue
      }
      word += c
      inWord = true
      i++
    }
    endCommand()
    return { commands: commands.map(stripRedirects).filter((w) => w.length > 0), substitutions, error: false }
  }

  function stripRedirects(words: string[]): string[] {
    const out: string[] = []
    for (let k = 0; k < words.length; k++) {
      const w = words[k]
      if (/^\d*(>>?|<<?<?|&>>?|>&|<&|>\|)$/.test(w)) {
        k++ // operator alone: skip its target too
        continue
      }
      if (/^\d*(>>?|<<?<?|&>>?|>&|<&|>\|)/.test(w)) continue
      out.push(w)
    }
    return out
  }

  // ── Simple-command rules ──────────────────────────────────────────────

  const ASSIGNMENT = /^[A-Za-z_][A-Za-z0-9_]*=/

  /** Wrappers whose remaining words are another command, with flags that take a value. */
  const WRAPPERS: Record<string, Set<string>> = {
    sudo: new Set(["-u", "-g", "-C", "-p", "-h", "-U", "-r", "-t", "-T", "-D", "-R"]),
    doas: new Set(["-u", "-C"]),
    env: new Set(["-u", "-C", "-S", "--unset", "--chdir"]),
    command: new Set(),
    builtin: new Set(),
    exec: new Set(["-a"]),
    nohup: new Set(),
    time: new Set(["-f", "-o"]),
    nice: new Set(["-n"]),
    ionice: new Set(["-c", "-n", "-p"]),
    timeout: new Set(["-s", "-k", "--signal", "--kill-after"]),
    xargs: new Set(["-I", "-L", "-n", "-P", "-s", "-d", "-E", "-a"]),
    stdbuf: new Set(["-i", "-o", "-e"]),
    caffeinate: new Set(["-t", "-w"]),
    watch: new Set(["-n", "-d"]),
  }

  function basename(word: string): string {
    return path.posix.basename(word)
  }

  /** Drop assignments and wrapper programs; return the real command's words. */
  function unwrap(words: string[]): string[] {
    let rest = words
    for (let guard = 0; guard < 16 && rest.length; guard++) {
      while (rest.length && ASSIGNMENT.test(rest[0])) rest = rest.slice(1)
      if (!rest.length) return rest
      const name = basename(rest[0])
      const takesValue = WRAPPERS[name]
      if (!takesValue) return rest
      let k = 1
      // timeout's first positional is the duration.
      let positionalToSkip = name === "timeout" ? 1 : 0
      while (k < rest.length) {
        const w = rest[k]
        if (w === "--") {
          k++
          break
        }
        if (name === "env" && ASSIGNMENT.test(w)) {
          k++
          continue
        }
        if (w.startsWith("-") && w.length > 1) {
          k += takesValue.has(w) ? 2 : 1
          continue
        }
        if (positionalToSkip > 0) {
          positionalToSkip--
          k++
          continue
        }
        break
      }
      rest = rest.slice(k)
    }
    return rest
  }

  function simple(raw: string[], home: string, depth: number): Verdict | undefined {
    const words = unwrap(raw)
    if (!words.length) return
    const name = basename(words[0])
    const args = words.slice(1)

    // Shells and eval: inspect the string they will run.
    if (["sh", "bash", "zsh", "dash", "ksh", "fish"].includes(name)) {
      const c = args.findIndex((a) => a === "-c" || (/^-[a-z]*c[a-z]*$/.test(a) && !a.startsWith("--")))
      if (c >= 0 && args[c + 1] !== undefined) return inspect(args[c + 1], home, false, depth + 1)
      return
    }
    if (name === "eval") return inspect(args.join(" "), home, false, depth + 1)

    if (name === "rm") return rm(args, home)
    if (/^mkfs(\..+)?$/.test(name) || /^newfs(_\w+)?$/.test(name)) return { reason: `${name} formats a filesystem` }
    if (name === "diskutil" && args.some((a) => /^(erase|zero|secureErase|partitionDisk|reformat)/i.test(a))) {
      return { reason: "diskutil erase/partition wipes a disk" }
    }
    if (name === "dd") {
      const target = args.find((a) => a.startsWith("of="))
      if (target && /^of=\/dev\/(r?disk|sd|hd|nvme|mmcblk|xvd|vd)/.test(target)) {
        return { reason: "dd onto a raw disk device" }
      }
      return
    }
    if (["shutdown", "reboot", "halt", "poweroff"].includes(name)) return { reason: `${name} stops the machine` }
    if (name === "systemctl" && args.some((a) => ["poweroff", "reboot", "halt", "kexec"].includes(a))) {
      return { reason: "systemctl power action stops the machine" }
    }
    if (name === "init" && (args[0] === "0" || args[0] === "6")) return { reason: "init runlevel change stops the machine" }
    if (name === "git") return git(args)
    if (name === "security") {
      const sub = args.find((a) => !a.startsWith("-"))
      if (sub && ["find-generic-password", "find-internet-password", "dump-keychain"].includes(sub)) {
        return { reason: `security ${sub} reads keychain secrets` }
      }
    }
    return
  }

  function rm(args: string[], home: string): Verdict | undefined {
    let recursive = false
    const targets: string[] = []
    let flagsDone = false
    for (const a of args) {
      if (!flagsDone && a === "--") {
        flagsDone = true
        continue
      }
      if (!flagsDone && a.startsWith("--")) {
        if (a === "--recursive") recursive = true
        continue
      }
      if (!flagsDone && a.startsWith("-") && a.length > 1) {
        if (/[rR]/.test(a.slice(1))) recursive = true
        continue
      }
      targets.push(a)
    }
    if (!recursive) return
    for (const t of targets) {
      if (isCatastrophicTarget(t, home)) return { reason: `recursive rm of ${t}` }
    }
    return
  }

  function isCatastrophicTarget(target: string, home: string): boolean {
    const bareHome = home.replace(/\/+$/, "")
    let t = target.replace(/^\$\{HOME\}/, "$HOME").replace(/^\$HOME(?=\/|$)/, "~")
    if (bareHome.length > 1 && (t === bareHome || t.startsWith(bareHome + "/"))) t = "~" + t.slice(bareHome.length)
    // Normalize "//", "/./", "/..", then strip trailing "/" and "/*" so "/",
    // "//", "/*", "/tmp/..", "~/", "~/*" and "~/." all read as their root.
    if (t.startsWith("/")) t = path.posix.normalize(t)
    else if (t === "~" || t.startsWith("~/")) t = "~" + path.posix.normalize("/" + t.slice(2)).replace(/^\/$/, "")
    while (t.length > 1 && (t.endsWith("/") || t.endsWith("/*"))) t = t.slice(0, t.endsWith("/*") ? -2 : -1) || "/"
    return t === "/" || t === "~"
  }

  function escape(s: string): string {
    return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
  }

  /** Global git options that take a value (the value is a separate word). */
  const GIT_VALUE_OPTS = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path", "--config-env"])

  function git(args: string[]): Verdict | undefined {
    let k = 0
    while (k < args.length && args[k].startsWith("-")) {
      k += GIT_VALUE_OPTS.has(args[k]) ? 2 : 1
    }
    if (args[k] !== "push") return
    const rest = args.slice(k + 1)
    let force = false
    let mirrorOrAll = false
    let del = false
    const positional: string[] = []
    const PUSH_VALUE_OPTS = new Set(["--repo", "--receive-pack", "--exec", "-o", "--push-option", "--signed"])
    for (let j = 0; j < rest.length; j++) {
      const a = rest[j]
      if (a === "--") {
        positional.push(...rest.slice(j + 1))
        break
      }
      if (a.startsWith("--")) {
        if (a === "--force" || a.startsWith("--force-with-lease") || a === "--force-if-includes") force = true
        else if (a === "--mirror") {
          force = true
          mirrorOrAll = true
        } else if (a === "--all" || a === "--branches") mirrorOrAll = true
        else if (a === "--delete") del = true
        else if (PUSH_VALUE_OPTS.has(a)) j++
        continue
      }
      if (a.startsWith("-") && a.length > 1) {
        if (a.slice(1).includes("f")) force = true
        if (a.slice(1).includes("d")) del = true
        if (a === "-o") j++
        continue
      }
      positional.push(a)
    }
    const refspecs = positional.slice(1)
    for (const spec of refspecs) {
      const plus = spec.startsWith("+")
      const body = plus ? spec.slice(1) : spec
      const colon = body.lastIndexOf(":")
      const dst = (colon >= 0 ? body.slice(colon + 1) : body).replace(/^refs\/heads\//, "")
      // HEAD / @ is whatever is checked out — possibly main.
      if ((force || plus) && (dst === "HEAD" || dst === "@")) {
        return { reason: "force push of HEAD may target main/master — name the branch explicitly" }
      }
      const protectedBranch = dst === "main" || dst === "master"
      if (!protectedBranch) continue
      if (force || plus) return { reason: `force push to ${dst}` }
      if (del || (colon === 0 && !plus)) return { reason: `deleting ${dst} on the remote` }
    }
    if (force && mirrorOrAll) return { reason: "force push of every branch (--mirror/--all) includes main/master" }
    if (force && refspecs.length === 0) {
      return { reason: "force push without a named branch may target main/master — name the branch explicitly" }
    }
    return
  }
}
