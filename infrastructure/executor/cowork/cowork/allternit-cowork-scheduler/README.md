# allternit-cowork-scheduler

Rust satellite crate for the Allternit cowork scheduler.

## Purpose

`allternit-cowork-scheduler` provides cron-based job scheduling for cowork runs. It is designed as a standalone satellite service that integrates with the Rails backend via HTTP, without requiring a dependency on the full `allternit-openclaw-host` scheduler stack.

Key capabilities:

- **Cron expression scheduling** powered by `tokio-cron-scheduler`.
- **SQLite persistence** for schedule definitions and trigger history.
- **REST API** for schedule CRUD, manual triggers, and status queries.
- **Manual wake endpoint** for triggering due schedules on demand.
- **Pluggable job handlers** for testing and custom execution backends.

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                 Scheduler Daemon / Library                  │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │   Scheduler │  │  SqliteStore │  │    Axum API      │   │
│  │             │  │              │  │                  │   │
│  │ - cron jobs │  │ - schedules  │  │ - CRUD           │   │
│  │ - wake due  │  │ - stats      │  │ - trigger/wake   │   │
│  └──────┬──────┘  └──────────────┘  └──────────────────┘   │
│         │                                                   │
│    JobHandler::execute                                      │
│         │                                                   │
└─────────┼───────────────────────────────────────────────────┘
          │ HTTP POST /api/v1/runs
┌─────────┴───────────────────────────────────────────────────┐
│                    Rails / API Server                       │
└─────────────────────────────────────────────────────────────┘
```

- `Scheduler` coordinates the `tokio-cron-scheduler` instance and the SQLite store.
- `SqliteStore` persists schedules, including enabled state, cron expression, and last/next run times.
- `ApiJobHandler` (default) triggers cowork runs by posting to the configured API URL.
- The Axum API layer exposes endpoints compatible with the Gizzi CLI cron command.

## API Summary

### Scheduler library

- `Scheduler::new(db_path, api_url).await` — create with default API handler.
- `Scheduler::with_handler(db_path, api_url, handler).await` — create with custom handler.
- `create_schedule(owner, req).await` — validate cron and persist a schedule.
- `get_schedule(id)`, `list_schedules()`, `list_schedules_for_owner(owner)` — read schedules.
- `enable_schedule(id)`, `disable_schedule(id)` — toggle schedule state.
- `delete_schedule(id)` — remove a schedule.
- `run_now(id)` — trigger a schedule immediately.
- `wake_due_schedules()` — manually check and trigger all due enabled schedules.
- `get_stats()` — return total/enabled schedule counts.

### HTTP API

| Method | Path                | Description                              |
|--------|---------------------|------------------------------------------|
| GET    | `/schedules`        | List all schedules.                      |
| POST   | `/schedules`        | Create a schedule.                       |
| GET    | `/schedules/:id`    | Get a schedule by ID.                    |
| PATCH  | `/schedules/:id`    | Update name/cron/entrypoint or pause.    |
| DELETE | `/schedules/:id`    | Delete a schedule.                       |
| POST   | `/schedules/:id/run`| Trigger a schedule immediately.          |
| GET    | `/status`           | Scheduler status and counts.             |
| POST   | `/wake`             | Trigger all due schedules.               |

## Build & Run

From the workspace root:

```bash
# Check the crate
cargo check -p allternit-cowork-scheduler

# Run tests
cargo test -p allternit-cowork-scheduler

# Run the scheduler daemon once
cargo run -p allternit-cowork-scheduler -- --once

# Run the scheduler daemon
cargo run -p allternit-cowork-scheduler -- --api-url http://127.0.0.1:3000 --port 3031
```

## JSON Schemas

Schema definitions for the schedule and scheduler stats resources are in `schemas/v1/`.

## Testing

Integration tests in `tests/integration_tests.rs` cover:

- Schedule CRUD operations.
- Cron validation (invalid expressions rejected).
- API endpoint responses using a mock job handler.
- `wake_due_schedules` triggering behavior.

## License

MIT — see the workspace `LICENSE` file.
