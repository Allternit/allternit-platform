# Work Queue API

Self-hosted sandbox workers poll this endpoint for tasks rather than receiving pushed work. The protocol is intentionally small: lease, heartbeat, acknowledge, or stop.

All routes are nested under `/api/v1/beta/work`.

> Base URL: `http://localhost:8013/api/v1`  
> Auth: `Authorization: Bearer <clerk_jwt>`  
> Lease duration: 60 seconds

---

## Enqueue a task

`POST /beta/work`

Callers enqueue tasks that workers will later lease. A task may optionally be tied to a managed session or a deployment.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `session_id` | string | no | Existing beta session to associate with the task. |
| `deployment_id` | string | no | Existing deployment to associate with the task. |
| `sandbox_image` | string | no | Container image the worker should run, e.g. `alpine:latest`. |
| `env` | object | no | Environment variables. Defaults to `{}`. |
| `payload` | object | no | Worker payload. Defaults to `{}`. |

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/work \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "session_id": "sess_01J3X8X8X8X8X8X8X8X8X8X8",
    "sandbox_image": "alpine:latest",
    "env": {"FOO": "bar"},
    "payload": {"command": "echo hello"}
  }'
```

### Response

```json
{
  "task": {
    "id": "task_01J3X8X8X8X8X8X8X8X8X8X8",
    "session_id": "sess_01J3X8X8X8X8X8X8X8X8X8X8",
    "deployment_id": null,
    "status": "queued",
    "payload": {"command": "echo hello"},
    "sandbox_image": "alpine:latest",
    "env": {"FOO": "bar"},
    "lease_worker_id": null,
    "lease_expires_at": null,
    "result": null,
    "error": null,
    "created_at": "2026-08-09T09:30:00Z",
    "updated_at": "2026-08-09T09:30:00Z"
  },
  "id": "task_01J3X8X8X8X8X8X8X8X8X8X8"
}
```

---

## List tasks

`GET /beta/work[?status=<status>]`

Returns the caller's tasks, newest first. Filter by one of `queued`, `leased`, `running`, `succeeded`, `failed`, or `cancelled`.

### Example

```bash
curl "http://localhost:8013/api/v1/beta/work?status=running" \
  -H "Authorization: Bearer $CLERK_JWT"
```

---

## Lease a task

`GET /beta/work/queue?worker_id=<worker_id>`

A worker calls this endpoint to claim the oldest available task. Leasing is exclusive: the task moves to `leased`, `lease_worker_id` is set, and `lease_expires_at` is 60 seconds in the future.

If the worker crashes or stalls, the lease expires and another worker can reclaim the task.

### Example

```bash
curl "http://localhost:8013/api/v1/beta/work/queue?worker_id=worker-1" \
  -H "Authorization: Bearer $CLERK_JWT"
```

### Response

```json
{
  "task": {
    "id": "task_01J3X8X8X8X8X8X8X8X8X8X8",
    "status": "leased",
    "lease_worker_id": "worker-1",
    "lease_expires_at": "2026-08-09T09:31:00Z",
    ...
  }
}
```

When no task is available, `task` is `null`.

---

## Heartbeat a task

`POST /beta/work/:id/heartbeat`

Renews the lease for another 60 seconds and transitions the task to `running`.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `worker_id` | string | yes | Must match the current `lease_worker_id`. |

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/work/task_01J3X8X8X8X8X8X8X8X8X8X8/heartbeat \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{"worker_id": "worker-1"}'
```

---

## Acknowledge a task

`POST /beta/work/:id/ack`

Marks the task as `succeeded`, stores the result, and clears the lease.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `worker_id` | string | yes | Must match the current `lease_worker_id`. |
| `result` | object | no | JSON result payload. |

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/work/task_01J3X8X8X8X8X8X8X8X8X8X8/ack \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "worker_id": "worker-1",
    "result": {"output": "hello"}
  }'
```

---

## Stop a task

`POST /beta/work/:id/stop`

Cancels the task. The owner may stop a task before it is leased; if `worker_id` is provided, the caller must hold the active lease.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `worker_id` | string | no | Required when stopping a leased/running task. |
| `error` | string | no | Optional cancellation reason. |

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/work/task_01J3X8X8X8X8X8X8X8X8X8X8/stop \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{"worker_id": "worker-1", "error": "user cancelled"}'
```

### Response

```json
{"stopped": true}
```

---

## Worker loop example

```bash
#!/usr/bin/env bash
set -euo pipefail
WORKER_ID="worker-$(hostname)"
API="http://localhost:8013/api/v1"

while true; do
  TASK=$(curl -s -H "Authorization: Bearer $CLERK_JWT" \
    "$API/beta/work/queue?worker_id=$WORKER_ID")
  ID=$(echo "$TASK" | jq -r '.task.id // empty')
  [ -z "$ID" ] && { sleep 5; continue; }

  # run the payload
  if run_sandbox "$TASK"; then
    curl -s -X POST -H "Authorization: Bearer $CLERK_JWT" \
      -H "Content-Type: application/json" \
      -d "{\"worker_id\":\"$WORKER_ID\",\"result\":{\"ok\":true}}" \
      "$API/beta/work/$ID/ack"
  else
    curl -s -X POST -H "Authorization: Bearer $CLERK_JWT" \
      -H "Content-Type: application/json" \
      -d "{\"worker_id\":\"$WORKER_ID\",\"error\":\"execution failed\"}" \
      "$API/beta/work/$ID/stop"
  fi
done
```

---

## Task statuses

| Status | Meaning |
|--------|---------|
| `queued` | Waiting for a worker. |
| `leased` | Claimed by a worker but not yet running. |
| `running` | Worker has heartbeated at least once. |
| `succeeded` | Worker reported success. |
| `failed` | Reserved for future executor use. |
| `cancelled` | Stopped by owner or worker. |

---

## Status codes

| Status | Meaning |
|--------|---------|
| 201 | Task created. |
| 200 | List/get/heartbeat/ack/stop succeeded. |
| 400 | `env` or `payload` is not an object, or referenced session/deployment does not exist. |
| 404 | Task not found, not owned by caller, or lease held by another worker. |
