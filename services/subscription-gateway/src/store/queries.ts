// §1 store — typed row mappers + prepared-statement helpers. Every function
// takes the Database handle (dependency-injected, testable).
import { randomUUID } from "node:crypto";
import type {
  Account,
  Artifact,
  QuotaPool,
  RouteDecision,
  SubmissionState,
  Task,
  TaskAttempt,
  TaskError,
  TaskResult,
  TaskRouting,
  TaskStatus,
  ThreadMapping,
} from "@allternit/subscription-fabric-contracts";
import type { Db } from "./db.js";

// ---------------------------------------------------------------------------
// tasks
// ---------------------------------------------------------------------------

interface TaskRow {
  task_id: string;
  idempotency_key: string | null;
  capability: string;
  capability_version: number;
  requester: string;
  thread_id: string | null;
  project_id: string | null;
  parent_task_id: string | null;
  prompt: string;
  inputs: string;
  options: string;
  routing: string;
  constraints: string;
  approval_id: string | null;
  priority: string;
  status: string;
  status_detail: string | null;
  route_decision: string | null;
  result: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
}

function taskFromRow(db: Db, row: TaskRow): Task {
  return {
    task_id: row.task_id,
    idempotency_key: row.idempotency_key,
    capability: row.capability as Task["capability"],
    capability_version: row.capability_version,
    requester: JSON.parse(row.requester),
    thread_id: row.thread_id,
    project_id: row.project_id,
    parent_task_id: row.parent_task_id,
    prompt: row.prompt,
    inputs: JSON.parse(row.inputs),
    options: JSON.parse(row.options),
    routing: JSON.parse(row.routing),
    constraints: JSON.parse(row.constraints),
    approval_id: row.approval_id,
    priority: row.priority as Task["priority"],
    status: row.status as TaskStatus,
    status_detail: row.status_detail,
    route_decision: row.route_decision ? JSON.parse(row.route_decision) : null,
    attempts: listAttempts(db, row.task_id),
    result: row.result ? JSON.parse(row.result) : null,
    error: row.error ? JSON.parse(row.error) : null,
    created_at: row.created_at,
    updated_at: row.updated_at,
    completed_at: row.completed_at,
  };
}

// ---------------------------------------------------------------------------
// task_attempts — attempts live in their own table; getTask reassembles the
// contract object.
// ---------------------------------------------------------------------------

interface AttemptRow {
  task_id: string;
  attempt_no: number;
  adapter_id: string;
  adapter_version: string;
  account_id: string;
  pool_key: string;
  submission_state: string;
  prompt_fingerprint: string;
  provider_thread_id: string | null;
  requested_model_class: string | null;
  observed_model: string | null;
  started_at: string;
  ended_at: string | null;
  outcome: string;
  error: string | null;
}

function attemptFromRow(row: AttemptRow): TaskAttempt {
  return {
    attempt_no: row.attempt_no,
    adapter_id: row.adapter_id,
    adapter_version: row.adapter_version,
    account_id: row.account_id,
    pool_key: row.pool_key,
    submission_state: row.submission_state as TaskAttempt["submission_state"],
    prompt_fingerprint: row.prompt_fingerprint,
    provider_thread_id: row.provider_thread_id,
    requested_model_class: row.requested_model_class,
    observed_model: row.observed_model,
    started_at: row.started_at,
    ended_at: row.ended_at,
    outcome: row.outcome as TaskAttempt["outcome"],
    error: row.error ? JSON.parse(row.error) : null,
  };
}

export function insertAttempt(db: Db, taskId: string, attempt: TaskAttempt): void {
  db.prepare(
    `INSERT INTO task_attempts (
      task_id, attempt_no, adapter_id, adapter_version, account_id, pool_key,
      submission_state, prompt_fingerprint, provider_thread_id,
      requested_model_class, observed_model, started_at, ended_at, outcome,
      error
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    taskId,
    attempt.attempt_no,
    attempt.adapter_id,
    attempt.adapter_version,
    attempt.account_id,
    attempt.pool_key,
    attempt.submission_state,
    attempt.prompt_fingerprint,
    attempt.provider_thread_id,
    attempt.requested_model_class,
    attempt.observed_model,
    attempt.started_at,
    attempt.ended_at,
    attempt.outcome,
    attempt.error ? JSON.stringify(attempt.error) : null
  );
}

export function listAttempts(db: Db, taskId: string): TaskAttempt[] {
  const rows = db
    .prepare(
      "SELECT * FROM task_attempts WHERE task_id = ? ORDER BY attempt_no ASC"
    )
    .all(taskId) as AttemptRow[];
  return rows.map(attemptFromRow);
}

export function insertTask(db: Db, task: Task): void {
  db.prepare(
    `INSERT INTO tasks (
      task_id, idempotency_key, requester_id, capability, capability_version,
      requester, thread_id, project_id, parent_task_id, prompt, inputs,
      options, routing, constraints, approval_id, priority, status,
      status_detail, route_decision, result, error, created_at, updated_at,
      completed_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    task.task_id,
    task.idempotency_key,
    task.requester.id,
    task.capability,
    task.capability_version,
    JSON.stringify(task.requester),
    task.thread_id,
    task.project_id,
    task.parent_task_id,
    task.prompt,
    JSON.stringify(task.inputs),
    JSON.stringify(task.options),
    JSON.stringify(task.routing),
    JSON.stringify(task.constraints),
    task.approval_id,
    task.priority,
    task.status,
    task.status_detail,
    task.route_decision ? JSON.stringify(task.route_decision) : null,
    task.result ? JSON.stringify(task.result) : null,
    task.error ? JSON.stringify(task.error) : null,
    task.created_at,
    task.updated_at,
    task.completed_at
  );
}

export function getTask(db: Db, taskId: string): Task | null {
  const row = db
    .prepare("SELECT * FROM tasks WHERE task_id = ?")
    .get(taskId) as TaskRow | undefined;
  return row ? taskFromRow(db, row) : null;
}

export function updateTaskStatus(
  db: Db,
  taskId: string,
  status: TaskStatus,
  options: {
    statusDetail?: string | null;
    completedAt?: string | null;
    result?: TaskResult | null;
    error?: TaskError | null;
  } = {}
): void {
  db.prepare(
    `UPDATE tasks
     SET status = ?,
         status_detail = COALESCE(?, status_detail),
         completed_at = COALESCE(?, completed_at),
         result = COALESCE(?, result),
         error = COALESCE(?, error),
         updated_at = ?
     WHERE task_id = ?`
  ).run(
    status,
    options.statusDetail ?? null,
    options.completedAt ?? null,
    options.result !== undefined ? JSON.stringify(options.result) : null,
    options.error !== undefined ? JSON.stringify(options.error) : null,
    new Date().toISOString(),
    taskId
  );
}

// ---------------------------------------------------------------------------
// events (append-only ledger; seq is monotonic per task)
// ---------------------------------------------------------------------------

export interface StoredEvent {
  event_id: string;
  task_id: string;
  caller_id: string;
  seq: number;
  kind: string;
  payload: unknown;
  created_at: string;
}

interface EventRow {
  event_id: string;
  task_id: string;
  caller_id: string;
  seq: number;
  kind: string;
  payload: string;
  created_at: string;
}

function eventFromRow(row: EventRow): StoredEvent {
  return { ...row, payload: JSON.parse(row.payload) };
}

export function appendEvent(
  db: Db,
  event: { taskId: string; callerId: string; kind: string; payload: unknown }
): StoredEvent {
  const insert = db.transaction((): StoredEvent => {
    const { next } = db
      .prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS next FROM events WHERE task_id = ?")
      .get(event.taskId) as { next: number };
    const row: EventRow = {
      event_id: randomUUID(),
      task_id: event.taskId,
      caller_id: event.callerId,
      seq: next,
      kind: event.kind,
      payload: JSON.stringify(event.payload),
      created_at: new Date().toISOString(),
    };
    db.prepare(
      `INSERT INTO events (event_id, task_id, caller_id, seq, kind, payload, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?)`
    ).run(
      row.event_id,
      row.task_id,
      row.caller_id,
      row.seq,
      row.kind,
      row.payload,
      row.created_at
    );
    return eventFromRow(row);
  });
  return insert();
}

export function listEvents(db: Db, taskId: string): StoredEvent[] {
  const rows = db
    .prepare("SELECT * FROM events WHERE task_id = ? ORDER BY seq ASC")
    .all(taskId) as EventRow[];
  return rows.map(eventFromRow);
}

// ---------------------------------------------------------------------------
// caller_outbox (D12) — at-least-once, idempotent on event_id
// ---------------------------------------------------------------------------

export function outboxEnqueue(db: Db, eventId: string, callerIds: string[]): void {
  const stmt = db.prepare(
    "INSERT OR IGNORE INTO caller_outbox (event_id, caller_id) VALUES (?, ?)"
  );
  for (const callerId of callerIds) stmt.run(eventId, callerId);
}

export function outboxMarkDelivered(db: Db, eventId: string, callerId: string): void {
  db.prepare(
    `UPDATE caller_outbox SET delivered_at = ?
     WHERE event_id = ? AND caller_id = ? AND delivered_at IS NULL`
  ).run(new Date().toISOString(), eventId, callerId);
}

export function outboxAck(db: Db, eventId: string, callerId: string): void {
  db.prepare(
    `UPDATE caller_outbox
     SET delivered_at = COALESCE(delivered_at, ?), acked_at = ?
     WHERE event_id = ? AND caller_id = ? AND acked_at IS NULL`
  ).run(new Date().toISOString(), new Date().toISOString(), eventId, callerId);
}

// Reconnect replay: everything never delivered, in ledger insertion order.
export function outboxFetchUndelivered(
  db: Db,
  callerId: string,
  limit = 1000
): StoredEvent[] {
  const rows = db
    .prepare(
      `SELECT e.* FROM caller_outbox o
       JOIN events e ON e.event_id = o.event_id
       WHERE o.caller_id = ? AND o.delivered_at IS NULL
       ORDER BY e.rowid ASC
       LIMIT ?`
    )
    .all(callerId, limit) as EventRow[];
  return rows.map(eventFromRow);
}

// ---------------------------------------------------------------------------
// tokens (§A6.2) — lookup only; hashing lives in security/tokens.ts
// ---------------------------------------------------------------------------

export interface TokenRow {
  token_id: string;
  caller_id: string;
  name: string;
  token_hash: string;
  scopes: string[];
  created_at: string;
  revoked_at: string | null;
}

interface TokenDbRow extends Omit<TokenRow, "scopes"> {
  scopes: string;
}

export function findTokenByHash(db: Db, tokenHash: string): TokenRow | null {
  const row = db
    .prepare("SELECT * FROM tokens WHERE token_hash = ?")
    .get(tokenHash) as TokenDbRow | undefined;
  return row ? { ...row, scopes: JSON.parse(row.scopes) } : null;
}

// ---------------------------------------------------------------------------
// accounts
// ---------------------------------------------------------------------------

interface AccountRow {
  account_id: string;
  provider: string;
  label: string;
  plan: string | null;
  plan_observed_at: string | null;
  profile_ref: string;
  session_health: string;
  enabled: number;
}

function accountFromRow(row: AccountRow): Account {
  return {
    account_id: row.account_id,
    provider: row.provider as Account["provider"],
    label: row.label,
    plan: row.plan,
    plan_observed_at: row.plan_observed_at,
    profile_ref: row.profile_ref,
    session_health: row.session_health as Account["session_health"],
    enabled: row.enabled === 1,
  };
}

export function upsertAccount(db: Db, account: Account): void {
  db.prepare(
    `INSERT INTO accounts (
      account_id, provider, label, plan, plan_observed_at, profile_ref,
      session_health, enabled
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT (account_id) DO UPDATE SET
      provider = excluded.provider,
      label = excluded.label,
      plan = excluded.plan,
      plan_observed_at = excluded.plan_observed_at,
      profile_ref = excluded.profile_ref,
      session_health = excluded.session_health,
      enabled = excluded.enabled`
  ).run(
    account.account_id,
    account.provider,
    account.label,
    account.plan,
    account.plan_observed_at,
    account.profile_ref,
    account.session_health,
    account.enabled ? 1 : 0
  );
}

export function getAccount(db: Db, accountId: string): Account | null {
  const row = db
    .prepare("SELECT * FROM accounts WHERE account_id = ?")
    .get(accountId) as AccountRow | undefined;
  return row ? accountFromRow(row) : null;
}

export function listAccounts(db: Db): Account[] {
  const rows = db
    .prepare("SELECT * FROM accounts ORDER BY account_id ASC")
    .all() as AccountRow[];
  return rows.map(accountFromRow);
}

export function getTaskByIdempotency(
  db: Db,
  requesterId: string,
  idempotencyKey: string
): Task | null {
  const row = db
    .prepare(
      "SELECT * FROM tasks WHERE requester_id = ? AND idempotency_key = ?"
    )
    .get(requesterId, idempotencyKey) as TaskRow | undefined;
  return row ? taskFromRow(db, row) : null;
}

// ---------------------------------------------------------------------------
// artifacts — metadata row lookup (byte store lands in P3)
// ---------------------------------------------------------------------------

interface ArtifactRow {
  artifact_id: string;
  type: string;
  mime_type: string | null;
  format: string | null;
  title: string | null;
  task_id: string;
  attempt_no: number;
  capability: string;
  provider: string;
  account_id: string;
  adapter_id: string;
  adapter_version: string;
  provider_artifact_id: string | null;
  provider_url: string | null;
  provider_url_expires_at: string | null;
  thread_id: string | null;
  project_id: string | null;
  bot_id: string | null;
  retrieval_state: string;
  local_path: string | null;
  sha256: string | null;
  size_bytes: number | null;
  local_preview_path: string | null;
  version: number;
  parent_artifact_id: string | null;
  editable_via: string;
  export_formats: string;
  trust: string;
  sensitivity: string;
  created_at: string;
}

function artifactFromRow(row: ArtifactRow): Artifact {
  return {
    artifact_id: row.artifact_id,
    type: row.type as Artifact["type"],
    mime_type: row.mime_type,
    format: row.format,
    title: row.title,
    source: {
      task_id: row.task_id,
      attempt_no: row.attempt_no,
      capability: row.capability as Artifact["source"]["capability"],
      provider: row.provider as Artifact["source"]["provider"],
      account_id: row.account_id,
      adapter_id: row.adapter_id,
      adapter_version: row.adapter_version,
      provider_artifact_id: row.provider_artifact_id,
      provider_url: row.provider_url,
      provider_url_expires_at: row.provider_url_expires_at,
    },
    context: {
      thread_id: row.thread_id,
      project_id: row.project_id,
      bot_id: row.bot_id,
    },
    storage: {
      retrieval_state: row.retrieval_state as Artifact["storage"]["retrieval_state"],
      local_path: row.local_path,
      sha256: row.sha256,
      size_bytes: row.size_bytes,
      local_preview_path: row.local_preview_path,
    },
    lineage: {
      version: row.version,
      parent_artifact_id: row.parent_artifact_id,
    },
    capabilities: {
      editable_via: JSON.parse(row.editable_via),
      export_formats: JSON.parse(row.export_formats),
    },
    trust: row.trust as Artifact["trust"],
    sensitivity: row.sensitivity as Artifact["sensitivity"],
    created_at: row.created_at,
  };
}

export function getArtifact(db: Db, artifactId: string): Artifact | null {
  const row = db
    .prepare("SELECT * FROM artifacts WHERE artifact_id = ?")
    .get(artifactId) as ArtifactRow | undefined;
  return row ? artifactFromRow(row) : null;
}

export function insertArtifact(db: Db, artifact: Artifact): void {
  db.prepare(
    `INSERT INTO artifacts (
      artifact_id, type, mime_type, format, title, task_id, attempt_no,
      capability, provider, account_id, adapter_id, adapter_version,
      provider_artifact_id, provider_url, provider_url_expires_at,
      thread_id, project_id, bot_id, retrieval_state, local_path, sha256,
      size_bytes, local_preview_path, version, parent_artifact_id,
      editable_via, export_formats, trust, sensitivity, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    artifact.artifact_id,
    artifact.type,
    artifact.mime_type,
    artifact.format,
    artifact.title,
    artifact.source.task_id,
    artifact.source.attempt_no,
    artifact.source.capability,
    artifact.source.provider,
    artifact.source.account_id,
    artifact.source.adapter_id,
    artifact.source.adapter_version,
    artifact.source.provider_artifact_id,
    artifact.source.provider_url,
    artifact.source.provider_url_expires_at,
    artifact.context.thread_id,
    artifact.context.project_id,
    artifact.context.bot_id,
    artifact.storage.retrieval_state,
    artifact.storage.local_path,
    artifact.storage.sha256,
    artifact.storage.size_bytes,
    artifact.storage.local_preview_path,
    artifact.lineage.version,
    artifact.lineage.parent_artifact_id,
    JSON.stringify(artifact.capabilities.editable_via),
    JSON.stringify(artifact.capabilities.export_formats),
    artifact.trust,
    artifact.sensitivity,
    artifact.created_at
  );
}

// §A6.6 — the byte store only ever moves retrieval_state between
// downloading/local/failed and fills in the verified storage fields.
export function updateArtifactStorage(
  db: Db,
  artifactId: string,
  patch: {
    retrieval_state?: Artifact["storage"]["retrieval_state"];
    local_path?: string | null;
    sha256?: string | null;
    size_bytes?: number | null;
    mime_type?: string | null;
    format?: string | null;
  }
): void {
  const sets: string[] = [];
  const values: unknown[] = [];
  for (const [key, value] of Object.entries(patch)) {
    sets.push(`${key} = ?`);
    values.push(value);
  }
  if (sets.length === 0) return;
  db.prepare(`UPDATE artifacts SET ${sets.join(", ")} WHERE artifact_id = ?`).run(
    ...values,
    artifactId
  );
}

export function listArtifactsForTask(db: Db, taskId: string): Artifact[] {
  const rows = db
    .prepare("SELECT * FROM artifacts WHERE task_id = ? ORDER BY created_at ASC")
    .all(taskId) as ArtifactRow[];
  return rows.map(artifactFromRow);
}

export function listArtifacts(db: Db, limit = 200): Artifact[] {
  const rows = db
    .prepare("SELECT * FROM artifacts ORDER BY created_at DESC LIMIT ?")
    .all(limit) as ArtifactRow[];
  return rows.map(artifactFromRow);
}

// ---------------------------------------------------------------------------
// attempt mutation + recovery sweep (§A1 two-write, §A2 reconcile)
// ---------------------------------------------------------------------------

export function updateAttempt(
  db: Db,
  taskId: string,
  attemptNo: number,
  patch: {
    submission_state?: SubmissionState;
    provider_thread_id?: string | null;
    observed_model?: string | null;
    ended_at?: string | null;
    outcome?: TaskAttempt["outcome"];
    error?: TaskError | null;
  }
): void {
  const sets: string[] = [];
  const values: unknown[] = [];
  for (const [key, value] of Object.entries(patch)) {
    sets.push(`${key} = ?`);
    values.push(
      key === "error" && value !== null && value !== undefined
        ? JSON.stringify(value)
        : value
    );
  }
  if (sets.length === 0) return;
  db.prepare(
    `UPDATE task_attempts SET ${sets.join(", ")}
     WHERE task_id = ? AND attempt_no = ?`
  ).run(...values, taskId, attemptNo);
}

// §A8 — every attempt stuck in sent_unconfirmed goes through reconcile()
// before the queue resumes after a (re)start.
export function listAttemptsBySubmissionState(
  db: Db,
  state: SubmissionState
): Array<{ task_id: string; attempt: TaskAttempt }> {
  const rows = db
    .prepare("SELECT * FROM task_attempts WHERE submission_state = ? ORDER BY started_at ASC")
    .all(state) as AttemptRow[];
  return rows.map((row) => ({ task_id: row.task_id, attempt: attemptFromRow(row) }));
}

// ---------------------------------------------------------------------------
// quota_pools (§S3) — worker writes signals; the P4 router reads state
// ---------------------------------------------------------------------------

interface QuotaPoolRow {
  pool_key: string;
  pool_id: string;
  state: string;
  remaining: number | null;
  remaining_confidence: string;
  window: string;
  reset_at: string | null;
  reset_at_source: string | null;
  local_used_in_window: number;
  local_budget: number | null;
  cooldown_until: string | null;
  last_signal: string | null;
  updated_at: string;
}

function quotaPoolFromRow(row: QuotaPoolRow): QuotaPool {
  return {
    pool_key: row.pool_key,
    pool_id: row.pool_id,
    state: row.state as QuotaPool["state"],
    remaining: row.remaining,
    remaining_confidence: row.remaining_confidence as QuotaPool["remaining_confidence"],
    window: JSON.parse(row.window),
    reset_at: row.reset_at,
    reset_at_source: row.reset_at_source as QuotaPool["reset_at_source"],
    local_used_in_window: row.local_used_in_window,
    local_budget: row.local_budget,
    cooldown_until: row.cooldown_until,
    last_signal: row.last_signal ? JSON.parse(row.last_signal) : null,
    updated_at: row.updated_at,
  };
}

export function upsertQuotaPool(db: Db, pool: QuotaPool): void {
  db.prepare(
    `INSERT INTO quota_pools (
      pool_key, pool_id, state, remaining, remaining_confidence, window,
      reset_at, reset_at_source, local_used_in_window, local_budget,
      cooldown_until, last_signal, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT (pool_key) DO UPDATE SET
      state = excluded.state,
      remaining = excluded.remaining,
      remaining_confidence = excluded.remaining_confidence,
      window = excluded.window,
      reset_at = excluded.reset_at,
      reset_at_source = excluded.reset_at_source,
      local_used_in_window = excluded.local_used_in_window,
      local_budget = excluded.local_budget,
      cooldown_until = excluded.cooldown_until,
      last_signal = excluded.last_signal,
      updated_at = excluded.updated_at`
  ).run(
    pool.pool_key,
    pool.pool_id,
    pool.state,
    pool.remaining,
    pool.remaining_confidence,
    JSON.stringify(pool.window),
    pool.reset_at,
    pool.reset_at_source,
    pool.local_used_in_window,
    pool.local_budget,
    pool.cooldown_until,
    pool.last_signal ? JSON.stringify(pool.last_signal) : null,
    pool.updated_at
  );
}

export function getQuotaPool(db: Db, poolKey: string): QuotaPool | null {
  const row = db
    .prepare("SELECT * FROM quota_pools WHERE pool_key = ?")
    .get(poolKey) as QuotaPoolRow | undefined;
  return row ? quotaPoolFromRow(row) : null;
}

// P4 — the router's snapshot reads every pool row.
export function listQuotaPools(db: Db): QuotaPool[] {
  const rows = db
    .prepare("SELECT * FROM quota_pools ORDER BY pool_key ASC")
    .all() as QuotaPoolRow[];
  return rows.map(quotaPoolFromRow);
}

// ---------------------------------------------------------------------------
// P4 router wiring — route_decision + resolved routing fields
// ---------------------------------------------------------------------------

export function updateTaskRoutingDecision(
  db: Db,
  taskId: string,
  routing: TaskRouting,
  decision: RouteDecision
): void {
  db.prepare(
    `UPDATE tasks SET routing = ?, route_decision = ?, updated_at = ? WHERE task_id = ?`
  ).run(JSON.stringify(routing), JSON.stringify(decision), new Date().toISOString(), taskId);
}

// ---------------------------------------------------------------------------
// P4 Phase 2 — route_rejections: every rejected pair of every decision,
// appended at the same call sites where decisions are persisted.
// ---------------------------------------------------------------------------

export interface RouteRejectionRow {
  task_id: string;
  decision_id: string;
  adapter_id: string;
  account_id: string | null;
  reason: string;
  created_at: string;
}

export function recordRouteRejections(
  db: Db,
  taskId: string,
  decision: RouteDecision,
  now: Date = new Date()
): void {
  const stmt = db.prepare(
    `INSERT INTO route_rejections (task_id, decision_id, adapter_id, account_id, reason, created_at)
     VALUES (?, ?, ?, ?, ?, ?)`
  );
  const createdAt = now.toISOString();
  for (const r of decision.rejected) {
    stmt.run(taskId, decision.decision_id, r.adapter_id, r.account_id ?? null, r.reason, createdAt);
  }
}

export function listRouteRejections(db: Db, limit: number): RouteRejectionRow[] {
  return db
    .prepare(
      `SELECT task_id, decision_id, adapter_id, account_id, reason, created_at
       FROM route_rejections ORDER BY id DESC LIMIT ?`
    )
    .all(limit) as RouteRejectionRow[];
}

// ---------------------------------------------------------------------------
// thread_mappings (§S6 / HARDENING #7) — fabric thread → provider thread,
// 1:N with epochs; the last_turn_fingerprint feeds the adapter's divergence
// check before chat.continue submits.
// ---------------------------------------------------------------------------

function mappingFromRow(row: Record<string, unknown>): ThreadMapping {
  return row as unknown as ThreadMapping;
}

export function getActiveThreadMapping(db: Db, threadId: string): ThreadMapping | null {
  const row = db
    .prepare(
      "SELECT * FROM thread_mappings WHERE thread_id = ? AND status = 'active' ORDER BY epoch DESC LIMIT 1"
    )
    .get(threadId) as Record<string, unknown> | undefined;
  return row ? mappingFromRow(row) : null;
}

export interface ThreadTurn {
  thread_id: string;
  provider: string;
  account_id: string;
  adapter_id: string;
  provider_thread_id: string;
  provider_url: string;
  last_turn_fingerprint: string;
  model_class: string | null;
  on_divergence?: ThreadMapping["on_divergence"];
}

// A completed turn: the same provider thread advances the active mapping;
// a different one (new chat, other account) retires it and opens epoch+1.
export function recordThreadTurn(db: Db, turn: ThreadTurn, now = new Date().toISOString()): ThreadMapping {
  const active = getActiveThreadMapping(db, turn.thread_id);
  if (
    active &&
    active.provider_thread_id === turn.provider_thread_id &&
    active.account_id === turn.account_id
  ) {
    db.prepare(
      "UPDATE thread_mappings SET last_turn_fingerprint = ?, last_synced_turn_index = last_synced_turn_index + 1, provider_url = ?, last_used_at = ? WHERE mapping_id = ?"
    ).run(turn.last_turn_fingerprint, turn.provider_url, now, active.mapping_id);
    return getActiveThreadMapping(db, turn.thread_id)!;
  }
  if (active) {
    db.prepare("UPDATE thread_mappings SET status = 'migrated', last_used_at = ? WHERE mapping_id = ?").run(
      now,
      active.mapping_id
    );
  }
  const epoch =
    ((db.prepare("SELECT MAX(epoch) AS e FROM thread_mappings WHERE thread_id = ?").get(turn.thread_id) as {
      e: number | null;
    }).e ?? 0) + 1;
  db.prepare(
    `INSERT INTO thread_mappings (mapping_id, thread_id, epoch, provider, account_id, adapter_id,
      provider_thread_id, provider_project_id, provider_url, last_synced_turn_index,
      last_turn_fingerprint, model_class, status, on_divergence, context_transfer_from,
      created_at, last_used_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, 1, ?, ?, 'active', ?, ?, ?, ?)`
  ).run(
    randomUUID(),
    turn.thread_id,
    epoch,
    turn.provider,
    turn.account_id,
    turn.adapter_id,
    turn.provider_thread_id,
    turn.provider_url,
    turn.last_turn_fingerprint,
    turn.model_class,
    turn.on_divergence ?? "fail",
    active ? active.mapping_id : null,
    now,
    now
  );
  return getActiveThreadMapping(db, turn.thread_id)!;
}


// ---------------------------------------------------------------------------
// image_chats — image-chat history policy: one reusable image chat per
// (provider, account) inside the configured project, rotated at a max count.
// ---------------------------------------------------------------------------

export interface ImageChat {
  chat_id: string;
  provider: string;
  account_id: string;
  provider_thread_id: string;
  provider_url: string;
  project: string | null;
  image_count: number;
  status: "active" | "full";
  created_at: string;
  last_used_at: string;
}

export function getActiveImageChat(db: Db, provider: string, accountId: string): ImageChat | null {
  const row = db
    .prepare(
      "SELECT * FROM image_chats WHERE provider = ? AND account_id = ? AND status = 'active' ORDER BY created_at DESC LIMIT 1"
    )
    .get(provider, accountId) as ImageChat | undefined;
  return row ?? null;
}

export function getLatestImageChat(db: Db, provider: string, accountId: string): ImageChat | null {
  const row = db
    .prepare(
      "SELECT * FROM image_chats WHERE provider = ? AND account_id = ? ORDER BY last_used_at DESC LIMIT 1"
    )
    .get(provider, accountId) as ImageChat | undefined;
  return row ?? null;
}

export interface ImageChatUse {
  provider: string;
  account_id: string;
  provider_thread_id: string;
  provider_url: string;
  project: string | null;
  images: number;
}

// A finished image task: the same chat advances its count (and its URL — a
// project chat's canonical URL is learned on reuse); a different chat retires
// the active one and becomes active. Reaching `max` marks the chat full, so
// the next image task opens a new chat.
export function recordImageChatUse(
  db: Db,
  use: ImageChatUse,
  max: number,
  now = new Date().toISOString()
): ImageChat {
  const active = getActiveImageChat(db, use.provider, use.account_id);
  if (active && active.provider_thread_id === use.provider_thread_id) {
    const count = active.image_count + use.images;
    db.prepare(
      "UPDATE image_chats SET image_count = ?, provider_url = ?, status = ?, last_used_at = ? WHERE chat_id = ?"
    ).run(count, use.provider_url, count >= max ? "full" : "active", now, active.chat_id);
    return db.prepare("SELECT * FROM image_chats WHERE chat_id = ?").get(active.chat_id) as ImageChat;
  }
  if (active) {
    db.prepare("UPDATE image_chats SET status = 'full', last_used_at = ? WHERE chat_id = ?").run(now, active.chat_id);
  }
  const chatId = randomUUID();
  db.prepare(
    `INSERT INTO image_chats (chat_id, provider, account_id, provider_thread_id, provider_url, project,
      image_count, status, created_at, last_used_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    chatId,
    use.provider,
    use.account_id,
    use.provider_thread_id,
    use.provider_url,
    use.project,
    use.images,
    use.images >= max ? "full" : "active",
    now,
    now
  );
  return db.prepare("SELECT * FROM image_chats WHERE chat_id = ?").get(chatId) as ImageChat;
}
