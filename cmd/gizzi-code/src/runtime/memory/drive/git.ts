/**
 * argv-only git runner for Memory Drive checkouts.
 *
 * - Never a shell string: git is spawned with an argument vector.
 * - Credentials never appear in argv or a URL. When a token is supplied it is
 *   passed in the child's environment (ALLTERNIT_MEMORY_TOKEN) and read by a
 *   one-shot credential helper given with `-c`; every configured helper is
 *   cleared first so the token is never stored in a keychain or file.
 * - Hooks are disabled, interactive prompts are off, and inherited GIT_DIR /
 *   GIT_WORK_TREE / GIT_INDEX_FILE / askpass variables are dropped.
 */
import path from "path"

export interface GitAuthor {
  name: string
  email: string
}

export interface GitRunOptions {
  cwd: string
  token?: string
  input?: string | Uint8Array
  env?: Record<string, string>
  author?: GitAuthor
  /** Network operations get a longer budget than local plumbing. */
  timeoutMs?: number
}

export interface GitResult {
  code: number
  stdout: string
  stderr: string
  stdoutBytes: Uint8Array
  timedOut: boolean
}

/** Credential helper: answers `get` with the token from the child env; ignores store/erase. */
const CREDENTIAL_HELPER =
  '!f() { test "$1" = get || exit 0; echo username=x-access-token; echo "password=$ALLTERNIT_MEMORY_TOKEN"; }; f'

const DROPPED_ENV = [
  "GIT_DIR",
  "GIT_WORK_TREE",
  "GIT_INDEX_FILE",
  "GIT_OBJECT_DIRECTORY",
  "GIT_ALTERNATE_OBJECT_DIRECTORIES",
  "GIT_ASKPASS",
  "SSH_ASKPASS",
  "GIT_CONFIG_PARAMETERS",
  "GIT_CONFIG_COUNT",
  "ALLTERNIT_MEMORY_TOKEN",
]

export class GitUnavailableError extends Error {
  constructor(cause: unknown) {
    super(
      `Memory Drive needs git on PATH (${cause instanceof Error ? cause.message : String(cause)}).`,
    )
    this.name = "GitUnavailableError"
  }
}

export async function runGit(args: string[], options: GitRunOptions): Promise<GitResult> {
  const env: Record<string, string> = {}
  for (const [k, v] of Object.entries(process.env)) {
    if (v !== undefined && !DROPPED_ENV.includes(k)) env[k] = v
  }
  env.GIT_TERMINAL_PROMPT = "0"
  env.GCM_INTERACTIVE = "never"
  env.LC_ALL = "C"
  if (options.author) {
    env.GIT_AUTHOR_NAME = options.author.name
    env.GIT_AUTHOR_EMAIL = options.author.email
    env.GIT_COMMITTER_NAME = options.author.name
    env.GIT_COMMITTER_EMAIL = options.author.email
  }
  if (options.token) env.ALLTERNIT_MEMORY_TOKEN = options.token
  Object.assign(env, options.env ?? {})

  const config = [
    "-c",
    `core.hooksPath=${path.join(options.cwd, ".git", "allternit-no-hooks")}`,
    "-c",
    "credential.helper=",
    ...(options.token ? ["-c", `credential.helper=${CREDENTIAL_HELPER}`] : []),
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.autocrlf=false",
    "-c",
    "core.quotePath=false",
    "-c",
    "gc.auto=0",
    "-c",
    "init.defaultBranch=main",
  ]

  const began = performance.now()
  let proc: ReturnType<typeof Bun.spawn>
  try {
    proc = Bun.spawn(["git", ...config, ...args], {
      cwd: options.cwd,
      env,
      stdin: options.input === undefined ? "ignore" : "pipe",
      stdout: "pipe",
      stderr: "pipe",
    })
  } catch (error) {
    throw new GitUnavailableError(error)
  }
  if (options.input !== undefined && proc.stdin && typeof proc.stdin !== "number") {
    const sink = proc.stdin as import("bun").FileSink
    sink.write(options.input)
    await sink.end()
  }
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    proc.kill()
  }, options.timeoutMs ?? 15_000)
  try {
    const [out, err, code] = await Promise.all([
      new Response(proc.stdout as ReadableStream).arrayBuffer(),
      new Response(proc.stderr as ReadableStream).text(),
      proc.exited,
    ])
    const stdoutBytes = new Uint8Array(out)
    if (process.env.GIZZI_MEMORY_DRIVE_TRACE) console.error(`[drive-git] ${(performance.now() - began).toFixed(0)}ms git ${args.slice(0, 3).join(" ")}`)
    return {
      code: timedOut ? 124 : code,
      stdout: new TextDecoder().decode(stdoutBytes).trim(),
      stderr: err.trim(),
      stdoutBytes,
      timedOut,
    }
  } finally {
    clearTimeout(timer)
  }
}

/** Remove anything that could carry a credential before a git message reaches a user. */
export function redactGitOutput(text: string): string {
  return text
    .replace(/https?:\/\/[^\s/@]+@/g, "https://")
    .replace(/(password|token)=\S+/gi, "$1=…")
    .split("\n")
    .filter((l) => l.trim())
    .slice(-6)
    .join("\n")
}
