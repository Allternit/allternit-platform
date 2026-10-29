/**
 * Agent Job Executor
 *
 * Executes AI agent tasks for cron jobs via the Session runtime directly.
 */

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

const log = createLogger("cron-agent-executor");

export interface AgentExecutorConfig {
  /** Default working directory for sessions */
  defaultCwd: string;
  /** Default model to use (providerID/modelID format) */
  defaultModel?: string;
  /** Injectable delivery seam for bot routines (tests inject fakes). */
  botRoutineDeps?: BotRoutineDeliveryDeps;
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
    if (jobConfig.model) {
      // Split on the first slash only: model ids may carry their own
      // (allternit-devices models are `<host_id>/<model_id>`).
      const slash = jobConfig.model.indexOf("/");
      if (slash > 0 && slash < jobConfig.model.length - 1) {
        model = { providerID: jobConfig.model.slice(0, slash), modelID: jobConfig.model.slice(slash + 1) };
      }
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
