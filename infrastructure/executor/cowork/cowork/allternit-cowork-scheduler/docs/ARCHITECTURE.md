# allternit-cowork-scheduler Architecture

## Goals

1. Provide a lightweight, Rust-native cron scheduler for cowork runs.
2. Remain independent of `allternit-openclaw-host` so it can run as a satellite.
3. Integrate with Rails through a simple HTTP API for triggering runs.
4. Offer a pluggable job handler interface for tests and alternate backends.

## Components

### `Scheduler`

Central orchestrator that owns:

- A `tokio_cron_scheduler::JobScheduler` for cron execution.
- An `Arc<RwLock<SqliteStore>>` for persistence.
- A `String` API base URL.
- An `Arc<dyn JobHandler>` for executing triggered jobs.

On creation it initializes the SQLite store but does not start the scheduler; callers must invoke `start()`.

### `SqliteStore`

SQLite-backed schedule store with the following schema:

```sql
CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    cron TEXT NOT NULL,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    enabled INTEGER NOT NULL DEFAULT 1,
    entrypoint TEXT NOT NULL,
    args TEXT NOT NULL DEFAULT '[]',
    env TEXT NOT NULL DEFAULT '{}',
    priority INTEGER NOT NULL DEFAULT 0,
    timeout_secs INTEGER NOT NULL DEFAULT 3600,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_triggered_at TEXT,
    next_run_at TEXT,
    owner TEXT NOT NULL,
    run_mode TEXT NOT NULL DEFAULT 'scheduled'
);
```

The store serializes `args` and `env` as JSON strings for SQLite compatibility.

### `JobHandler` / `ApiJobHandler`

`JobHandler` is an async trait with a single method:

```rust
async fn execute(&self, ctx: JobContext) -> Result<()>;
```

`ApiJobHandler` is the default implementation. It POSTs a JSON payload to `{api_url}/api/v1/runs` with tenant/workspace/initiator/mode/entrypoint/args/env.

Custom handlers can be supplied via `Scheduler::with_handler` for testing or alternative execution backends.

### Axum API (`api.rs`)

REST API mapping to scheduler operations:

- `GET /schedules` → `list_schedules`
- `POST /schedules` → `create_schedule`
- `GET /schedules/:id` → `get_schedule`
- `PATCH /schedules/:id` → `update_schedule` (supports `active`/`paused`/`disabled` status)
- `DELETE /schedules/:id` → `delete_schedule`
- `POST /schedules/:id/run` → `run_now`
- `GET /status` → `get_stats`
- `POST /wake` → `wake_due_schedules`

Errors are returned as JSON `{ "error": "..." }` with appropriate HTTP status codes.

### Cron Execution

When a schedule is created or enabled:

1. The cron expression is validated using `cron_parser`.
2. `next_run_at` is computed from the current UTC time.
3. An async `tokio-cron-scheduler` job is added that invokes the configured handler.
4. On successful execution the store's `last_triggered_at` is updated.

Disabled/deleted schedules are skipped at trigger time. The current `tokio-cron-scheduler` version does not support removing jobs by ID, so jobs remain registered but are no-ops when disabled.

### Wake Behavior

`wake_due_schedules()` provides an explicit manual check:

1. Load all enabled schedules.
2. For each schedule with a `next_run_at` in the past:
   - Skip if already triggered since `next_run_at`.
   - Otherwise call `run_now()` and record the trigger.
3. Return the list of triggered schedule IDs.

This is useful for cold-start scenarios or when an external caller wants to force a check.

## Concurrency Model

- `Scheduler` is wrapped in `Arc<RwLock<_>>` by the API server so multiple requests can share it.
- `SqliteStore` uses a shared `sqlx` pool; SQLite handles row-level locking.
- Cron jobs are spawned by `tokio-cron-scheduler` internally and hold clones of the handler and store references.

## Future Work

- Remove/disable individual cron jobs when schedules are disabled or deleted.
- Timezone-aware cron parsing (currently uses UTC).
- Track per-schedule run counts and failure counts.
- Add OpenTelemetry metrics and tracing integration.
- Support schedule update for name/cron/entrypoint fields beyond status changes.
