/**
 * Agent Job Executor
 *
 * Executes AI agent tasks for cron jobs via the Session runtime directly.
 */

import { refuseIfPersonOnly } from "@/runtime/providers/person-plan/rules";
import { createLogger } from "../utils/logger";
import type { CronJob, CronRun } from "../types";
import { Session } from "@/runtime/session";
import { SessionPrompt } from "@/runtime/session/prompt";
import {
  botRoutineLabel,
  deliverBotRoutine,
  type BotRoutineDeliveryDeps,
} from "@/runtime/bots/bot-routines";
import { classifyFailure, failureReasonOf } from "@/runtime/bots/failure-reasons";
import { ROUTINE_MARKER_PREFIX } from "@/runtime/bots/bot-routines";
import { Automation } from "@/runtime/primitives/automation";
import { Heartbeat } from "@/runtime/primitives/heartbeat";
import { UserMessage } from "@/runtime/primitives/user-message";
import type { AgentJob } from "../types";

const log = createLogger("cron-agent-executor");

export interface AgentExecutorConfig {
  /** Default working directory for sessions */
  defaultCwd: string;
  /** Default model to use (providerID/modelID format) */
  defaultModel?: string;
  /** Injectable delivery seam for bot routines (tests inject fakes). */
  botRoutineDeps?: BotRoutineDeliveryDeps;
  /** Injectable session/model seam for scheduled tasks (tests inject fakes). */
  automationDeps?: AutomationRunDeps;
}

/* -------------------------------------------------------------------------- */
/* Scheduled tasks (the automation tool's /cron fallback)                      */
/* -------------------------------------------------------------------------- */

type Model = { providerID: string; modelID: string };
type Reply = { text: string; tokensUsed: number };

/**
 * Where a scheduled task's run touches sessions and the model. The real
 * implementation is runtimeAutomationDeps(); tests replace it.
 */
export interface AutomationRunDeps {
  /** A marked user turn in an existing session; queued behind a running turn. Resolves with the reply. */
  deliverTurn(input: { sessionID: string; text: string; metadata: Record<string, unknown>; model?: Model; agent?: string }): Promise<Reply>;
  /** A run in a session of its own. `keep: false` removes the session afterwards. */
  runIsolated(input: {
    prompt: string;
    title: string;
    directory: string;
    metadata: Record<string, unknown>;
    model?: Model;
    agent?: string;
    keep: boolean;
    onSession?: (sessionID: string) => void;
  }): Promise<Reply & { sessionId: string }>;
  /** A visible message in the thread (a send_user_message card), with a push when `push`. */
  postNotice(input: { sessionID: string; text: string; push: boolean; automation: { id: string; title: string; kind: string } }): Promise<{
    posted: boolean;
    push: UserMessage.Push;
  }>;
  /** The approval this run stopped at, if it stopped at one (read once). */
  blockedOn(runId: string): Promise<string | undefined>;
}

/** Failed runs in a row before a scheduled task pauses itself. */
export const AUTOMATION_FAIL_STREAK = 3;
/** How long a notice waits for its thread's running turn to end before it is posted anyway as a push only. */
const NOTICE_IDLE_WAIT_MS = 15 * 60_000;

function replyText(message: { parts?: Array<{ type: string; text?: string; synthetic?: boolean }>; info?: unknown } | undefined): Reply {
  const parts = (message?.parts ?? []).filter((p) => p.type === "text" && !p.synthetic);
  const info = message?.info as { role?: string; tokens?: { output?: number } } | undefined;
  return {
    text: info?.role === "assistant" ? parts.map((p) => p.text ?? "").join("") : "",
    tokensUsed: info?.tokens?.output ?? 0,
  };
}

export function runtimeAutomationDeps(): AutomationRunDeps {
  return {
    async deliverTurn(input) {
      const { Instance } = await import("@/runtime/context/project/instance");
      const session = await Session.get(input.sessionID);
      // The session's own instance: its clients listen on that bus, and its
      // busy state (the queue) lives there.
      return Instance.provide({
        directory: session.directory,
        fn: async () => {
          const reply = await SessionPrompt.prompt({
            sessionID: input.sessionID,
            parts: [{ type: "text", text: input.text }],
            metadata: input.metadata,
            ...(input.model && { model: input.model }),
            ...(input.agent && { agent: input.agent }),
          });
          return replyText(reply as never);
        },
      });
    },
    async runIsolated(input) {
      const { Instance } = await import("@/runtime/context/project/instance");
      return Instance.provide({
        directory: input.directory,
        fn: async () => {
          const session = await Session.createNext({ directory: input.directory, title: input.title });
          input.onSession?.(session.id);
          try {
            const reply = await SessionPrompt.prompt({
              sessionID: session.id,
              parts: [{ type: "text", text: input.prompt }],
              metadata: input.metadata,
              ...(input.model && { model: input.model }),
              ...(input.agent && { agent: input.agent }),
            });
            return { ...replyText(reply as never), sessionId: session.id };
          } finally {
            if (!input.keep) {
              await Session.remove(session.id).catch((error: unknown) =>
                log.warn("Failed to delete scheduled-task session", { sessionId: session.id, error: String(error) }),
              );
            }
          }
        },
      });
    },
    async postNotice(input) {
      const text = input.text.length > UserMessage.MAX_TEXT_CHARS ? input.text.slice(0, UserMessage.MAX_TEXT_CHARS - 1) + "…" : input.text;
      // The push first: it doesn't wait for the thread to be free.
      let push: UserMessage.Push = "not_requested";
      if (input.push) {
        const sent = await UserMessage.sendUserMessage({ sessionID: input.sessionID, text, notify: "push", ctx: { roots: [] } }).catch(
          () => undefined,
        );
        push = sent?.push ?? "unavailable";
      }
      const session = await Session.get(input.sessionID).catch(() => undefined);
      if (!session) return { posted: false, push };
      const [{ Instance }, { SessionStatus }, { MessageV2 }, { Identifier }, { Bus }] = await Promise.all([
        import("@/runtime/context/project/instance"),
        import("@/runtime/session/status"),
        import("@/runtime/session/message-v2"),
        import("@/shared/id/id"),
        import("@/shared/bus"),
      ]);
      const posted = await Instance.provide({
        directory: session.directory,
        fn: async () => {
          // A finished assistant message written while a turn runs would end
          // that turn early (the loop reads it as the answer): wait it out.
          const deadline = Date.now() + NOTICE_IDLE_WAIT_MS;
          while (SessionStatus.get(input.sessionID).type !== "idle") {
            if (Date.now() > deadline) return false;
            await new Promise((resolve) => setTimeout(resolve, 2_000));
          }
          let user: { id: string; agent: string; model: Model } | undefined;
          for await (const message of MessageV2.stream(input.sessionID)) {
            if (message.info.role === "user") {
              user = message.info as never;
              break;
            }
          }
          if (!user) return false;
          const now = Date.now();
          const message = {
            id: Identifier.ascending("message"),
            sessionID: input.sessionID,
            role: "assistant" as const,
            parentID: user.id,
            modelID: user.model.modelID,
            providerID: user.model.providerID,
            mode: "automation",
            agent: user.agent,
            path: { cwd: session.directory, root: session.directory },
            cost: 0,
            tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
            time: { created: now, completed: now },
            finish: "stop",
          };
          await Session.updateMessage(message as never);
          const metadata: UserMessage.Metadata & { automation: typeof input.automation } = {
            text,
            files: [],
            push,
            automation: input.automation,
          };
          const part = {
            id: Identifier.ascending("part"),
            sessionID: input.sessionID,
            messageID: message.id,
            type: "tool" as const,
            callID: `automation-${message.id}`,
            tool: "send_user_message",
            state: {
              status: "completed" as const,
              input: { text, notify: input.push ? "push" : "none" },
              output: UserMessage.summary({ text, files: [], push }, input.push ? "push" : "none"),
              title: UserMessage.title(text),
              metadata,
              time: { start: now, end: now },
            },
          };
          const saved = await Session.updatePart(part as never);
          Bus.publish(MessageV2.Event.PartUpdated, { part: saved as never });
          return true;
        },
      });
      return { posted, push };
    },
    async blockedOn(runId) {
      const { PermissionNext } = await import("@/runtime/tools/guard/permission/next");
      return PermissionNext.takeUnattendedBlock(runId);
    },
  };
}

export class AgentExecutor {
  private config: AgentExecutorConfig;
  private activeSessions = new Map<string, { sessionId: string; abortController: AbortController }>();

  constructor(config: AgentExecutorConfig) {
    this.config = config;
  }

  async execute(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const jobConfig = job.config as {
      prompt: string;
      agentId?: string;
      model?: string;
      context?: string;
      bot?: string;
    };

    log.info("Starting agent job execution", {
      jobId: job.id,
      runId: run.id,
      prompt: jobConfig.prompt.slice(0, 100),
    });

    const abortController = new AbortController();
    const sessionKey = `${job.id}-${run.id}`;
    this.activeSessions.set(sessionKey, { sessionId: "", abortController });

    // A user Stop (CronService.abort) cancels the turn in flight; the turn
    // then returns with what it had, which becomes the run's output.
    let liveSession = "";
    signal.addEventListener("abort", () => {
      abortController.abort();
      if (liveSession) SessionPrompt.cancel(liveSession);
    });

    let model: { providerID: string; modelID: string } | undefined;
    // Routines never run on the person's own plan (chatgpt/…, plan/…, copilot-plan/…).
    const personOnly = refuseIfPersonOnly(jobConfig.model, "routine");
    if (personOnly) throw new Error(personOnly);
    if (jobConfig.model) {
      // The provider is up to the first slash: a model id can hold more
      // ("openrouter/x/y"; allternit-devices models are `<host_id>/<model_id>`).
      const slash = jobConfig.model.indexOf("/");
      if (slash > 0 && slash < jobConfig.model.length - 1) {
        model = { providerID: jobConfig.model.slice(0, slash), modelID: jobConfig.model.slice(slash + 1) };
      }
    }

    // A scheduled task the automation tool made (its /cron fallback).
    if ((job.config as AgentJob["config"]).kind) {
      try {
        await this.executeAutomation(job as AgentJob, run, model, (id) => {
          liveSession = id;
        });
      } finally {
        await this.cleanupSession(sessionKey);
      }
      return;
    }

    // Bot Mode (B3, D3): a routine delivers into the bot's canonical chat
    // (resume + one marked turn) instead of an ephemeral Session.createNext
    // session. The persona injection fires automatically because the prompt
    // pipeline keys off the canonical session id. An unknown bot throws a
    // structured error here, which the CronService run record captures as a
    // failed run.
    if (jobConfig.bot) {
      const label = botRoutineLabel(job.name);
      log.info("Delivering bot routine", { jobId: job.id, runId: run.id, bot: jobConfig.bot, label });
      const startTime = Date.now();
      try {
        const delivery = await deliverBotRoutine({
          botName: jobConfig.bot,
          label,
          prompt: jobConfig.prompt,
          context: jobConfig.context,
          model,
          agentId: jobConfig.agentId,
          projectPath: this.config.defaultCwd,
          deps: this.config.botRoutineDeps,
          onSession: (id) => {
            liveSession = id;
          },
        });
        run.response = delivery.response;
        run.output = delivery.response;
        run.tokensUsed = delivery.tokensUsed;
        run.agentId = jobConfig.agentId ?? `bot:${jobConfig.bot}`;
        log.info("Bot routine delivered", {
          jobId: job.id,
          runId: run.id,
          bot: jobConfig.bot,
          sessionId: delivery.sessionId,
          createdSession: delivery.createdSession,
          tokensUsed: run.tokensUsed,
          duration: Date.now() - startTime,
        });
      } catch (error) {
        // Typed failure taxonomy (B4/D4): persist the closed reason code on
        // the run record (typed `reason` column + metadata) alongside the
        // free-text error. deliverBotRoutine annotates the error when it
        // gives up after its retry policy; fall back to classifying here so
        // every failure path carries a reason.
        const reason = failureReasonOf(error) ?? classifyFailure(error);
        run.reason = reason;
        run.metadata = { ...run.metadata, reason };
        log.error("Bot routine delivery failed", {
          jobId: job.id,
          runId: run.id,
          bot: jobConfig.bot,
          reason,
          error: error instanceof Error ? error.message : String(error),
        });
        throw error;
      } finally {
        // The canonical session must survive the run — never delete it the
        // way cleanupSession deletes ephemeral agent sessions. The entry was
        // registered with an empty sessionId, so cleanup is already a no-op;
        // aborting the controller only tears down this run's listeners.
        await this.cleanupSession(sessionKey);
      }
      return;
    }

    try {
      const session = await Session.createNext({ directory: this.config.defaultCwd });
      const sessionId = session.id;
      this.activeSessions.set(sessionKey, { sessionId, abortController });
      liveSession = sessionId;

      log.info("Agent session created", { sessionId, jobId: job.id });

      const fullPrompt = jobConfig.context
        ? `${jobConfig.context}\n\n${jobConfig.prompt}`
        : jobConfig.prompt;

      const startTime = Date.now();

      await SessionPrompt.prompt({
        sessionID: sessionId,
        parts: [{ type: "text", text: fullPrompt }],
        ...(model && { model }),
        ...(jobConfig.agentId && { agent: jobConfig.agentId }),
      });

      const messages = await Session.messages({ sessionID: sessionId });
      const assistantMessages = messages.filter((m) => m.info.role === "assistant");
      const lastAssistant = assistantMessages[assistantMessages.length - 1];

      if (lastAssistant) {
        const textParts = lastAssistant.parts.filter((p) => p.type === "text") as Array<{ text?: string }>;
        run.response = textParts.map((p) => p.text ?? "").join("");
        run.output = run.response;
        run.tokensUsed = (lastAssistant.info as any).tokens?.output ?? 0;
      }

      run.agentId = jobConfig.agentId ?? "default";

      log.info("Agent job completed", {
        jobId: job.id,
        runId: run.id,
        tokensUsed: run.tokensUsed,
        duration: Date.now() - startTime,
      });
    } catch (error) {
      // Same D4 taxonomy as the bot path: every failed agent run carries a
      // typed reason code on the run record.
      const reason = failureReasonOf(error) ?? classifyFailure(error);
      run.reason = reason;
      run.metadata = { ...run.metadata, reason };
      log.error("Agent job failed", {
        jobId: job.id,
        runId: run.id,
        reason,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    } finally {
      await this.cleanupSession(sessionKey);
    }
  }

  /**
   * One run of a scheduled task:
   * - message + this_thread: a "[routine: <title>]" turn in the thread,
   *   marked `metadata.automation_run` (queued while the thread is busy);
   * - message + new_session: the same turn in a session of its own, kept;
   * - heartbeat: an isolated run of the prompt plus the verdict instructions.
   *   NOTIFY posts the line to the thread and pushes it; DONT_NOTIFY is a
   *   quiet run; DONE posts, then ends the task; no verdict fails the run.
   * Afterwards the task is re-armed from its real schedule, or ended when
   * nothing more is due (one-shot, UNTIL, COUNT). A run due more than 12 h
   * ago is logged as missed instead. Three failures in a row pause it.
   * Every turn carries the run's id, so an approval ask in it is refused at
   * once (PermissionNext) and the run reports needs_you.
   */
  private async executeAutomation(job: AgentJob, run: CronRun, model: Model | undefined, onSession: (id: string) => void): Promise<void> {
    const config = job.config;
    const deps = this.config.automationDeps ?? runtimeAutomationDeps();
    const title = config.title ?? job.name;
    const kind = config.kind ?? "message";
    const meta = { id: job.id, title, kind };
    const metadata = { [Automation.RUN_METADATA_KEY]: { id: job.id, runId: run.id } };
    const directory = config.directory ?? this.config.defaultCwd;
    const end = (how: "completed" | "missed") => {
      job.status = "disabled";
      config.ended = how;
    };
    const record = (status: string, extra: Record<string, unknown> = {}) => {
      config.lastRunStatus = status;
      run.metadata = { ...run.metadata, automation: { kind, outcome: status, ...extra } };
    };
    const notice = async (text: string, push = true) => {
      if (!config.targetSessionId) return { posted: false };
      return deps.postNotice({ sessionID: config.targetSessionId, text, push, automation: meta });
    };
    let failed = false;
    try {
      const now = Date.now();
      const due = job.nextRunAt ? Date.parse(job.nextRunAt) : NaN;
      if ((run.triggeredBy === "schedule" || run.triggeredBy === "wake") && Number.isFinite(due) && now - due > Automation.LATE_LIMIT_MS) {
        record("missed");
        if (config.automationSchedule?.type === "once") end("missed");
        throw new Error(
          `Missed: this run was due ${Automation.formatInZone(new Date(due), config.timezone || "UTC")}, more than 12 hours ago (the computer was asleep or the scheduler was off).`,
        );
      }
      if (config.until && now > Date.parse(config.until)) {
        end("completed");
        record("done");
        run.output = "Ended: past its end time.";
        return;
      }

      if (kind === "message") {
        const text = `${ROUTINE_MARKER_PREFIX}${title}] ${config.prompt}`;
        const reply =
          config.delivery === "session" && config.targetSessionId
            ? await deps.deliverTurn({ sessionID: config.targetSessionId, text, metadata, model, agent: config.agentId })
            : await deps.runIsolated({ prompt: text, title, directory, metadata, model, agent: config.agentId, keep: true, onSession });
        run.response = reply.text;
        run.output = reply.text;
        run.tokensUsed = reply.tokensUsed;
        const blocked = await deps.blockedOn(run.id);
        record(blocked ? "needs_you" : "delivered", blocked ? { blocked } : {});
      } else {
        const reply = await deps.runIsolated({
          prompt: Heartbeat.runPrompt(config.prompt),
          title: `Check: ${title}`,
          directory,
          metadata,
          model,
          agent: config.agentId,
          keep: false,
          onSession,
        });
        run.response = reply.text;
        run.output = reply.text;
        run.tokensUsed = reply.tokensUsed;
        const blocked = await deps.blockedOn(run.id);
        if (blocked) {
          // The push went out when the ask was refused; the thread gets the card.
          record("needs_you", { blocked });
          await notice(`"${title}" stopped: it needs your approval to ${blocked}.`, false);
          throw new Error(`Stopped: this check needs the owner's approval to ${blocked}.`);
        }
        const verdict = Heartbeat.parseVerdict(reply.text);
        if (verdict.kind === "none") {
          record("failed", { verdict: "none" });
          throw new Error("The check's reply didn't end with NOTIFY: <line>, DONT_NOTIFY or DONE: <line>, so nothing was sent.");
        }
        if (verdict.kind === "dont_notify") {
          record("quiet", { verdict: "dont_notify", quiet: true });
        } else {
          const line = verdict.line || `"${title}" is done.`;
          const sent = await notice(line);
          record(verdict.kind === "done" ? "done" : "notify", { verdict: verdict.kind, line, posted: sent.posted });
          if (verdict.kind === "done") end("completed");
        }
      }
      config.failStreak = 0;
      // COUNT: this success is run number runCount + 1.
      if (job.status === "active" && job.maxRuns && job.runCount + 1 >= job.maxRuns) end("completed");
    } catch (error) {
      failed = true;
      if (config.lastRunStatus !== "missed" && config.lastRunStatus !== "needs_you" && config.lastRunStatus !== "failed") record("failed");
      const reason = failureReasonOf(error) ?? classifyFailure(error);
      run.reason = reason;
      run.metadata = { ...run.metadata, reason };
      log.error("Scheduled task run failed", {
        jobId: job.id,
        runId: run.id,
        reason,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    } finally {
      if (failed && config.lastRunStatus !== "missed") {
        config.failStreak = (config.failStreak ?? 0) + 1;
        if (job.status === "active" && config.failStreak >= AUTOMATION_FAIL_STREAK) {
          job.status = "paused";
          await notice(`Scheduled task "${title}" is paused after ${config.failStreak} failed runs in a row. Turn it back on from your scheduled tasks.`).catch(
            () => {},
          );
        }
      }
      // Re-arm from the real schedule (the service then sets nextRunAt from this interval).
      if (job.status === "active") {
        const next = Automation.nextFire(config, new Date());
        if (next) job.schedule = Automation.intervalUntil(next);
        else end("completed");
      }
    }
  }

  private async cleanupSession(sessionKey: string): Promise<void> {
    const session = this.activeSessions.get(sessionKey);
    if (!session) return;

    try {
      session.abortController.abort();
      if (session.sessionId) {
        await Session.remove(session.sessionId).catch((err: unknown) => {
          log.warn("Failed to delete agent session", {
            sessionId: session.sessionId,
            error: err instanceof Error ? err.message : String(err),
          });
        });
      }
    } finally {
      this.activeSessions.delete(sessionKey);
    }
  }

  async cancel(jobId: string, runId: string): Promise<void> {
    const sessionKey = `${jobId}-${runId}`;
    const session = this.activeSessions.get(sessionKey);
    if (session) {
      session.abortController.abort();
      await this.cleanupSession(sessionKey);
    }
  }

  async healthCheck(): Promise<boolean> {
    try {
      const _ = [...Session.list({ directory: this.config.defaultCwd })];
      return true;
    } catch {
      return false;
    }
  }
}
