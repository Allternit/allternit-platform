import { cmd } from "@/cli/commands/cmd"

/**
 * `gizzi worker`: sandbox worker for a self-hosted Agents Server
 * (allternit-agents-server). It only makes outbound HTTPS calls: claims work
 * for one environment, runs the session's tool calls with gizzi's own tools
 * in the workdir (bubblewrap sandbox, environment egress policy), and posts
 * the results. Protocol: docs/SELFHOST_WORKER_PROTOCOL.md.
 *
 * The handlers import the runtime relatively (not `@/`) because Bun.build
 * does not apply tsconfig paths to dynamic imports in the compiled binary
 * (same as fabric-worker).
 */

const WorkerAttachCommand = cmd({
  command: "attach",
  describe: "serve one claimed work item from env (run inside a per-session sandbox started by --on-work)",
  builder: (yargs) =>
    yargs
      .option("server", { type: "string", describe: "Agents Server URL (env ALLTERNIT_SERVER_URL)" })
      .option("workdir", { type: "string", describe: "tool working directory (env ALLTERNIT_WORKDIR, default /workspace)" })
      .option("block-ms", { type: "number", default: 25_000, describe: "long-poll wait for the next tool call" }),
  handler: async (args) => {
    const { runAttachCommand } = await import("../../runtime/selfhost/worker-entry")
    const code = await runAttachCommand({
      server: args.server as string | undefined,
      workdir: args.workdir as string | undefined,
      blockMs: args.blockMs as number | undefined,
    })
    process.exit(code)
  },
})

export const WorkerCommand = cmd({
  command: "worker",
  describe: "run a self-hosted Agents Server sandbox worker",
  builder: (yargs) =>
    yargs
      .command(WorkerAttachCommand)
      .option("server", { type: "string", describe: "Agents Server URL (env ALLTERNIT_SERVER_URL)" })
      .option("environment-key", { type: "string", describe: "environment key aek_... (env ALLTERNIT_ENVIRONMENT_KEY)" })
      .option("environment", { type: "string", describe: "environment id (env ALLTERNIT_ENVIRONMENT_ID)" })
      .option("worker-id", { type: "string", describe: "stable worker name shown in status (default host-pid)" })
      .option("block-ms", { type: "number", default: 25_000, describe: "long-poll wait for work and tool calls (max 30000)" })
      .option("drain", { type: "boolean", default: false, describe: "run one item (or find none), then exit 0" })
      .option("reclaim-older-than-ms", {
        type: "number",
        describe: "also claim items whose lease ran out at least this long ago (dead workers)",
      })
      .option("auto-stop", { type: "number", describe: "exit 0 after this many seconds with no work" })
      .option("on-work", {
        type: "string",
        describe: "run this command per claimed item (env: ALLTERNIT_WORK_ID, ALLTERNIT_SESSION_ID, ...); this process keeps the lease",
      })
      .option("workdir", { type: "string", describe: "tool working directory (env ALLTERNIT_WORKDIR, default /workspace)" }),
  handler: async (args) => {
    const { runWorkerCommand } = await import("../../runtime/selfhost/worker-entry")
    const code = await runWorkerCommand({
      server: args.server as string | undefined,
      environmentKey: args.environmentKey as string | undefined,
      environment: args.environment as string | undefined,
      workerId: args.workerId as string | undefined,
      blockMs: args.blockMs as number | undefined,
      drain: args.drain as boolean | undefined,
      reclaimOlderThanMs: args.reclaimOlderThanMs as number | undefined,
      autoStop: args.autoStop as number | undefined,
      onWork: args.onWork as string | undefined,
      workdir: args.workdir as string | undefined,
    })
    process.exit(code)
  },
})
