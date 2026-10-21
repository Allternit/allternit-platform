import type { CommandModule } from "yargs";
/**
 * Cron CLI Commands
 * 
 * Commands:
 * - allternit cron list              List all scheduled jobs
 * - allternit cron add               Add a new job (interactive)
 * - allternit cron remove <id>       Remove a job
 * - allternit cron run <id>          Run a job immediately
 * - allternit cron pause <id>        Pause a job
 * - allternit cron resume <id>       Resume a job
 * - allternit cron status            Show daemon status
 * - allternit cron start             Start the cron daemon
 * - allternit cron stop              Stop the cron daemon
 */

import { startDaemon, getRemoteStatus, stopRemoteDaemon, installCronCrashHandlers } from "@/runtime/automation/cron";
import {
  disableSupervision,
  enableSupervision,
  pidfilePath,
  readLastCrash,
  supervisionState,
  validatePidfile,
} from "@/runtime/automation/cron/supervision";
import { Global } from "@/runtime/context/global";
import { colors } from "../utils/colors";
import { cmd } from "./cmd";
import path from "path";

const DEFAULT_CRON_PORT = 3031;
const CRON_DB_PATH = path.join(Global.Path.data, "cron.db");

/** `… jobs start`: the daemon in the foreground (also the supervised entrypoint). */
const CronStartCommand: CommandModule<{}, any> = {
        command: "start",
        describe: "Start the cron daemon",
        builder: (yargs) =>
          yargs
            .option("port", {
              type: "number",
              default: DEFAULT_CRON_PORT,
              describe: "Port to run the daemon on",
            })
            .option("background", {
              type: "boolean",
              default: false,
              describe: "Run in the background",
            }),
        handler: async (argv) => {
          process.stdout.write(colors.cyan("Starting GIZZI Cron Daemon...\n"));

          if (argv.background) {
            const { spawn } = await import("child_process");
            const proc = spawn(process.execPath, process.argv.slice(1).filter(a => a !== "--background"), {
              detached: true,
              stdio: "ignore",
            });
            proc.unref();
            process.stdout.write(`Daemon started in background (PID: ${proc.pid})\n`);
            return;
          }

          try {
            await startDaemon({
              port: argv.port as number,
              dbPath: CRON_DB_PATH,
              pidfile: pidfilePath(),
            });
            // From here on a crash must be recorded and exit non-zero
            // (never leaving a stale pidfile), so launchd/systemd can
            // restart the daemon.
            installCronCrashHandlers(pidfilePath());
          } catch (e: any) {
            if (e.message?.includes("already in use")) {
              process.stdout.write(colors.yellow(`Daemon is already running on port ${argv.port}\n`));
            } else {
              process.stderr.write(colors.red(`Failed to start daemon: ${e.message}\n`));
            }
          }
        }
}

/**
 * Gizzi's local job scheduler (the cron daemon), mounted as
 * `gizzi workflows wake jobs`. The engine's Wake (`workflows wake
 * list|due|run`) schedules Factory checks; this runs Gizzi's own scheduled
 * jobs and bot routines.
 */
export const CronCommand = cmd({
  command: "jobs",
  describe: "Gizzi's local scheduled jobs and bot routines (the cron daemon)",
  builder: (yargs) =>
    yargs
      .command(
        "list",
        "List all scheduled jobs",
        () => {},
        async () => {
          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs`, {
              signal: AbortSignal.timeout(2000),
            });
            if (!res.ok) {
              process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
              return;
            }
            const { jobs } = await res.json() as { jobs: Array<{ id: string; name: string; type: string; schedule: string; status: string; lastRun?: string; nextRun?: string }> };
            if (!jobs || jobs.length === 0) {
              process.stdout.write(colors.yellow("No scheduled jobs found.\n"));
              return;
            }
            process.stdout.write(colors.bold("\nScheduled Jobs:\n"));
            process.stdout.write("=".repeat(80) + "\n");
            for (const job of jobs) {
              const statusColor = job.status === "active" ? colors.green : job.status === "paused" ? colors.yellow : colors.dim;
              process.stdout.write(
                `${colors.bold(job.name)} [${job.id}]\n` +
                `  Type: ${job.type}  Schedule: ${job.schedule}  Status: ${statusColor(job.status)}\n` +
                (job.lastRun ? `  Last run: ${job.lastRun}\n` : "") +
                (job.nextRun ? `  Next run: ${job.nextRun}\n` : "") +
                "\n"
              );
            }
            process.stdout.write(`Total: ${jobs.length}\n`);
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .command(CronStartCommand)
      .command(
        "enable",
        "Start the cron daemon on boot/login (launchd/systemd)",
        () => {},
        async () => {
          const result = await enableSupervision(process.execPath);
          if (result.ok) {
            process.stdout.write(colors.green(`${result.message}\n`));
          } else {
            process.stderr.write(colors.yellow(`${result.message}\n`));
          }
        }
      )
      .command(
        "disable",
        "Remove cron daemon boot/login autostart",
        () => {},
        async () => {
          const result = await disableSupervision();
          process.stdout.write(`${result.message}\n`);
        }
      )
      .command(
        "stop",
        "Stop the cron daemon",
        (yargs) => yargs.option("port", { type: "number", default: DEFAULT_CRON_PORT, describe: "Daemon port" }),
        async (argv) => {
          const port = argv.port as number;
          const running = await getRemoteStatus(port);
          if (!running) {
            process.stdout.write(colors.yellow(`No daemon running on port ${port}\n`));
            return;
          }
          const stopped = await stopRemoteDaemon(port);
          if (stopped) {
            process.stdout.write(colors.green("Cron daemon stopped.\n"));
          } else {
            process.stderr.write(colors.red("Failed to stop daemon — it may have already exited.\n"));
          }
        }
      )
      .command(
        "status",
        "Show daemon status",
        () => {},
        async () => {
          try {
            const status = await getRemoteStatus(DEFAULT_CRON_PORT);
            if (!status) {
              process.stdout.write(`Status:  ${colors.red("Offline")}\n`);
            } else {
              process.stdout.write(colors.bold("\nCron Daemon Status:\n"));
              process.stdout.write("=".repeat(40) + "\n");
              process.stdout.write(`Status:  ${colors.green("Online")}\n`);
              process.stdout.write(`Port:    ${(status as any).config?.port ?? DEFAULT_CRON_PORT}\n`);
              process.stdout.write(`Uptime:  ${formatDuration(Date.now() - new Date((status as any).startTime ?? Date.now()).getTime())}\n`);
              process.stdout.write(`Jobs:    ${(status as any).jobs?.total ?? 0} total, ${colors.green(String((status as any).jobs?.active ?? 0))} active, ${colors.yellow(String((status as any).jobs?.paused ?? 0))} paused\n`);
            }

            // Supervision state: autostart unit + pidfile validity + last crash.
            process.stdout.write(colors.bold("\nSupervision:\n"));
            process.stdout.write("=".repeat(40) + "\n");
            const sup = await supervisionState();
            if (!sup.supported) {
              process.stdout.write(`Autostart: ${colors.yellow("unsupported on " + process.platform)}\n`);
            } else {
              const unit = sup.launchdPlist ?? sup.systemdUnit;
              process.stdout.write(`Autostart: ${unit ? colors.green(`installed (${unit})`) : colors.dim("not installed — `gizzi workflows wake jobs enable`")}\n`);
            }

            const pidfile = pidfilePath();
            const pidState = await validatePidfile(pidfile);
            if (pidState.status === "running") {
              process.stdout.write(`Pidfile:   ${colors.green(`alive (pid ${pidState.pid})`)}\n`);
            } else if (pidState.status === "stale") {
              process.stdout.write(`Pidfile:   ${colors.yellow(`stale (pid ${pidState.pid}) — removed`)}\n`);
            } else {
              process.stdout.write(`Pidfile:   ${colors.dim("none")}\n`);
            }

            const crash = await readLastCrash();
            if (crash) {
              process.stdout.write(`Last crash: ${colors.red(`${crash.at} — ${crash.message}`)}\n`);
            }
          } catch {
            process.stdout.write(`Status:  ${colors.red("Offline")}\n`);
          }
        }
      )
      .command(
        "add",
        "Add a new scheduled job",
        (yargs) =>
          yargs
            .option("name", { type: "string", demandOption: true, describe: "Job name" })
            .option("type", {
              type: "string",
              demandOption: true,
              choices: ["shell", "http", "agent", "cowork", "function"],
              describe: "Job type",
            })
            .option("schedule", { type: "string", demandOption: true, describe: "Cron expression or natural language (e.g. 'every 5 minutes', '0 * * * *')" })
            .option("description", { type: "string", describe: "Optional description" })
            .option("command", { type: "string", describe: "Shell command (for type=shell)" })
            .option("url", { type: "string", describe: "HTTP URL (for type=http)" })
            .option("method", { type: "string", default: "GET", describe: "HTTP method (for type=http)" })
            .option("prompt", { type: "string", describe: "Agent prompt (for type=agent)" })
            .option("tags", { type: "string", describe: "Comma-separated tags" }),
        async (argv) => {
          let config: Record<string, unknown>;
          if (argv.type === "shell") {
            if (!argv.command) {
              process.stderr.write(colors.red("--command is required for type=shell\n"));
              process.exit(1);
            }
            config = { command: argv.command };
          } else if (argv.type === "http") {
            if (!argv.url) {
              process.stderr.write(colors.red("--url is required for type=http\n"));
              process.exit(1);
            }
            config = { url: argv.url, method: argv.method ?? "GET" };
          } else if (argv.type === "agent") {
            if (!argv.prompt) {
              process.stderr.write(colors.red("--prompt is required for type=agent\n"));
              process.exit(1);
            }
            config = { prompt: argv.prompt };
          } else if (argv.type === "cowork") {
            config = { runtime: "local", commands: [] };
          } else {
            config = {};
          }

          const body = {
            name: argv.name,
            description: argv.description,
            type: argv.type,
            schedule: argv.schedule,
            config,
            tags: argv.tags ? argv.tags.split(",").map((t: string) => t.trim()) : [],
          };

          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs`, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify(body),
              signal: AbortSignal.timeout(3000),
            });
            if (!res.ok) {
              const err = await res.json() as { error: string };
              process.stderr.write(colors.red(`Failed to create job: ${err.error}\n`));
              return;
            }
            const { job } = await res.json() as { job: { id: string; name: string; scheduleDescription: string } };
            process.stdout.write(colors.green(`Job created: ${colors.bold(job.name)} [${job.id}]\n`));
            process.stdout.write(`  Schedule: ${job.scheduleDescription}\n`);
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .command(
        "remove <id>",
        "Remove a scheduled job",
        (yargs) => yargs.positional("id", { type: "string", demandOption: true, describe: "Job ID" }),
        async (argv) => {
          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs/${argv.id}`, {
              method: "DELETE",
              signal: AbortSignal.timeout(3000),
            });
            if (res.status === 404) {
              process.stderr.write(colors.red(`Job not found: ${argv.id}\n`));
              return;
            }
            if (!res.ok) {
              process.stderr.write(colors.red("Failed to remove job.\n"));
              return;
            }
            process.stdout.write(colors.green(`Job removed: ${argv.id}\n`));
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .command(
        "run <id>",
        "Run a job immediately",
        (yargs) => yargs.positional("id", { type: "string", demandOption: true, describe: "Job ID" }),
        async (argv) => {
          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs/${argv.id}/run`, {
              method: "POST",
              signal: AbortSignal.timeout(3000),
            });
            if (res.status === 404) {
              process.stderr.write(colors.red(`Job not found: ${argv.id}\n`));
              return;
            }
            if (!res.ok) {
              process.stderr.write(colors.red("Failed to trigger job.\n"));
              return;
            }
            const { run } = await res.json() as { run: { id: string; status: string } };
            process.stdout.write(colors.green(`Job triggered. Run ID: ${run.id}  Status: ${run.status}\n`));
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .command(
        "pause <id>",
        "Pause a scheduled job",
        (yargs) => yargs.positional("id", { type: "string", demandOption: true, describe: "Job ID" }),
        async (argv) => {
          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs/${argv.id}/pause`, {
              method: "POST",
              signal: AbortSignal.timeout(3000),
            });
            if (res.status === 404) {
              process.stderr.write(colors.red(`Job not found: ${argv.id}\n`));
              return;
            }
            if (!res.ok) {
              process.stderr.write(colors.red("Failed to pause job.\n"));
              return;
            }
            process.stdout.write(colors.yellow(`Job paused: ${argv.id}\n`));
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .command(
        "resume <id>",
        "Resume a paused job",
        (yargs) => yargs.positional("id", { type: "string", demandOption: true, describe: "Job ID" }),
        async (argv) => {
          try {
            const res = await fetch(`http://127.0.0.1:${DEFAULT_CRON_PORT}/jobs/${argv.id}/resume`, {
              method: "POST",
              signal: AbortSignal.timeout(3000),
            });
            if (res.status === 404) {
              process.stderr.write(colors.red(`Job not found: ${argv.id}\n`));
              return;
            }
            if (!res.ok) {
              process.stderr.write(colors.red("Failed to resume job.\n"));
              return;
            }
            process.stdout.write(colors.green(`Job resumed: ${argv.id}\n`));
          } catch {
            process.stderr.write(colors.red("Failed to connect to cron daemon. Is it running?\n"));
          }
        }
      )
      .demandCommand(1),
  handler: async () => {},
});

function formatDuration(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (days > 0) return `${days}d ${hours % 24}h`;
  if (hours > 0) return `${hours}h ${minutes % 60}m`;
  if (minutes > 0) return `${minutes}m ${seconds % 60}s`;
  return `${seconds}s`;
}

/**
 * Hidden `gizzi cron start`, kept only because LaunchAgents/systemd units
 * written by earlier releases run exactly that line. Not in --help or
 * completions; `gizzi doctor` tells the user to re-run
 * `gizzi workflows wake jobs enable` to rewrite the unit.
 */
export const CronLegacyStartCommand = cmd({
  command: "cron",
  describe: false,
  builder: (yargs) => yargs.command(CronStartCommand).demandCommand(1),
  handler: async () => {},
})
