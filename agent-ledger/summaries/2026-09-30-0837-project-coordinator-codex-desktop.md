# Unified project coordinator and Desktop installation

Eoj authorized commit, land, build, and local Desktop installation. Backend PR [#1001](https://github.com/Gizziio/allternit-platform/pull/1001) merged at af4166416ed7f8971d7fca6b1c4a9d9301a72561. UI PR [#290](https://github.com/Gizziio/allternit-ai/pull/290) merged at 5be0e4a1834f68513ba6115fabf90a5103719da1.

Projects are one collection. Non-bot projects open their persistent coordinator conversation with task threads beside it and retain their existing Details view. Bot projects keep their specialized launcher. Separate coordinator/thread model and permission settings, project instructions, icon/color, native transcript streaming, task status, and persisted reports are wired through authenticated APIs. Internal workers and workspace registrations are excluded from user-facing bot/native project collections. Task sessions snapshot context/model/permission/directory, and requests route to the selected directory.

Verification: 18 backend coordinator tests and 20 focused UI tests passed; UI fast typecheck, ESLint, and production build passed. Backend PR checks and UI Cloudflare preview passed. Release preflight: 52 passed, 0 failed. Scratch runtime registration and native task creation passed HTTP and Desktop UI checks. Preview captures are evidence of the development renderer only.

Build/install status at initial attestation: current-main UI static export staged with clean manifest, behindMain=0; Desktop shell and service staging complete; Gizzi 2.1.8 production arm64 build succeeded; optimized API build in progress. Whole-bundle installation and installed-app smoke verification will be recorded below after completion. No installed-app asset patching performed.

Repository incident: shared backend and UI main fast-forwards are blocked by other sessions’ dirty tracked files and untracked UI files. Those files are preserved. Owned worktrees are built from merged main commits, not the stale shared checkouts. Attestation is landed through the owned branch because the shared checkout cannot safely be synchronized while that work is in flight.
