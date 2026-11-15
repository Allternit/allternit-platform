// Content-artifact client commands (A:// Artifacts API Phase 2,
// docs/design/artifacts-api.md §5 "Code"): list/read/create renderable
// artifacts on the local gateway (allternit-api, :8013) from the CLI.
// Named `artifact` per the design doc; distinct from `html-artifact`
// (allternit-api canvas publish) and the `/artifact` TUI markdown viewer.
import type { Argv } from "yargs"
import { randomUUID } from "node:crypto"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { Filesystem } from "@/runtime/util/filesystem"
import {
  createApiContentArtifact,
  getAllternitApiConfigWithDeviceToken,
  getApiContentArtifact,
  getApiContentArtifactVersion,
  listApiContentArtifacts,
} from "@/runtime/services/api/allternitApi"
import { artifactShareStatus, shareArtifact, unshareArtifact } from "@/cli/work/api"

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = []
  for await (const chunk of process.stdin) chunks.push(chunk as Buffer)
  return Buffer.concat(chunks).toString("utf8")
}

function printArtifactRow(a: {
  id: string
  title?: string
  type?: string
  version?: number
  projectId?: string
  updatedAt?: string
}): void {
  const title = a.title ?? "Untitled"
  const version = a.version != null ? ` v${a.version}` : ""
  const updated = a.updatedAt ? ` — ${a.updatedAt}` : ""
  process.stdout.write(`${a.id}${version}  [${a.type ?? "text/html"}]  ${title}${updated}\n`)
  process.stdout.write(`  a://artifact/${a.id}${a.projectId ? `  (project ${a.projectId})` : ""}\n`)
}

export const ArtifactListCommand = cmd({
  command: "list",
  describe: "list content artifacts on the local gateway",
  builder: (yargs: Argv) =>
    yargs
      .option("type", { type: "string", describe: "filter by MIME type, e.g. text/html" })
      .option("project", { type: "string", describe: "filter by design project id" })
      .option("q", { type: "string", describe: "free-text search over title/prompt" })
      .option("limit", { type: "number", describe: "max results (default 50, cap 100)" }),
  handler: async (args: { type?: string; project?: string; q?: string; limit?: number }) => {
    const config = await getAllternitApiConfigWithDeviceToken()
    const res = await listApiContentArtifacts(config, {
      ...(args.type ? { type: args.type } : {}),
      ...(args.project ? { project: args.project } : {}),
      ...(args.q ? { q: args.q } : {}),
      ...(args.limit != null ? { limit: args.limit } : {}),
    })
    const artifacts = res.artifacts ?? []
    if (artifacts.length === 0) {
      process.stdout.write("No content artifacts found.\n")
    }
    for (const a of artifacts) printArtifactRow(a)
    process.exit(0)
  },
})

export const ArtifactShowCommand = cmd({
  command: "show <id>",
  describe: "read a content artifact (current version) from the local gateway",
  builder: (yargs: Argv) =>
    yargs
      // `--version` here means artifact version, not CLI version.
      .version(false)
      .positional("id", { type: "string", describe: "artifact id (art_…), or a full a://artifact/<id> address" })
      .option("version", { type: "number", describe: "read a specific version instead of the current one" })
      .option("body-only", { type: "boolean", describe: "print only the artifact body", default: false }),
  handler: async (args: { id: string; version?: number; bodyOnly?: boolean }) => {
    const id = args.id.replace(/^a:\/\/artifact\//, "")
    const config = await getAllternitApiConfigWithDeviceToken()

    if (args.version != null) {
      const v = await getApiContentArtifactVersion(config, id, args.version)
      if (args.bodyOnly) {
        process.stdout.write(v.body)
      } else {
        process.stdout.write(`a://artifact/${v.artifactId} — version ${v.version}\n`)
        process.stdout.write(v.body)
        if (!v.body.endsWith("\n")) process.stdout.write("\n")
      }
      process.exit(0)
    }

    const res = await getApiContentArtifact(config, id)
    const a = res.artifact
    if (args.bodyOnly) {
      process.stdout.write(a.body ?? "")
    } else {
      process.stdout.write(`a://artifact/${a.id}\n`)
      process.stdout.write(`  title:   ${a.title ?? "Untitled"}\n`)
      process.stdout.write(`  type:    ${a.type ?? "text/html"}\n`)
      process.stdout.write(`  version: ${a.version ?? 1}\n`)
      if (a.provenance?.sourceSessionId) {
        process.stdout.write(`  session: ${a.provenance.sourceSessionId}\n`)
      }
      if (a.provenance?.prompt) {
        process.stdout.write(`  prompt:  ${a.provenance.prompt}\n`)
      }
      process.stdout.write(`${"-".repeat(40)}\n`)
      process.stdout.write(a.body ?? "")
      if (!(a.body ?? "").endsWith("\n")) process.stdout.write("\n")
    }
    process.exit(0)
  },
})

export const ArtifactSaveCommand = cmd({
  command: "save",
  describe: "create a content artifact on the local gateway (version 1)",
  builder: (yargs: Argv) =>
    yargs
      .option("title", { type: "string", demandOption: true, describe: "display name for the artifact" })
      .option("type", { type: "string", describe: "MIME type (default text/html)", default: "text/html" })
      .option("file", { type: "string", describe: "path to the artifact body file (use '-' or --stdin for stdin)" })
      .option("stdin", { type: "boolean", describe: "read the artifact body from stdin", default: false })
      .option("body", { type: "string", describe: "inline artifact body (alternative to --file/--stdin)" })
      .option("project-id", { type: "string", describe: "owning design project id, when known" })
      .option("prompt", { type: "string", describe: "producing prompt, recorded as provenance" }),
  handler: async (args: {
    title: string
    type: string
    file?: string
    stdin?: boolean
    body?: string
    projectId?: string
    prompt?: string
  }) => {
    let body = args.body
    if (body == null && (args.stdin || args.file === "-")) {
      body = await readStdin()
    } else if (body == null && args.file) {
      body = await Filesystem.readText(args.file)
    }
    if (body == null || body.length === 0) {
      throw new Error("Pass the artifact body via --file <path> (or '-' for stdin), or --body.")
    }

    const config = await getAllternitApiConfigWithDeviceToken()
    const idempotencyKey = `cli-save-${randomUUID()}`
    const res = await createApiContentArtifact(config, {
      title: args.title,
      type: args.type,
      body,
      ...(args.projectId ? { projectId: args.projectId } : {}),
      ...(args.prompt ? { prompt: args.prompt } : {}),
      sandboxPolicy: "standard",
      idempotencyKey,
    })
    const a = res.artifact
    process.stdout.write(`Saved "${args.title}" — a://artifact/${a?.id ?? "(no id returned)"}\n`)
    // Same pattern as html-artifact.ts: force a clean exit after a real
    // network call so the shared command bootstrap can't hang the shell.
    process.exit(0)
  },
})

export const ArtifactShareCommand = cmd({
  command: "share <id>",
  describe: "publish an artifact and print its share link (--status to check, --off to unpublish)",
  builder: (yargs: Argv) =>
    yargs
      .positional("id", { type: "string", demandOption: true })
      .option("at", { type: "number", describe: "publish this version (default: current)" })
      .option("status", { type: "boolean", describe: "only show whether it is shared" })
      .option("off", { type: "boolean", describe: "stop sharing (removes the link)" }),
  async handler(args) {
    const id = String(args.id)
    try {
      if (args.off) {
        await unshareArtifact(id)
        process.stdout.write(`${id}: no longer shared\n`)
        return
      }
      const s = args.status ? await artifactShareStatus(id) : await shareArtifact(id, args.at)
      if (!s.published) process.stdout.write(`${id}: not shared. Run \`gizzi artifact share ${id}\`.\n`)
      else process.stdout.write(`${s.url ?? s.routePath}${s.version != null ? `  (v${s.version})` : ""}\n`)
    } catch (e) {
      UI.error((e as Error).message)
      process.exitCode = 1
    }
  },
})

export const ArtifactCommand = cmd({
  command: "artifact",
  describe: "content-artifact client for the local gateway (A:// Artifacts API — list/read/create/share)",
  builder: (yargs: Argv) =>
    yargs
      .command(ArtifactListCommand)
      .command(ArtifactShowCommand)
      .command(ArtifactSaveCommand)
      .command(ArtifactShareCommand)
      .demandCommand(),
  async handler() {},
})
