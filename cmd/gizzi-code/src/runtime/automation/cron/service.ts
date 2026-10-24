/**
 * Enhanced CronService with SQLite Persistence
 * 
 * Unified TypeScript implementation replacing both:
 * - Rust allternit-scheduler (will be deprecated)
 * - Previous in-memory CronService
 * 
 * Features:
 * - SQLite persistence via Bun's built-in database
 * - Natural language schedule parsing
 * - Multiple job types (shell, http, agent, cowork, function)
 * - Daemon mode with HTTP API
 * - Event-driven architecture
 * - Run history and logs
 */

import { CronDatabase } from "./database";
import { parseScheduleToType, getNextRunTime, describeSchedule } from "./parser";
import { AgentExecutor } from "./executors/agent-executor";
import { CoworkExecutor } from "./executors/cowork-executor";
import { VaultSyncExecutor } from "./executors/vault-sync-executor";
import { getFunction, listRegisteredFunctions } from "./executors/function-registry";
import type {
  CronJob,
  CronRun,
  CreateJobInput,
  UpdateJobInput,
  JobStatus,
  JobType,
  CronServiceConfig,
  CronEvent,
  DaemonStatus,
} from "./types";
import { Global } from "@/runtime/context/global";
import { Log } from "@/shared/util/log";
import { join } from "path";
import { randomUUID } from "crypto";

const log = Log.create({ service: "cron-service" });

/** The abort reason a user Stop gives a run (vs. a timeout). */
const STOPPED_BY_USER = "Stopped by user";

// Singleton vault sync executor reused across job runs
let vaultSyncExecutor: VaultSyncExecutor | null = null;
function getVaultSyncExecutor(): VaultSyncExecutor {
  if (!vaultSyncExecutor) {
    vaultSyncExecutor = new VaultSyncExecutor({});
  }
  return vaultSyncExecutor;
}

// ═══════════════════════════════════════════════════════════════════════════════
// State Management
// ═══════════════════════════════════════════════════════════════════════════════

interface ServiceState {
  db: CronDatabase | null;
  timer: ReturnType<typeof setInterval> | null;
  isRunning: boolean;
  config: Required<CronServiceConfig>;
  runningJobs: Map<string, AbortController>; // jobId -> abort controller
  eventListeners: Set<(event: CronEvent) => void>;
}

const DEFAULT_CONFIG: Required<CronServiceConfig> = {
  dbPath: join(Global.Path.data, "cron.db"),
  checkIntervalMs: 60000,      // Check every minute
  timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  maxConcurrentJobs: 10,
  defaultTimeoutSeconds: 300,  // 5 minutes
  defaultMaxRetries: 0,
  agentQueue: undefined,
  onJobExecute: async () => {},
  onJobComplete: async () => {},
  onJobError: async () => {},
  emitEvent: () => {},
};

let state: ServiceState = {
  db: null,
  timer: null,
  isRunning: false,
  config: DEFAULT_CONFIG,
  runningJobs: new Map(),
  eventListeners: new Set(),
};

// ═══════════════════════════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════════════════════════

export const CronService = {
  // ═══════════════════════════════════════════════════════════════════════════════
  // Lifecycle
  // ═══════════════════════════════════════════════════════════════════════════════

  /**
   * Initialize the cron service with configuration
   */
  initialize(config: CronServiceConfig = {}): void {
    state.config = { ...DEFAULT_CONFIG, ...config };
    state.db = new CronDatabase(state.config.dbPath);
    
    emitEvent({
      type: "daemon:started",
      timestamp: new Date().toISOString(),
      data: { dbPath: state.config.dbPath },
    });
    
    log.info("initialized", { dbPath: state.config.dbPath });
  },

  /**
   * Start the scheduler (checks for due jobs)
   */
  start(): void {
    if (state.isRunning) return;
    if (!state.db) throw new Error("CronService not initialized. Call initialize() first.");

    state.isRunning = true;

    const now = new Date().toISOString();

    // Schedule next runs for all active jobs; catch up missed persistent runs
    const jobs = state.db.getActiveJobs();
    for (const job of jobs) {
      // Disable expired session loops on startup
      if ((job as any).expiresAt && (job as any).expiresAt < now) {
        job.status = "disabled";
        state.db.saveJob(job);
        log.info("expired loop disabled on startup", { name: job.name });
        continue;
      }

      if (!job.nextRunAt) {
        this._scheduleNextRun(job);
        continue;
      }

      // Catch up missed fires for persistent jobs with catchUpMissed enabled
      if ((job as any).catchUpMissed && (job as any).scope !== "session" && job.nextRunAt < now) {
        log.info("catching up missed run", { name: job.name, missedAt: job.nextRunAt });
        this._executeJob(job, "wake");
      }
    }

    // Start the check timer
    state.timer = setInterval(() => this._checkDueJobs(), state.config.checkIntervalMs);

    log.info("scheduler started", { intervalMs: state.config.checkIntervalMs });
  },

  /**
   * Delete all session-scoped loop jobs for a given session.
   * Call this when a session closes.
   */
  cleanupSessionJobs(sessionId: string): number {
    if (!state.db) return 0;
    const jobs = state.db.getAllJobs();
    let removed = 0;
    for (const job of jobs) {
      if ((job as any).scope === "session" && (job as any).sessionId === sessionId) {
        this.delete(job.id);
        removed++;
      }
    }
    if (removed > 0) log.info("session loops cleaned up", { sessionId, count: removed });
    return removed;
  },

  /**
   * Stop the scheduler
   */
  stop(): void {
    state.isRunning = false;
    if (state.timer) {
      clearInterval(state.timer);
      state.timer = null;
    }
    
    // Cancel all running jobs
    for (const [jobId, controller] of state.runningJobs) {
      controller.abort();
      state.runningJobs.delete(jobId);
    }
    
    emitEvent({
      type: "daemon:stopped",
      timestamp: new Date().toISOString(),
      data: {},
    });
    
    log.info("scheduler stopped");
  },

  /**
   * Close database connection
   */
  close(): void {
    this.stop();
    state.db?.close();
    state.db = null;
  },

  /**
   * Check if scheduler is running
   */
  isRunning(): boolean {
    return state.isRunning;
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Job Management
  // ═══════════════════════════════════════════════════════════════════════════════

  /**
   * Create a new scheduled job
   */
  create(input: CreateJobInput): CronJob {
    if (!state.db) throw new Error("CronService not initialized");

    // Parse schedule
    const schedule = typeof input.schedule === "string"
      ? parseScheduleToType(input.schedule)
      : input.schedule;
    
    if (!schedule) {
      throw new Error(`Invalid schedule: ${input.schedule}`);
    }

    const now = new Date().toISOString();
    // Session-scoped loops expire after 3 days by default
    const defaultExpiry = input.scope === "session"
      ? new Date(Date.now() + 3 * 24 * 60 * 60 * 1000).toISOString()
      : undefined;
    const job = {
      id: randomUUID(),
      name: input.name,
      description: input.description,
      type: input.type,
      status: input.status ?? "active",
      schedule,
      config: input.config as CronJob["config"],
      createdAt: now,
      updatedAt: now,
      runCount: 0,
      failCount: 0,
      maxRuns: input.maxRuns,
      timeoutSeconds: input.timeoutSeconds ?? state.config.defaultTimeoutSeconds,
      maxRetries: input.maxRetries ?? state.config.defaultMaxRetries,
      tags: input.tags ?? [],
      metadata: input.metadata ?? {},
      scope: input.scope ?? "persistent",
      sessionId: input.sessionId,
      expiresAt: input.expiresAt ?? defaultExpiry,
      catchUpMissed: input.catchUpMissed ?? false,
    } as CronJob;

    // Calculate next run
    job.nextRunAt = getNextRunTime(schedule, undefined, new Date()).toISOString();

    // Persist
    state.db.saveJob(job);

    emitEvent({
      type: "job:created",
      timestamp: now,
      data: { jobId: job.id, name: job.name },
    });

    log.info("job created", { name: job.name, schedule: describeSchedule(schedule) });
    return job;
  },

  /**
   * Get a job by ID
   */
  get(id: string): CronJob | null {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getJob(id);
  },

  /**
   * Get all jobs
   */
  list(): CronJob[] {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getAllJobs();
  },

  /**
   * Get active jobs
   */
  listActive(): CronJob[] {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getActiveJobs();
  },

  /**
   * Update a job
   */
  update(id: string, input: UpdateJobInput): CronJob {
    if (!state.db) throw new Error("CronService not initialized");
    
    const job = state.db.getJob(id);
    if (!job) throw new Error(`Job not found: ${id}`);

    // Update fields
    if (input.name !== undefined) job.name = input.name;
    if (input.description !== undefined) job.description = input.description;
    if (input.status !== undefined) job.status = input.status;
    if (input.maxRuns !== undefined) job.maxRuns = input.maxRuns;
    if (input.timeoutSeconds !== undefined) job.timeoutSeconds = input.timeoutSeconds;
    if (input.maxRetries !== undefined) job.maxRetries = input.maxRetries;
    if (input.tags !== undefined) job.tags = input.tags;
    
    // Update schedule
    if (input.schedule) {
      const schedule = typeof input.schedule === "string"
        ? parseScheduleToType(input.schedule)
        : input.schedule;
      if (schedule) {
        job.schedule = schedule;
        job.nextRunAt = getNextRunTime(schedule, undefined, new Date()).toISOString();
      }
    }

    // Update config
    if (input.config) {
      job.config = { ...job.config, ...input.config } as CronJob["config"];
    }

    job.updatedAt = new Date().toISOString();
    state.db.saveJob(job);

    emitEvent({
      type: "job:updated",
      timestamp: job.updatedAt,
      data: { jobId: job.id },
    });

    log.info("job updated", { name: job.name });
    return job;
  },

  /**
   * Pause a job
   */
  pause(id: string): CronJob {
    const job = this.update(id, { status: "paused" });
    emitEvent({
      type: "job:paused",
      timestamp: new Date().toISOString(),
      data: { jobId: id },
    });
    return job;
  },

  /**
   * Resume a paused job
   */
  resume(id: string): CronJob {
    const job = this.update(id, { status: "active" });
    this._scheduleNextRun(job);
    emitEvent({
      type: "job:resumed",
      timestamp: new Date().toISOString(),
      data: { jobId: id },
    });
    return job;
  },

  /**
   * Delete a job
   */
  delete(id: string): boolean {
    if (!state.db) throw new Error("CronService not initialized");
    
    const success = state.db.deleteJob(id);
    if (success) {
      emitEvent({
        type: "job:deleted",
        timestamp: new Date().toISOString(),
        data: { jobId: id },
      });
      log.info("job deleted", { id });
    }
    return success;
  },

  /**
   * Stop a job's current run. The run ends "cancelled" with whatever output
   * it had, and is not retried. False when the job is not running.
   */
  abort(id: string): boolean {
    const controller = state.runningJobs.get(id);
    if (!controller) return false;
    controller.abort(new Error(STOPPED_BY_USER));
    return true;
  },

  /**
   * Run a job manually
   */
  async run(id: string, triggeredByUser?: string): Promise<CronRun> {
    if (!state.db) throw new Error("CronService not initialized");
    
    const job = state.db.getJob(id);
    if (!job) throw new Error(`Job not found: ${id}`);

    return this._executeJob(job, "manual", triggeredByUser);
  },

  /**
   * Wake schedules - immediately check for due jobs
   */
  wake(): number {
    return this._checkDueJobs();
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Run History
  // ═══════════════════════════════════════════════════════════════════════════════

  /**
   * Get runs for a job
   */
  getRuns(jobId: string, limit = 100): CronRun[] {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getRunsByJob(jobId, limit);
  },

  /**
   * Get recent runs across all jobs
   */
  getRecentRuns(limit = 50): CronRun[] {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getRecentRuns(limit);
  },

  /**
   * Get run by ID
   */
  getRun(id: string): CronRun | null {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getRun(id);
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Statistics & Status
  // ═══════════════════════════════════════════════════════════════════════════════

  /**
   * Get daemon status
   */
  getStatus(): DaemonStatus {
    if (!state.db) {
      return {
        running: false,
        port: 0,
        jobs: { total: 0, active: 0, paused: 0 },
        runs: { pending: 0, running: 0, last24h: 0 },
        version: "1.0.0",
      };
    }

    const stats = state.db.getStats();
    return {
      running: state.isRunning,
      port: 0, // Set by HTTP server layer
      startedAt: undefined, // Track if needed
      jobs: stats.jobs,
      runs: stats.runs,
      version: "1.0.0",
    };
  },

  /**
   * Get statistics
   */
  getStats() {
    if (!state.db) throw new Error("CronService not initialized");
    return state.db.getStats();
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Event Handling
  // ═══════════════════════════════════════════════════════════════════════════════

  /**
   * Subscribe to events
   */
  onEvent(handler: (event: CronEvent) => void): () => void {
    state.eventListeners.add(handler);
    return () => state.eventListeners.delete(handler);
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Internal Methods
  // ═══════════════════════════════════════════════════════════════════════════════

  _checkDueJobs(): number {
    if (!state.db || !state.isRunning) return 0;

    const now = new Date().toISOString();
    const dueJobs = state.db.getJobsDueBefore(now);
    let executed = 0;

    for (const job of dueJobs) {
      // Skip expired jobs (session loops past TTL)
      if ((job as any).expiresAt && (job as any).expiresAt < now) {
        job.status = "disabled";
        state.db.saveJob(job);
        log.info("job expired, disabling", { name: job.name, expiresAt: (job as any).expiresAt });
        continue;
      }

      // Skip if max runs reached
      if (job.maxRuns && job.runCount >= job.maxRuns) {
        job.status = "disabled";
        state.db.saveJob(job);
        continue;
      }

      // Skip if too many concurrent jobs
      if (state.runningJobs.size >= state.config.maxConcurrentJobs) {
        log.warn("max concurrent jobs reached, skipping", { name: job.name });
        continue;
      }

      // Execute
      this._executeJob(job, "schedule");
      executed++;
    }

    if (executed > 0) {
      log.debug("due jobs executed", { count: executed });
    }

    return executed;
  },

  async _executeJob(job: CronJob, triggeredBy: CronRun["triggeredBy"], triggeredByUser?: string): Promise<CronRun> {
    if (!state.db) throw new Error("CronService not initialized");

    const runId = randomUUID();
    const now = new Date().toISOString();

    // Create run record
    const run: CronRun = {
      id: runId,
      jobId: job.id,
      status: "pending",
      scheduledAt: now,
      attempt: 1,
      triggeredBy,
      triggeredByUser,
      metadata: {},
    };

    state.db.saveRun(run);

    emitEvent({
      type: "job:run:started",
      timestamp: now,
      data: { jobId: job.id, runId, name: job.name },
    });

    // Create abort controller for timeout
    const controller = new AbortController();
    state.runningJobs.set(job.id, controller);

    const startTime = Date.now();

    try {
      // Update job and run status
      run.status = "running";
      run.startedAt = new Date().toISOString();
      state.db.saveRun(run);

      // Call user-provided handler
      await state.config.onJobExecute(job, run);

      // Execute based on job type
      // TODO(types): CronJob union omits VaultJob even though the daemon
      // registers vault jobs at runtime — cast until the union is widened.
      switch (job.type as JobType) {
        case "shell":
          await this._executeShell(job, run, controller.signal);
          break;
        case "http":
          await this._executeHttp(job, run, controller.signal);
          break;
        case "agent":
          await this._executeAgent(job, run, controller.signal);
          break;
        case "cowork":
          await this._executeCowork(job, run, controller.signal);
          break;
        case "function":
          await this._executeFunction(job, run, controller.signal);
          break;
        case "vault":
          await this._executeVault(job, run, controller.signal);
          break;
      }

      // A Stop can let the turn return normally with partial output.
      if (controller.signal.aborted) throw controller.signal.reason;

      // Success
      run.status = "success";
      run.finishedAt = new Date().toISOString();
      run.durationMs = Date.now() - startTime;

      job.lastRunAt = run.startedAt;
      job.runCount++;

      emitEvent({
        type: "job:run:completed",
        timestamp: run.finishedAt,
        data: { jobId: job.id, runId, durationMs: run.durationMs },
      });

      await state.config.onJobComplete(job, run);

    } catch (error) {
      if (controller.signal.aborted && (controller.signal.reason as Error | undefined)?.message === STOPPED_BY_USER) {
        run.status = "cancelled";
        run.error = STOPPED_BY_USER;
        run.finishedAt = new Date().toISOString();
        run.durationMs = Date.now() - startTime;
        emitEvent({
          type: "job:run:cancelled",
          timestamp: run.finishedAt,
          data: { jobId: job.id, runId },
        });
        return run;
      }
      // Failure
      run.status = "failed";
      run.error = error instanceof Error ? error.message : String(error);
      run.finishedAt = new Date().toISOString();
      run.durationMs = Date.now() - startTime;

      job.failCount++;

      // Check for retries
      if (run.attempt <= (job.maxRetries ?? 0)) {
        run.status = "pending";
        emitEvent({
          type: "job:run:retry",
          timestamp: run.finishedAt,
          data: { jobId: job.id, runId, attempt: run.attempt + 1 },
        });
      } else {
        emitEvent({
          type: "job:run:failed",
          timestamp: run.finishedAt,
          data: { jobId: job.id, runId, error: run.error },
        });
      }

      await state.config.onJobError(job, run, error instanceof Error ? error : new Error(String(error)));
    } finally {
      state.runningJobs.delete(job.id);
      state.db?.saveRun(run);
      state.db?.saveJob(job);

      // Schedule next run
      if (job.status === "active") {
        this._scheduleNextRun(job);
      }
    }

    return run;
  },

  _scheduleNextRun(job: CronJob): void {
    const nextRun = getNextRunTime(job.schedule, undefined, new Date());
    job.nextRunAt = nextRun.toISOString();
    state.db?.saveJob(job);

    emitEvent({
      type: "job:run:scheduled",
      timestamp: new Date().toISOString(),
      data: { jobId: job.id, nextRunAt: job.nextRunAt },
    });
  },

  // ═══════════════════════════════════════════════════════════════════════════════
  // Job Type Executors
  // ═══════════════════════════════════════════════════════════════════════════════

  async _executeShell(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const config = job.config as { command: string; cwd?: string; env?: Record<string, string>; shell?: string };
    
    const proc = Bun.spawn({
      cmd: [config.shell ?? "bash", "-c", config.command],
      cwd: config.cwd,
      env: { ...process.env, ...config.env },
      signal,
    });

    const stdout = await new Response(proc.stdout).text();
    const stderr = await new Response(proc.stderr).text();
    const exitCode = await proc.exited;

    run.output = stdout;
    if (stderr) run.error = stderr;
    run.exitCode = exitCode;

    if (exitCode !== 0) {
      throw new Error(`Command failed with exit code ${exitCode}: ${stderr || stdout}`);
    }
  },

  async _executeHttp(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const config = job.config as { url: string; method: string; headers?: Record<string, string>; body?: string; timeoutSeconds?: number };
    
    const timeoutMs = (config.timeoutSeconds ?? 30) * 1000;
    const controller = new AbortController();
    
    const timeoutId = setTimeout(() => controller.abort(), timeoutMs);
    
    try {
      const response = await fetch(config.url, {
        method: config.method,
        headers: config.headers,
        body: config.body,
        signal: controller.signal,
      });

      run.httpStatus = response.status;
      run.output = await response.text();

      if (!response.ok) {
        throw new Error(`HTTP ${response.status}: ${run.output}`);
      }
    } finally {
      clearTimeout(timeoutId);
    }
  },

  async _executeAgent(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const executor = new AgentExecutor({ defaultCwd: (state.config as any).cwd ?? process.cwd() })
    await executor.execute(job, run, signal)
  },

  async _executeCowork(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const executor = new CoworkExecutor({ defaultCwd: (state.config as any).cwd ?? process.cwd() })
    await executor.execute(job, run, signal)
  },

  async _executeFunction(job: CronJob, run: CronRun, _signal: AbortSignal): Promise<void> {
    const config = job.config as { function: string; args: unknown[] };
    
    const fn = getFunction(config.function);
    if (!fn) {
      throw new Error(
        `Function "${config.function}" is not registered. ` +
        `Available functions: ${listRegisteredFunctions().join(", ") || "none"}. ` +
        `Register functions with registerFunction() before creating cron jobs.`
      );
    }
    
    try {
      const result = await fn(...config.args);
      run.output = JSON.stringify(result, null, 2);
    } catch (error) {
      throw new Error(`Function "${config.function}" execution failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  },

  async _executeVault(job: CronJob, run: CronRun, signal: AbortSignal): Promise<void> {
    const executor = getVaultSyncExecutor()
    await executor.execute(job, run, signal)
  },
};

// ═══════════════════════════════════════════════════════════════════════════════
// Event Helpers
// ═══════════════════════════════════════════════════════════════════════════════

function emitEvent(event: CronEvent): void {
  state.db?.logEvent(event);
  
  for (const handler of state.eventListeners) {
    try {
      handler(event);
    } catch (e) {
      log.error("event handler error", { error: e instanceof Error ? e.message : e });
    }
  }
  
  state.config.emitEvent(event);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Re-exports
// ═══════════════════════════════════════════════════════════════════════════════

export type { CronJob, CronRun, CreateJobInput, UpdateJobInput, JobStatus, CronEvent, DaemonStatus };
