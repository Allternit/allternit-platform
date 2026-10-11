# Bot redesign feedback, Computer gaps, and current-main builds

Eoj authorized completion of all accumulated UI feedback and the Computer gap review, followed by building all new pushes, explicitly UI #515. Implementation is merged: UI #514, #516, #517, #518, #519, #520 (final UI 883d82bb26fc04996a7a61a6d3df0a0cfc01b001; includes #515 6cf9c89f). Platform #1503 dedb1bf0 and #1504 5619d673.

## Completed behavior

Workspace Edit with chat is below the editor; session chat has bounded reading width. The banner pencil is the sole Edit bot entry; thread Bot settings opens that bot's Settings tab. Integrations use fresh auth/current gateway, bounded requests, owned tool catalog metadata and visible retry. Messaging uses channel brand marks. A:// uses the wordmark in autonomy/spend/chips/suggestions; mascot hover identity follows the actual bot. Avatars are unframed, including the team strip added by #515. Rail pointer drag follows the grab offset, places/replaces/swaps slots, persists layout and cancels with Escape. Window capture release prevents a stuck preview when release leaves the source row. Appearance offers 119 icons, search, 20 banner colors, and choose/drop/paste custom images. Project Details banners match bot customization; project appearance affordance appears on hover/focus and remains usable on touch.

All eight Computer review areas are accounted for: approvals restore pending state; watcher steering routes to the running/open/main thread; hand-back notes send only after control returns; retained run history has step images/thumbnails; replay/scrubbing returns to live; clipboard has manual fallback and controller ACL; files use bounded ordered 1 MB chunks through the 5 MB phone relay, with a 25 MB total; shared compact layouts and shortcuts preserve phone behavior. Server history is private, redacted and bounded (30 days/500 steps/100 MB image budget). Missing old snapshots have an explicit empty state. Historical images cannot be reconstructed before this feature.

Desktop fixes also handle the optional macOS socket QoS EINVAL without swallowing unrelated errors, and show the installed app version in splash/logs. Native API release binary SHA256: 0ad818a49cf5755565eafd5bee35bcf9a6ec1c3125ccf5a89c348c9b6814623f. Factory/Gizzi native source was unchanged, so known working binaries were retained. Pinned computer driver 0.34.0, Lima 2.1.2 and Lume 0.3.9 are staged in the installer.

## Verification

- Native API release build and Clippy completed. 168 Computer tests, 3 appearance tests and 2 chunk transfer tests passed; baseline Clippy warnings remain.
- All 399 Desktop tests passed after the shell version fix. UI TypeScript is clean; focused rail, mascot/team strip, integration, history/replay and chunk transfer suites passed. Final release-outside-row regression suite: 11/11.
- Docs: 527 nav entries/522 pages, 0 problems. Dependency map valid.
- Live installed app: workspace chat below editor; Settings navigation; 20 colors/48 symbols/search/custom image controls; catalog loaded 8661 tools; generic Computer watcher/history/transfer controls; actual 2 MB + 7 byte upload over three chunks matched SHA256, with the temporary control lease released and own file deleted. No clipboard contents read and no paid sandbox provisioned.
- Final live rail test on build 5512: Engineer placed, Designer replaced its occupied slot, ordinary Engineer click opened its home with Edit bot in the banner. Original empty pin roster restored. Launch/team avatars have no wrapper rings or white vendor tiles.
- Installed local API is healthy (db/jwks/gizzi ready), runtime bad-version list empty. UI #515 is present in the rebuilt source and bundled UI. Cloud Linux API and docs deploy jobs passed.

## Build and publication

The final packaging/publication consumes this merged documentation-only commit after source QA. Desktop 1.1.6 is a local development installer: Apple signing/notarization credentials are unavailable; do not represent the DMG as an Apple-notarized public release. Runtime updates are Ed25519 signed. Production ai main deploy and manual phone PWA deploy share the same final UI; phone service-worker cache is v109. Final build/runtime URLs and hashes are recorded in the durable completion file below.

Evidence and final artifact record: `/Users/joe/Desktop/allternit-workspace/handoffs/bot-redesign-2026-10-11/completion.md` and `evidence/`. Earlier verification.md entries saying unpublished/incomplete are historical and superseded by this attestation and completion record.

## Incidents and preservation

Live QA caught both a catalog metadata mismatch and an actual stuck drag despite source tests; both were fixed, with new regression coverage and another live check. The 5 MB relay constraint required chunked transfer. A missing driver/Lima staging issue was corrected before final packaging. No other session's active Terminal agent, uncommitted files or unmerged branches were modified. Steering consult backend is unavailable and the normal hook records CONSULT_FAILED/fails open; it was not disabled. Shared trees retain pre-existing WIP; final hygiene evidence uses the checker-supported explicit preservation list for other sessions' unmerged branches, with soft-dirty warnings recorded honestly. Only this session's worktrees, merged branches and scratch/build targets are removed after final shipment.
