# Open Dots (Anil-matcha) native AAI adapter

Primary source: https://github.com/Anil-matcha/open-dots at commit 7f6b23e9a46a088063b17f8748f772b0770727f4 (backend/app/routers/local.py, task.py, local_access_policy.py, services/task_execution_service.py).

Uses the local Claude lane of the Open Dots backend at `http://127.0.0.1:8000` (override `SUBS_GATEWAY_OPENDOTS_ANIL_URL`; must be loopback). Open Dots has no sign-in of its own: the adapter enforces the loopback origin, sends no credential, and refuses redirects. Open Dots itself refuses local execution from any proxy, tunnel, forwarded request or untrusted Host header. Open Dots' own README states it is single-owner local use.

Connect with your Open Dots `user_id` (the sealed account credential, or deployment `SUBS_GATEWAY_OPENDOTS_ANIL_USER_ID`). Requires `LOCAL_EXECUTION_ENABLED=true` on the Open Dots backend and a signed-in Claude CLI on the same machine. Routes used: `GET /api/v1/local/status`, `POST /api/v1/local/tasks`, `GET /api/v1/local/tasks/{id}`.

Each message is one local run. Open Dots gives it a fresh session, so nothing carries over between messages. Replies come from `result` of a `succeeded` run. A run that enters `waiting_approval` is reported as needing approval in Open Dots. This adapter never calls `/ask/{id}/answer`, so approvals stay in Open Dots. No cancellation, streaming, memory or computer control is advertised.

Send outcomes and contexts are kept in process; nothing resumes across gateway restarts. A send whose run has not finished within the timeout is reported as unconfirmed; check Open Dots before sending again.

Source-based checks only: static type-check passes. Live verification against a running Open Dots backend remains pending.
