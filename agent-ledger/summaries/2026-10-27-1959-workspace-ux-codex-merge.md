# Workspace UX merge — Codex — 2026-10-27

Eoj requested implementation across Desktop, ai.allternit.com and the m.allternit.com PWA, then explicitly requested merge and a live view.

Merged UI PR https://github.com/Allternit/allternit-ai/pull/858 at `36749006695398a701bc131b6e0c828e8a23dbf3`; merged paired API/docs PR https://github.com/Allternit/allternit-platform/pull/1922 at `2a44894f7df5a3ebe45b40cbc4b64b74839ffa5d`. Both shared checkouts were synced to those merges. Only this task's original shared-checkout edits were removed after comparison and backup; unrelated files were preserved.

Changes: account-backed artifact archive/restore and kind grouping; real task-status filters on desktop and both phone shells; routine next-run timezone/owner/outcome/history/failure clarity and serialized actions; clearer existing artifact pane navigation. Migration 109 and the archive API preserve ACLs and sharing. Existing editors, shell rail, typography, composer tabs and Code/Terminal execution ownership remain.

Validation: fast UI typecheck, 24 focused frontend tests, 10 artifact API Postgres route tests, changed-file lint, docs/Mintlify validation, docs links and dependency-map validation passed. PR checks passed after adding factual CLI integration pages to the existing docs-lint exception list. Backend test compilation used an owned isolated target through the build queue.

Release state at wrap-up: both web deployments started automatically from UI main; Cloud API workflow https://github.com/Allternit/allternit-platform/actions/runs/40611855557 was running its test job. No manual production migration or service restart was performed. Tailscale SSH required a check; the existing documented public SSH-key route worked, so the check was no longer needed. The signed-in ai.allternit.com window was opened in Safari for showing the result after the deployment completes.

Desktop source contains the change, but the build queue reported no Desktop build containing UI merge `367490066`; the installed Desktop binary is not claimed updated. Per the release-train rule, sessions merge and the queued release builds Desktop.

Factory tracking: dag_417887 / wih_6427. Owned Agent Trees are being landed and removed after merge; no dev server was started. Remaining release verification is visible in the CI/Pages workflows, rather than being represented as completed.
