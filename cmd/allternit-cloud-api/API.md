# Cowork Runtime API Reference

Complete REST API documentation for the Cowork Runtime system.

**Base URL:** `http://localhost:8080` (default)

**Content-Type:** `application/json`

---

## Table of Contents

- [Runs](#runs)
- [Events](#events)
- [Schedules](#schedules)
- [Approvals](#approvals)
- [Attachments](#attachments)
- [Code Environments](#code-environments)
- [Common Types](#common-types)
- [Error Responses](#error-responses)

---

## Runs

Runs are the core execution units in the Cowork Runtime. A run represents a single execution of a task, job, or workflow.

### Run Status

| Status | Description |
|--------|-------------|
| `pending` | Created but not started |
| `planning` | Planning execution steps |
| `queued` | Waiting for execution slot |
| `running` | Actively executing |
| `paused` | Paused (approval needed or manual) |
| `completed` | Successfully finished |
| `failed` | Failed with error |
| `cancelled` | Cancelled by user |

### Run Modes

| Mode | Description |
|------|-------------|
| `local` | Local VM execution (Apple VF/Firecracker) |
| `remote` | Remote VPS execution (allternit-node) |
| `cloud` | Cloud-managed execution (Kubernetes, etc.) |

---

### Create Run

Create a new run in the Cowork Runtime.

```http
POST /api/v1/runs
```

**Request Body:**

```json
{
  "name": "My Run",
  "description": "Optional description",
  "mode": "local",
  "config": {
    "command": "./script.sh",
    "args": ["--verbose"],
    "working_dir": "/workspace",
    "env": {
      "API_KEY": "secret123"
    },
    "resource_limits": {
      "memory_mb": 2048,
      "cpu_cores": 2.0,
      "disk_gb": 10,
      "timeout_seconds": 3600
    },
    "sync": {
      "enabled": true,
      "watch_patterns": ["**/*.rs"],
      "ignore_patterns": ["target/**"],
      "bidirectional": false
    }
  },
  "auto_start": true
}
```

**Response:**

```json
{
  "id": "run-uuid-123",
  "name": "My Run",
  "description": "Optional description",
  "mode": "local",
  "status": "pending",
  "step_cursor": null,
  "total_steps": null,
  "completed_steps": 0,
  "config": { ... },
  "owner_id": null,
  "tenant_id": null,
  "runtime_id": null,
  "runtime_type": null,
  "schedule_id": null,
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z",
  "started_at": null,
  "completed_at": null,
  "error_message": null,
  "error_details": null
}
```

---

### List Runs

List all runs with optional filtering.

```http
GET /api/v1/runs
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | string | Filter by status (comma-separated for multiple) |
| `mode` | string | Filter by mode: `local`, `remote`, `cloud` |
| `owner_id` | string | Filter by owner ID |
| `limit` | integer | Maximum number of results (default: 100) |
| `offset` | integer | Number of results to skip |

**Example:**

```http
GET /api/v1/runs?status=running,pending&mode=local&limit=20
```

**Response:**

```json
[
  {
    "id": "run-uuid-123",
    "name": "My Run",
    "mode": "local",
    "status": "running",
    "completed_steps": 5,
    "total_steps": 10,
    "created_at": "2024-01-15T10:30:00Z",
    "updated_at": "2024-01-15T10:35:00Z"
  }
]
```

---

### Get Run

Get detailed information about a specific run.

```http
GET /api/v1/runs/:id
```

**Response:**

```json
{
  "id": "run-uuid-123",
  "name": "My Run",
  "description": "Optional description",
  "mode": "local",
  "status": "running",
  "step_cursor": "step-5",
  "total_steps": 10,
  "completed_steps": 5,
  "config": { ... },
  "owner_id": null,
  "tenant_id": null,
  "runtime_id": "vm-123",
  "runtime_type": "local",
  "schedule_id": null,
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:35:00Z",
  "started_at": "2024-01-15T10:31:00Z",
  "completed_at": null,
  "error_message": null,
  "error_details": null
}
```

---

### Update Run

Update run metadata (name and description only).

```http
PATCH /api/v1/runs/:id
```

**Request Body:**

```json
{
  "name": "Updated Run Name",
  "description": "Updated description"
}
```

---

### Delete Run

Delete a run and all associated data.

```http
DELETE /api/v1/runs/:id
```

**Response:** `204 No Content`

---

### Start Run

Start a pending or paused run.

```http
POST /api/v1/runs/:id/start
```

**Response:** Returns the updated run object.

---

### Pause Run

Pause a running run.

```http
POST /api/v1/runs/:id/pause
```

**Response:** Returns the updated run object.

---

### Resume Run

Resume a paused run.

```http
POST /api/v1/runs/:id/resume
```

**Response:** Returns the updated run object.

---

### Cancel Run

Cancel a run (terminal state).

```http
POST /api/v1/runs/:id/cancel
```

**Response:** Returns the updated run object.

---

### Attach to Run

Attach a client to a run for event streaming.

```http
POST /api/v1/runs/:id/attach
```

**Request Body:**

```json
{
  "client_type": "terminal",
  "user_id": "user-123"
}
```

**Client Types:** `terminal`, `web`, `desktop`, `mobile`, `api`

**Response:**

```json
{
  "client_id": "client-uuid-456",
  "run_id": "run-uuid-123",
  "attached": true
}
```

---

### Detach from Run

Detach a client from a run.

```http
POST /api/v1/runs/:id/detach
```

**Request Body:**

```json
{
  "client_id": "client-uuid-456"
}
```

---

### Get Run Attachments

List all clients attached to a run.

```http
GET /api/v1/runs/:id/attachments
```

**Response:**

```json
[
  {
    "id": "attach-uuid-789",
    "run_id": "run-uuid-123",
    "client_id": "client-uuid-456",
    "client_type": "terminal",
    "user_id": "user-123",
    "cursor_sequence": 42,
    "attached_at": "2024-01-15T10:30:00Z",
    "last_seen_at": "2024-01-15T10:35:00Z",
    "detached_at": null
  }
]
```

---

## Events

Events form an append-only ledger for each run. All outputs, state changes, and milestones are recorded as events.

### Event Types

| Type | Description |
|------|-------------|
| `run_created` | Run was created |
| `run_started` | Run started executing |
| `run_completed` | Run completed successfully |
| `run_failed` | Run failed |
| `run_cancelled` | Run was cancelled |
| `run_paused` | Run was paused |
| `run_resumed` | Run was resumed |
| `step_started` | A step started |
| `step_completed` | A step completed |
| `step_failed` | A step failed |
| `stdout` | Standard output |
| `stderr` | Standard error |
| `output` | General output |
| `tool_call` | Tool was called |
| `tool_result` | Tool returned result |
| `approval_needed` | Approval required |
| `approval_given` | Approval granted |
| `approval_denied` | Approval denied |
| `checkpoint_created` | Checkpoint was created |
| `checkpoint_restored` | Checkpoint was restored |

---

### Get Run Events

Get events for a run with pagination.

```http
GET /api/v1/runs/:id/events
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `cursor` | integer | Sequence number to start from |
| `limit` | integer | Maximum events to return |

**Response:**

```json
[
  {
    "id": "event-uuid-001",
    "run_id": "run-uuid-123",
    "sequence": 1,
    "event_type": "run_created",
    "payload": {
      "run_name": "My Run",
      "mode": "local"
    },
    "source_client_id": null,
    "source_client_type": null,
    "created_at": "2024-01-15T10:30:00Z"
  },
  {
    "id": "event-uuid-002",
    "run_id": "run-uuid-123",
    "sequence": 2,
    "event_type": "stdout",
    "payload": {
      "content": "Hello, World!\n"
    },
    "source_client_id": "runtime-123",
    "source_client_type": "local",
    "created_at": "2024-01-15T10:31:00Z"
  }
]
```

---

### Stream Run Events (SSE)

Stream events in real-time using Server-Sent Events.

```http
GET /api/v1/runs/:id/events/stream
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `cursor` | integer | Start from this sequence number (includes historical events) |

**Example:**

```bash
curl -N http://localhost:8080/api/v1/runs/{run-id}/events/stream
```

The stream includes both historical events (if cursor provided) and new events as they occur. The connection stays open until the run completes or the client disconnects.

---

## Schedules

Schedules enable cron-based periodic execution of runs.

### Misfire Policies

| Policy | Description |
|--------|-------------|
| `ignore` | Skip missed executions |
| `fire_once` | Execute once when system recovers (default) |
| `fire_all` | Execute all missed runs |

---

### List Schedules

```http
GET /api/v1/schedules
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `enabled` | boolean | Filter by enabled status |
| `limit` | integer | Maximum results |
| `offset` | integer | Results to skip |

**Response:**

```json
[
  {
    "id": "sched-uuid-001",
    "name": "Daily Backup",
    "enabled": true,
    "cron_expr": "0 2 * * *",
    "natural_lang": "daily at 2am",
    "next_run_at": "2024-01-16T02:00:00Z",
    "run_count": 15
  }
]
```

---

### Create Schedule

```http
POST /api/v1/schedules
```

**Request Body:**

```json
{
  "name": "Daily Backup",
  "description": "Backup database daily",
  "cron_expr": "0 2 * * *",
  "natural_lang": "daily at 2am",
  "timezone": "America/New_York",
  "job_template": {
    "command": "./backup.sh",
    "args": ["--full"],
    "env": {
      "BACKUP_DIR": "/backups"
    },
    "working_dir": "/app",
    "timeout_seconds": 3600
  },
  "enabled": true,
  "misfire_policy": "fire_once"
}
```

---

### Get Schedule

```http
GET /api/v1/schedules/:id
```

---

### Update Schedule

```http
PATCH /api/v1/schedules/:id
```

**Request Body:** (all fields optional)

```json
{
  "name": "Updated Name",
  "cron_expr": "0 3 * * *",
  "enabled": false
}
```

---

### Delete Schedule

```http
DELETE /api/v1/schedules/:id
```

---

### Enable Schedule

```http
POST /api/v1/schedules/:id/enable
```

---

### Disable Schedule

```http
POST /api/v1/schedules/:id/disable
```

---

### Trigger Schedule

Manually trigger a schedule to create a run immediately.

```http
POST /api/v1/schedules/:id/trigger
```

**Response:**

```json
{
  "message": "Schedule triggered",
  "schedule_id": "sched-uuid-001",
  "run_id": "run-uuid-456",
  "run": { ... }
}
```

---

## Approvals

The approval system provides human-in-the-loop checkpoints for autonomous runs.

### Approval Status

| Status | Description |
|--------|-------------|
| `pending` | Waiting for approval |
| `approved` | Approved, run can continue |
| `denied` | Denied, run should stop |
| `timed_out` | Timed out waiting for response |
| `cancelled` | Cancelled by the run |

### Priority Levels

| Priority | Description |
|----------|-------------|
| `low` | Low priority |
| `normal` | Normal priority (default) |
| `high` | High priority |
| `critical` | Critical priority |

---

### List Approvals

```http
GET /api/v1/approvals
```

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `run_id` | string | Filter by run ID |
| `status` | string | Filter by status |
| `limit` | integer | Maximum results (max: 100) |

**Response:**

```json
[
  {
    "id": "approval-uuid-001",
    "run_id": "run-uuid-123",
    "status": "pending",
    "priority": "high",
    "title": "Approve file deletion",
    "action_type": "file_delete",
    "created_at": "2024-01-15T10:30:00Z",
    "responded_at": null
  }
]
```

---

### Get Approval

```http
GET /api/v1/approvals/:id
```

**Response:**

```json
{
  "id": "approval-uuid-001",
  "run_id": "run-uuid-123",
  "step_cursor": "step-5",
  "status": "pending",
  "priority": "high",
  "title": "Approve file deletion",
  "description": "The run wants to delete /tmp/old-data.csv",
  "action_type": "file_delete",
  "action_params": {
    "path": "/tmp/old-data.csv"
  },
  "reasoning": "File is no longer needed per cleanup policy",
  "requested_by": null,
  "responded_by": null,
  "response_message": null,
  "timeout_seconds": 300,
  "created_at": "2024-01-15T10:30:00Z",
  "responded_at": null
}
```

---

### Create Approval

```http
POST /api/v1/approvals
```

**Request Body:**

```json
{
  "run_id": "run-uuid-123",
  "step_cursor": "step-5",
  "priority": "high",
  "title": "Approve file deletion",
  "description": "The run wants to delete /tmp/old-data.csv",
  "action_type": "file_delete",
  "action_params": {
    "path": "/tmp/old-data.csv"
  },
  "reasoning": "File is no longer needed",
  "requested_by": "agent-1",
  "timeout_seconds": 300
}
```

---

### Approve Request

```http
POST /api/v1/approvals/:id/approve
```

**Request Body:**

```json
{
  "approved": true,
  "message": "Approved - file can be deleted"
}
```

---

### Deny Request

```http
POST /api/v1/approvals/:id/deny
```

**Request Body:**

```json
{
  "approved": false,
  "message": "Keep the file for now"
}
```

---

### Cancel Approval

```http
POST /api/v1/approvals/:id/cancel
```

---

## Attachments

Attachments track clients watching a run.

### Attachment Model

```json
{
  "id": "attach-uuid-001",
  "run_id": "run-uuid-123",
  "client_id": "client-uuid-456",
  "client_type": "terminal",
  "user_id": "user-123",
  "cursor_sequence": 42,
  "attached_at": "2024-01-15T10:30:00Z",
  "last_seen_at": "2024-01-15T10:35:00Z",
  "detached_at": null
}
```

---

## Code Environments

Cloud environments for Code mode sessions. An environment is a per-user recipe
a cloud Code session starts in: network policy, setup script, `.env` text,
repos to clone, and write-only secrets. Spec: `openapi/code-environments.yaml`.

**Auth:** Clerk session, an `allternit_*` API token with the `compute` scope,
or a runtime device token (`Bearer allternit_runtime_…`) acting as its owner.
Rows are scoped to the caller; another user's id answers 404.
`GET /:id/resolve` accepts **only** a runtime device token owned by the
environment's user.

**Secrets:** values are encrypted at rest with the platform AES-256-GCM
credential cipher (`ALLTERNIT_CREDENTIALS_KEY`, the same key as provider tokens
and BYOK keys). Without that key, the secret routes and `/resolve` answer
`503 {"error":"code_environment_secrets_not_configured"}`. Values are never
logged and never returned except by `/resolve`.

### CodeEnvironment

```json
{
  "id": "6f1c…",
  "name": "Default",
  "network_access": "full",
  "allowed_hosts": ["github.com", "registry.npmjs.org", "…"],
  "secrets": [{ "name": "NPM_TOKEN", "hosts": ["registry.npmjs.org"], "created_at": "2026-10-20T12:00:00Z" }],
  "setup_script": "npm ci",
  "env_vars": "NODE_ENV=development\n",
  "repos": [{ "url": "https://github.com/acme/web.git", "branch": "main" }],
  "is_default": true,
  "archived": false,
  "created_at": "2026-10-20T12:00:00Z",
  "updated_at": "2026-10-20T12:00:00Z"
}
```

Validation (422 `VALIDATION_ERROR` on failure): `name` 1–80 characters;
`network_access` one of `full`, `limited`, `none`; hosts are hostnames or
`*.` wildcards (max 200); `setup_script` and `env_vars` at most 64 KB each;
repos are `https://`, `ssh://` or `git@host:owner/repo` URLs (max 20), branch
optional; secret names match `^[A-Z_][A-Z0-9_]*$`, values at most 64 KB, max
100 secrets per environment.

New environments get the default `allowed_hosts` list (GitHub plus the npm,
Yarn, Bun, PyPI, crates.io, Go proxy, RubyGems, Maven Central, Debian and
Ubuntu package hosts — see `DEFAULT_ALLOWED_HOSTS` in
`src/routes/code_environments.rs`) unless the request sends its own list.

### List Environments

```http
GET /api/v1/code-environments
GET /api/v1/code-environments?archived=1
```

Returns `{ "environments": CodeEnvironment[] }`, default first, then oldest
first. Archived rows only with `?archived=1`. A user with no live environment
gets one named "Default" (`network_access: "full"`, `is_default: true`)
created on the spot.

### Create Environment

```http
POST /api/v1/code-environments
```

Body: `name` (required), `network_access`, `allowed_hosts`, `setup_script`,
`env_vars`, `repos`, `is_default`. Returns `201` with the `CodeEnvironment`.
`is_default: true` clears the flag on the user's other environments.

### Update Environment

```http
PATCH /api/v1/code-environments/:id
```

Any subset of the create fields; omitted fields are unchanged. Making an
archived environment the default answers `409`.

### Archive / Unarchive

```http
POST /api/v1/code-environments/:id/archive
POST /api/v1/code-environments/:id/unarchive
```

Returns the `CodeEnvironment`. Archiving clears `is_default`.

### Set Secret

```http
POST /api/v1/code-environments/:id/secrets
```

Body `{ "name": "NPM_TOKEN", "value": "…", "hosts": ["registry.npmjs.org"] }`.
Upserts by name and returns the `CodeEnvironment` (names and hosts only).

### Delete Secret

```http
DELETE /api/v1/code-environments/:id/secrets/:name
```

Returns the `CodeEnvironment`; `404` when no such secret.

### Resolve (runtime only)

```http
GET /api/v1/code-environments/:id/resolve
Authorization: Bearer allternit_runtime_…
```

Returns the `CodeEnvironment` plus `secret_values: { "NAME": "value" }`. Used
by gizzi-code when a cloud session starts in the environment. A Clerk session
or API token answers `401`; a device owned by another user answers `404`.

---

## Common Types

### RunConfig

```json
{
  "command": "string",
  "args": ["string"],
  "working_dir": "string",
  "env": {
    "KEY": "value"
  },
  "resource_limits": {
    "memory_mb": 2048,
    "cpu_cores": 2.0,
    "disk_gb": 10,
    "timeout_seconds": 3600
  },
  "sync": {
    "enabled": true,
    "watch_patterns": ["**/*.rs"],
    "ignore_patterns": ["target/**"],
    "bidirectional": false
  }
}
```

### JobConfig

```json
{
  "command": "string",
  "args": ["string"],
  "env": {
    "KEY": "value"
  },
  "working_dir": "string",
  "timeout_seconds": 3600
}
```

---

## Error Responses

All errors follow this format:

```json
{
  "error": {
    "code": "not_found",
    "message": "Run not found: run-uuid-123"
  }
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `bad_request` | 400 | Invalid request parameters |
| `unauthorized` | 401 | Authentication required |
| `forbidden` | 403 | Permission denied |
| `not_found` | 404 | Resource not found |
| `conflict` | 409 | Resource conflict |
| `internal_error` | 500 | Server error |

---

## WebSocket API

For real-time bidirectional communication, use WebSocket connections:

```
ws://localhost:8080/ws/runs/:id
```

WebSocket messages are JSON-encoded and follow the same event format as the SSE endpoint.

---

## SDK Examples

### JavaScript/TypeScript

```typescript
// Create a run
const run = await fetch('http://localhost:8080/api/v1/runs', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    name: 'Test Run',
    mode: 'local',
    config: { command: 'echo Hello' },
    auto_start: true
  })
}).then(r => r.json());

// Stream events
const eventSource = new EventSource(
  `http://localhost:8080/api/v1/runs/${run.id}/events/stream`
);

eventSource.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log(data.event_type, data.payload);
};
```

### Python

```python
import requests

# Create a run
run = requests.post('http://localhost:8080/api/v1/runs', json={
    'name': 'Test Run',
    'mode': 'local',
    'config': {'command': 'echo Hello'},
    'auto_start': True
}).json()

# Get events
import sseclient
response = requests.get(
    f'http://localhost:8080/api/v1/runs/{run["id"]}/events/stream',
    stream=True
)
client = sseclient.SSEClient(response)
for event in client.events():
    data = json.loads(event.data)
    print(data['event_type'], data['payload'])
```

---

## Rate Limiting

API requests are rate-limited per client:

- **Default:** 1000 requests per hour
- **Burst:** 100 requests per minute

Rate limit headers are included in responses:

```http
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 999
X-RateLimit-Reset: 1642252800
```
