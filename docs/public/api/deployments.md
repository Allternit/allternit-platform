# Deployments API

A deployment binds an agent to a cron schedule and tracks every triggered execution in a separate run history. `next_run_at` is computed from the cron expression whenever a deployment is created, updated, or triggered.

All routes are nested under `/api/v1/beta/deployments`.

> Base URL: `http://localhost:8013/api/v1`  
> Auth: `Authorization: Bearer <clerk_jwt>`

---

## Create a deployment

`POST /beta/deployments`

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent_id` | string | no | Agent to invoke on each scheduled run. |
| `cron` | string | yes | Standard five-field cron expression. |
| `metadata` | object | no | Arbitrary key/value object. Defaults to `{}`. |

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/deployments \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "agent_id": "agent_01J3X8X8X8X8X8X8X8X8X8X8",
    "cron": "0 9 * * *",
    "metadata": {"region": "us-east", "team": "platform"}
  }'
```

### Response

```json
{
  "deployment": {
    "id": "dep_01J3X8X8X8X8X8X8X8X8X8X8",
    "agent_id": "agent_01J3X8X8X8X8X8X8X8X8X8X8",
    "cron": "0 9 * * *",
    "next_run_at": "2026-08-10T09:00:00Z",
    "last_run_at": null,
    "status": "active",
    "metadata": {"region": "us-east", "team": "platform"},
    "created_at": "2026-08-09T09:30:00Z",
    "updated_at": "2026-08-09T09:30:00Z"
  },
  "id": "dep_01J3X8X8X8X8X8X8X8X8X8X8"
}
```

Invalid cron expressions return `400 Bad Request`.

---

## List deployments

`GET /beta/deployments[?status=active|paused|archived]`

Returns the caller's deployments, newest first.

### Example

```bash
curl "http://localhost:8013/api/v1/beta/deployments?status=active" \
  -H "Authorization: Bearer $CLERK_JWT"
```

### Response

```json
{
  "deployments": [
    {
      "id": "dep_01J3X8X8X8X8X8X8X8X8X8X8",
      "agent_id": "agent_01J3X8X8X8X8X8X8X8X8X8X8",
      "cron": "0 9 * * *",
      "next_run_at": "2026-08-10T09:00:00Z",
      "last_run_at": null,
      "status": "active",
      "metadata": {"region": "us-east", "team": "platform"},
      "created_at": "2026-08-09T09:30:00Z",
      "updated_at": "2026-08-09T09:30:00Z"
    }
  ]
}
```

---

## Get a deployment

`GET /beta/deployments/:id`

### Example

```bash
curl http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8 \
  -H "Authorization: Bearer $CLERK_JWT"
```

---

## Update a deployment

`PATCH /beta/deployments/:id`

`next_run_at` is recomputed on every successful update, even when only `status` changes.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `agent_id` | string | no | Change the bound agent. |
| `cron` | string | no | New schedule; recomputes `next_run_at`. |
| `status` | string | no | `active`, `paused`, or `archived`. |
| `metadata` | object | no | Replaces the existing metadata object. |

### Example

```bash
curl -X PATCH http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8 \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{"status": "paused"}'
```

---

## Delete a deployment

`DELETE /beta/deployments/:id`

Removes the deployment and cascades to its run history.

### Example

```bash
curl -X DELETE http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8 \
  -H "Authorization: Bearer $CLERK_JWT"
```

---

## Run history

### List runs

`GET /beta/deployments/:id/runs`

Returns runs for a deployment, newest first.

### Example

```bash
curl http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8/runs \
  -H "Authorization: Bearer $CLERK_JWT"
```

### Response

```json
{
  "runs": [
    {
      "id": "run_01J3X8X8X8X8X8X8X8X8X8X8",
      "deployment_id": "dep_01J3X8X8X8X8X8X8X8X8X8X8",
      "status": "succeeded",
      "result": {"tickets_closed": 3},
      "error": null,
      "started_at": "2026-08-09T09:00:00Z",
      "finished_at": "2026-08-09T09:01:12Z"
    }
  ]
}
```

### Trigger a run

`POST /beta/deployments/:id/runs`

Starts a run immediately, updates `last_run_at`, and recomputes `next_run_at` from the deployment cron.

### Example

```bash
curl -X POST http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8/runs \
  -H "Authorization: Bearer $CLERK_JWT"
```

### Response

```json
{
  "run": {
    "id": "run_01J3X8X8X8X8X8X8X8X8X8X8",
    "deployment_id": "dep_01J3X8X8X8X8X8X8X8X8X8X8",
    "status": "running",
    "result": null,
    "error": null,
    "started_at": "2026-08-09T09:30:00Z",
    "finished_at": null
  },
  "id": "run_01J3X8X8X8X8X8X8X8X8X8X8"
}
```

### Update a run

`PATCH /beta/deployments/:id/runs/:run_id`

Used by the executor to report the final state of a run.

### Request body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status` | string | yes | `succeeded`, `failed`, or `cancelled`. |
| `result` | object | no | JSON result payload. |
| `error` | string | no | Error message when status is `failed` or `cancelled`. |

### Example

```bash
curl -X PATCH http://localhost:8013/api/v1/beta/deployments/dep_01J3X8X8X8X8X8X8X8X8X8X8/runs/run_01J3X8X8X8X8X8X8X8X8X8X8 \
  -H "Authorization: Bearer $CLERK_JWT" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "succeeded",
    "result": {"tickets_closed": 3}
  }'
```

---

## Status codes

| Status | Meaning |
|--------|---------|
| 201 | Deployment or run created. |
| 200 | List/get/update succeeded. |
| 204 | Deployment deleted. |
| 400 | Invalid cron expression or unsupported status. |
| 404 | Deployment or run not found, or not owned by caller. |
