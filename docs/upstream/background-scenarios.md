# Background task and print-mode scenarios

The canonical lifecycle owner is `runtime/session/background-task.ts`. A task is durable before its
worker starts, belongs to exactly one parent session, optionally points to a child session, and has
one terminal transition. A daemon restart changes orphaned `queued`/`running` rows to `interrupted`.

| Scenario | Expected contract |
|---|---|
| Foreground subagent | Parent tool waits and receives the final handoff directly. |
| Background subagent | Tool immediately returns both background-task ID and resumable child-session ID. |
| Completion | Persist output first, publish `background.task.finished`, then optionally steer parent. |
| Failure | Persist structured failure text and notify with `status: failed`. |
| Cancel | Verify parent ownership, cancel the child session, settle once as `cancelled`. |
| Late worker settlement | Cannot overwrite an existing terminal state. |
| Daemon restart | Active task becomes `interrupted`; it is never presented as still running. |
| Parent busy at completion | Synthetic notification is durably queued and drives the next turn. |
| Parent idle at completion | Synthetic notification starts a new turn immediately. |
| Print `exit` | Do not steer; return after the main turn. |
| Print `drain` | Suppress parent steering, wait for all observed tasks, then return. |
| Print `steer` | Completion steers parent turns until quiescent, ceiling, or maximum turn count. |
| Wait ceiling | Stop withholding CLI completion when the configured wall-clock ceiling expires. |
| Multiple completions | Preserve every notification as a durable user message; queued messages share the next turn. |

Notification payloads explicitly mark subagent output as untrusted data. They never interpolate it
into the system prompt. Background lifecycle events are available to CLI, web, and IDE clients, and
the REST session routes provide list/cancel operations.
