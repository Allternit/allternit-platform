/**
 * Unified Cron Types for Gizzi CLI
 * 
 * Based on research from Supabase Cron and GitHub Actions
 * - Multiple job types (Shell, HTTP, Agent, Cowork)
 * - Natural language + cron expression support
 * - Comprehensive run tracking
 */

// @ts-ignore - Bun types may not be available in all contexts
type Database = import("bun:sqlite").Database;

// ═══════════════════════════════════════════════════════════════════════════════
// Job Types
// ═══════════════════════════════════════════════════════════════════════════════

export type JobType = 
  | "shell"      // Execute shell command
  | "http"       // HTTP webhook
  | "agent"      // AI agent task
  | "cowork"     // Start cowork session
  | "function"   // Internal function call
  | "vault";     // Vault sync (Gmail, Calendar, Fireflies, live notes)

export type JobStatus = 
  | "active"     // Job is scheduled and running
  | "paused"     // Job temporarily stopped
  | "disabled"   // Job disabled
  | "error";     // Job in error state

export type RunStatus = 
  | "pending"    // Waiting to execute
  | "running"    // Currently executing
  | "success"    // Completed successfully
  | "failed"     // Failed with error
  | "cancelled"  // Cancelled before completion
  | "timeout";   // Timed out

// ═══════════════════════════════════════════════════════════════════════════════
// Schedule Types
// ═══════════════════════════════════════════════════════════════════════════════

export interface CronSchedule {
  type: "cron";
  expression: string;  // Standard cron: "0 9 * * *"
  timezone?: string;   // IANA timezone: "America/New_York"
}

export interface IntervalSchedule {
  type: "interval";
  seconds: number;     // Run every N seconds
  startAt?: string;    // ISO timestamp to start
}

export type Schedule = CronSchedule | IntervalSchedule;

// ═══════════════════════════════════════════════════════════════════════════════
// Job Definitions
// ═══════════════════════════════════════════════════════════════════════════════

export interface BaseJob {
  id: string;
  name: string;
  description?: string;
  type: JobType;
  status: JobStatus;
  schedule: Schedule;

  // Timing
  createdAt: string;
  updatedAt: string;
  lastRunAt?: string;
  nextRunAt?: string;

  // Execution limits
  maxRuns?: number;        // Maximum number of runs (null = unlimited)
  runCount: number;        // Number of successful runs
  failCount: number;       // Number of failed runs

  // Timeout and retry
  timeoutSeconds?: number; // Default: 300 (5 minutes)
  maxRetries?: number;     // Default: 0
  retryDelaySeconds?: number; // Default: 60

  // Loop vs Scheduled distinction
  scope?: "session" | "persistent"; // "session" = loop (dies with session), "persistent" = scheduled task
  sessionId?: string;      // For session-scoped loops: owning session ID
  expiresAt?: string;      // ISO timestamp — auto-disable after this time (loops: 3 days)
  catchUpMissed?: boolean; // For persistent jobs: run missed fires on wake (default false)

  // Metadata
  tags: string[];
  metadata: Record<string, unknown>;
}

export interface ShellJob extends BaseJob {
  type: "shell";
  config: {
    command: string;
    cwd?: string;
    env?: Record<string, string>;
    shell?: string;
  };
}

export interface HttpJob extends BaseJob {
  type: "http";
  config: {
    url: string;
    method: "GET" | "POST" | "PUT" | "DELETE" | "PATCH";
    headers?: Record<string, string>;
    body?: string;
    timeoutSeconds?: number;
    retryOnFailure?: boolean;
  };
}

export interface AgentJob extends BaseJob {
  type: "agent";
  config: {
    prompt: string;
    agentId?: string;      // Specific agent, or auto-assign
    model?: string;        // Model to use
    context?: string;      // Additional context
    maxTokens?: number;
    temperature?: number;
    /** Bot Mode (B3): deliver into this bot's canonical chat (resume + turn)
     * instead of minting an ephemeral session. Jobs are namespaced
     * `[bot:<name>] <label>`. */
    bot?: string;
  };
}

export interface CoworkJob extends BaseJob {
  type: "cowork";
  config: {
    runtime: "docker" | "vm" | "local";
    image?: string;        // Docker image or VM template
    commands: string[];    // Commands to run
    env?: Record<string, string>;
    resources?: {
      cpus?: number;
      memory?: string;     // "512m", "2g", etc.
      disk?: string;
    };
    timeoutMinutes?: number;
  };
}

export interface FunctionJob extends BaseJob {
  type: "function";
  config: {
    module?: string;       // Deprecated: module paths (especially @/ aliases) do not work in compiled binaries. Use registerFunction() + function name instead.
    function: string;      // Registered function name (see function-registry.ts)
    args: unknown[];       // Arguments to pass
  };
}

export type CronJob = ShellJob | HttpJob | AgentJob | CoworkJob | FunctionJob;

// ═══════════════════════════════════════════════════════════════════════════════
// Job Run Records
// ═══════════════════════════════════════════════════════════════════════════════

export interface CronRun {
  id: string;
  jobId: string;
  status: RunStatus;
  
  // Timing
  scheduledAt: string;     // When it was scheduled
  startedAt?: string;      // When execution started
  finishedAt?: string;     // When execution finished
  durationMs?: number;     // Total duration
  
  // Execution details
  attempt: number;         // Attempt number (1 = first try)
  retryOf?: string;        // If this is a retry, link to original run
  
  // Results
  output?: string;         // stdout/response
  error?: string;          // stderr/error message
  reason?: string;         // Typed failure code (D4 closed vocabulary) when status = failed
  exitCode?: number;       // For shell jobs
  httpStatus?: number;     // For HTTP jobs
  
  // Agent-specific
  agentId?: string;        // Agent that handled the task
  response?: string;       // Agent response
  tokensUsed?: number;     // Token usage
  
  // Metadata
  triggeredBy: "schedule" | "manual" | "api" | "retry" | "wake";
  triggeredByUser?: string;
  metadata: Record<string, unknown>;
}

// ═══════════════════════════════════════════════════════════════════════════════
// API Types
// ═══════════════════════════════════════════════════════════════════════════════

export interface CreateJobInput {
  name: string;
  description?: string;
  type: JobType;
  schedule: Schedule | string;  // Can be natural language
  config: CronJob["config"];
  status?: JobStatus;
  maxRuns?: number;
  timeoutSeconds?: number;
  maxRetries?: number;
  tags?: string[];
  metadata?: Record<string, unknown>;
  // Loop / Scheduled distinction
  scope?: "session" | "persistent";
  sessionId?: string;
  expiresAt?: string;
  catchUpMissed?: boolean;
}

export interface UpdateJobInput {
  name?: string;
  description?: string;
  schedule?: Schedule | string;
  config?: Partial<CronJob["config"]>;
  status?: JobStatus;
  maxRuns?: number;
  timeoutSeconds?: number;
  maxRetries?: number;
  tags?: string[];
  metadata?: Record<string, unknown>;
}

export interface ListJobsFilter {
  type?: JobType;
  status?: JobStatus;
  tags?: string[];
  search?: string;
}

export interface ListRunsFilter {
  jobId?: string;
  status?: RunStatus;
  since?: string;
  until?: string;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Event Types
// ═══════════════════════════════════════════════════════════════════════════════

export type CronEventType = 
  | "job:created"
  | "job:updated"
  | "job:deleted"
  | "job:paused"
  | "job:resumed"
  | "job:run:scheduled"
  | "job:run:started"
  | "job:run:completed"
  | "job:run:failed"
  | "job:run:timeout"
  | "job:run:retry"
  | "job:run:cancelled"
  | "daemon:started"
  | "daemon:stopped";

export interface CronEvent<T = unknown> {
  type: CronEventType;
  timestamp: string;
  data: T;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Daemon Types
// ═══════════════════════════════════════════════════════════════════════════════

export interface DaemonConfig {
  port: number;
  host: string;
  dbPath: string;
  checkIntervalMs: number;
  logLevel: "debug" | "info" | "warn" | "error";
  maxConcurrentJobs: number;
  jobTimeoutSeconds: number;
  /** Pidfile path for supervision/staleness detection. Absent = no pidfile. */
  pidfile?: string;
}

export interface DaemonStatus {
  running: boolean;
  pid?: number;
  port: number;
  startedAt?: string;
  jobs: {
    total: number;
    active: number;
    paused: number;
  };
  runs: {
    pending: number;
    running: number;
    last24h: number;
  };
  version: string;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Natural Language Schedule Parsing
// ═══════════════════════════════════════════════════════════════════════════════

export interface ParsedSchedule {
  original: string;
  type: "cron" | "interval";
  expression: string;
  seconds?: number;
  description: string;
}

// ═══════════════════════════════════════════════════════════════════════════════
// SQLite Schema Types
// ═══════════════════════════════════════════════════════════════════════════════

export interface DatabaseSchema {
  jobs: CronJob;
  runs: CronRun;
  events: {
    id: string;
    type: CronEventType;
    timestamp: string;
    data: string;  // JSON
  };
}

// ═══════════════════════════════════════════════════════════════════════════════
// CronTypes Namespace (for CronStore compatibility)
// ═══════════════════════════════════════════════════════════════════════════════

export namespace CronTypes {
  export type CronJob = import("./types").CronJob;
  export type CronRun = import("./types").CronRun;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Service Configuration
// ═══════════════════════════════════════════════════════════════════════════════

export interface AgentQueueConfig {
  agentId: string;
  agentRole?: string;
  schedule?: string | Schedule;
}

export interface CronServiceConfig {
  // Persistence
  dbPath?: string;
  
  // Scheduling
  checkIntervalMs?: number;
  timezone?: string;
  
  // Execution
  maxConcurrentJobs?: number;
  defaultTimeoutSeconds?: number;
  defaultMaxRetries?: number;
  
  // Agent queue worker
  agentQueue?: AgentQueueConfig;
  
  // Callbacks
  onJobExecute?: (job: CronJob, run: CronRun) => Promise<void>;
  onJobComplete?: (job: CronJob, run: CronRun) => Promise<void>;
  onJobError?: (job: CronJob, run: CronRun, error: Error) => Promise<void>;
  
  // Event bus
  emitEvent?: (event: CronEvent) => void;
}
