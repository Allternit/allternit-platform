# Session checkpoint — CLI background task telemetry

## Goal
Fix missing Claude CLI background tasks in the Tasks pane, ship the runtime and update Desktop.

## Just did
- Runtime fixes merged in #1532 and #1534; docs integration lint correction in #1533.
- 60 relevant tests, typecheck and production compilation pass. Real media QA streamed and completed; final Stop test cancelled with preserved output and no connection error.
- Installed Desktop 1.1.7.5638, runtime 2.2.6, source 33b05dd12b968bd9af48e32bd70ed7fd50283219.
- Official 2.2.6 npm/GitHub release succeeded on all five platforms. Homebrew tap #2 merged and local gizzi upgraded to 2.2.6. Verified archive hashes and refreshed all packaging manifests.
- Recordly remains stopped; no media QA process remains.

## Next
Land the packaging/verification record and remove this session's temporary checkout. Resume the telemetry promotion from a fresh, short recording when Eoj starts Recordly.

## Open questions / known limits
Claude CLI can merge an old automated-task reminder with a new request on resume; it can ignore that request. This provider limitation is documented and is not claimed fixed. Shared allternit checkout fast-forward is blocked by other sessions' overlapping dirty source files. Do not stash or reset them. Repo-wide git discipline has pre-existing stale branches; no clean shared-state claim is made.
