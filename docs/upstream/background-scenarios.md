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

## Claude CLI-owned work

`runtime/session/cli-background-tasks.ts` maps stream-json `task_started`,
`task_progress`, and `task_notification` events into the durable table used by
`SessionTasks.list`. Bash background tool results register jobs too (including
automatic backgrounding), correlate their actual output files, and retain at
most 64KB. IDs include the parent session to avoid collisions across CLI runs.
Only matching task output files under a real temporary root are read; paths are
resolved before validation so symlinks to unrelated files are rejected.

The stream-json driver completes the foreground model stream on `result` but
retains the CLI and consumes its task/control messages while work remains. The
normal process registry still owns shutdown. When the connection ends early,
remaining rows settle as `interrupted`, preserving the latest output. A task
notification is not converted into a new synthetic model instruction by this
bridge. Existing native-task notification/steering behavior is unchanged.

A surface's Stop calls the registered canceller, which sends the CLI SDK
`stop_task` control for that vendor task ID and waits for its response. Rejection
or timeout must propagate rather than marking the task cancelled. Provider
terminal notifications settle directly, without recursively requesting another
stop. The CLI is released once its final task settles; other tasks in the same
CLI are not killed when one task is stopped.

Regression: `cmd/gizzi-code/test/session/cli-background-tasks.test.ts` exercises
real subprocess/stdio, durable storage, the shared list, live file output,
foreground/background lifetime separation, targeted stop, rejected stop,
terminal idempotence, unknown statuses, owner scoping and crash interruption.
