// Inline fixtures for the contracts tests.
//
// HARD GATE: no provider-name literals here either — every provider/account/
// adapter id uses the `*_example_*` convention so the package is
// provider-agnostic by construction.
import type {
  Account,
  AdapterEvent,
  AdapterManifest,
  Artifact,
  CapabilityDef,
  QuotaPool,
  RouteDecision,
  Task,
  ThreadMapping,
} from "../src/index";

export const PROVIDER = "prov_example";
export const ACCOUNT = "acct_example_01";
export const ADAPTER = "adapt_example_web";

export const capabilityDef: CapabilityDef = {
  id: "example.create" as CapabilityDef["id"],
  version: 1,
  input_schema: {
    type: "object",
    properties: { count: { type: "number" }, format: { type: "string" } },
    required: ["count"],
  },
  output_artifact_types: ["presentation", "pdf"],
  side_effects: "provider_state",
  requires_thread: false,
  long_running_hint: true,
};

export const adapterManifest: AdapterManifest = {
  adapter_id: ADAPTER,
  adapter_version: "1.0.0",
  provider: PROVIDER as AdapterManifest["provider"],
  interface: "ui_bridge_web",
  origins: ["https://provider.example"],
  auth: {
    login_url: "https://provider.example/login",
    logged_in_probe: "composer",
  },
  plans: [{ plan_id: "pro", label: "Pro", notes: "fixture plan" }],
  capabilities: [
    {
      id: "example.create" as AdapterManifest["capabilities"][number]["id"],
      min_capability_version: 1,
      plans: ["pro"],
      pool_id: "example-pool",
      detachable: true,
      export_formats: ["pptx", "pdf"],
      max_inputs: { files: 5, bytes_per_file: 10485760 },
      status: "stable",
    },
  ],
  pacing: {
    min_action_gap_ms: [800, 2500],
    min_task_gap_s: 5,
    max_tasks_per_hour: 20,
    max_tasks_per_day: 60,
    quiet_hours: [23, 7],
  },
  selectors_version: "2026-01-01",
};

export const account: Account = {
  account_id: ACCOUNT,
  provider: PROVIDER as Account["provider"],
  label: "Example — Pro",
  plan: "pro",
  plan_observed_at: "2026-01-01T00:00:00.000Z",
  profile_ref: "profile://example_01",
  session_health: "ready",
  enabled: true,
};

export const quotaPool: QuotaPool = {
  pool_key: `${PROVIDER}:${ACCOUNT}:example-pool`,
  pool_id: "example-pool",
  state: "estimated",
  remaining: 42,
  remaining_confidence: "estimated",
  window: { kind: "rolling", seconds: 18000 },
  reset_at: "2026-01-01T05:00:00.000Z",
  reset_at_source: "inferred",
  local_used_in_window: 3,
  local_budget: 50,
  cooldown_until: null,
  last_signal: {
    kind: "counter_visible",
    raw_excerpt: "42 remaining",
    observed_at: "2026-01-01T00:00:00.000Z",
    task_id: null,
  },
  updated_at: "2026-01-01T00:00:00.000Z",
};

// A Task exercising every TaskInput variant (artifact, file, text, url, image).
export const task: Task = {
  task_id: "task_example_01",
  idempotency_key: "idem_example_01",
  capability: "example.create" as Task["capability"],
  capability_version: 1,
  requester: { kind: "bot", id: "bot_example", bot_id: "bot_example" },
  initiated_by: { kind: "human", user_id: "user_example", action_id: "action_example_01" },
  thread_id: "thread_example_01",
  project_id: null,
  parent_task_id: null,
  prompt: "Create a deck about fixtures.",
  inputs: [
    { type: "artifact", artifact_id: "artifact_example_00" },
    { type: "file", path: "workspace/notes.txt", sha256: "a".repeat(64), size_bytes: 128 },
    { type: "text", name: "brief", content: "Keep it short." },
    { type: "url", url: "https://example.test/source" },
    { type: "image", mime_type: "image/png", data_base64: "iVBORw0KGgo=" },
  ],
  options: { count: 3, format: "pptx" },
  routing: {
    mode: "auto",
    allow_fallback: true,
    allow_metered: false,
    allow_thread_migration: false,
  },
  constraints: {
    sensitivity: "internal",
    deadline_at: null,
    max_metered_usd: null,
    required_export_format: "pptx",
  },
  approval_id: null,
  priority: "normal",
  status: "provider_running",
  status_detail: "provider researching",
  route_decision: null,
  attempts: [
    {
      attempt_no: 1,
      adapter_id: ADAPTER,
      adapter_version: "1.0.0",
      account_id: ACCOUNT,
      pool_key: `${PROVIDER}:${ACCOUNT}:example-pool`,
      submission_state: "acknowledged",
      prompt_fingerprint: "b".repeat(64),
      provider_thread_id: "provthread_example_1",
      requested_model_class: "reasoning",
      observed_model: "example-fast",
      started_at: "2026-01-01T00:00:00.000Z",
      ended_at: null,
      outcome: "partial",
      error: null,
    },
  ],
  result: null,
  error: null,
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-01T00:01:00.000Z",
  completed_at: null,
};

export const artifact: Artifact = {
  artifact_id: "artifact_example_01",
  type: "presentation",
  mime_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  format: "pptx",
  title: "Fixture deck",
  source: {
    task_id: "task_example_01",
    attempt_no: 1,
    capability: "example.create" as Artifact["source"]["capability"],
    provider: PROVIDER as Artifact["source"]["provider"],
    account_id: ACCOUNT,
    adapter_id: ADAPTER,
    adapter_version: "1.0.0",
    provider_artifact_id: "provartifact_example_1",
    provider_url: "https://provider.example/artifacts/1",
    provider_url_expires_at: "2026-01-01T01:00:00.000Z",
  },
  context: { thread_id: "thread_example_01", project_id: null, bot_id: "bot_example" },
  storage: {
    retrieval_state: "local",
    local_path: "ab/abcd",
    sha256: "c".repeat(64),
    size_bytes: 4096,
    local_preview_path: "ab/abcd.preview",
  },
  lineage: { version: 1, parent_artifact_id: null },
  capabilities: { editable_via: [], export_formats: ["pptx", "pdf"] },
  trust: "untrusted_provider_output",
  sensitivity: "internal",
  created_at: "2026-01-01T00:02:00.000Z",
};

export const threadMapping: ThreadMapping = {
  mapping_id: "mapping_example_01",
  thread_id: "thread_example_01",
  epoch: 1,
  provider: PROVIDER as ThreadMapping["provider"],
  account_id: ACCOUNT,
  adapter_id: ADAPTER,
  provider_thread_id: "provthread_example_1",
  provider_project_id: null,
  provider_url: "https://provider.example/threads/1",
  last_synced_turn_index: 4,
  last_turn_fingerprint: "d".repeat(64),
  model_class: "reasoning",
  status: "active",
  on_divergence: "adopt",
  context_transfer_from: null,
  created_at: "2026-01-01T00:00:00.000Z",
  last_used_at: "2026-01-01T00:02:00.000Z",
};

export const routeDecision: RouteDecision = {
  decision_id: "decision_example_01",
  primary: {
    adapter_id: ADAPTER,
    account_id: ACCOUNT,
    pool_key: `${PROVIDER}:${ACCOUNT}:example-pool`,
    lane: "subscription",
    est_metered_usd: 0,
    requires_approval: false,
  },
  fallbacks: [
    {
      adapter_id: "adapt_example_alt",
      account_id: null,
      pool_key: null,
      lane: "metered",
      est_metered_usd: 0.42,
      requires_approval: true,
    },
  ],
  rejected: [
    { adapter_id: ADAPTER, account_id: "acct_example_02", reason: "pool_exhausted" },
  ],
  policy_version: "policy-1",
  explain: "subscription entitlement available; no metered cost",
};

export const adapterEvents: AdapterEvent[] = [
  { t: "submitted", provider_thread_id: "provthread_example_1", provider_url: null },
  {
    t: "reply",
    event: {
      type: "reply.text.delta",
      replyId: "reply_example_1",
      runId: "run_example_1",
      itemId: "item_example_1",
      delta: "hello",
      ts: 1,
    },
  } as AdapterEvent,
  { t: "progress", label: "researching", fraction: 0.5 },
  { t: "progress.heartbeat", elapsed_s: 30, last_change_at: "2026-01-01T00:00:30Z" },
  {
    t: "artifact.partial",
    ref: {
      provider: PROVIDER as never,
      provider_artifact_id: "provartifact_example_1",
      provider_url: "https://provider.example/artifacts/1",
      provider_url_expires_at: null,
    },
  },
  {
    t: "artifact.ready",
    ref: {
      provider: PROVIDER as never,
      provider_artifact_id: "provartifact_example_1",
      provider_url: "https://provider.example/artifacts/1",
      provider_url_expires_at: null,
    },
    meta: { artifact_id: "artifact_example_01", format: "pptx" },
  },
  { t: "model.observed", model: "example-fast" },
  {
    t: "quota.signal",
    pool_id: "example-pool",
    signal: {
      kind: "limit_banner",
      raw_excerpt: "approaching limit",
      observed_at: "2026-01-01T00:00:00.000Z",
      task_id: null,
    },
  },
  { t: "needs_user", reason: "challenge", message: "Solve verification in window" },
  {
    t: "detached",
    resume_token: {
      token: "opaque-token",
      adapter_id: ADAPTER,
      attempt_no: 1,
      issued_at: "2026-01-01T00:00:00.000Z",
      poll_after_s: 30,
    },
    poll_after_s: 30,
  },
  { t: "done", outcome: "success", text: "finished" },
  {
    t: "error",
    error: {
      class: "timeout",
      scope: "task",
      retryable: true,
      fallback_eligible: true,
      cooldown_s: 60,
      user_action: null,
      detail: "watchdog fired",
      evidence_ref: null,
    },
  },
];
