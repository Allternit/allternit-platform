# Checkpoint — session/kimi-router-gsu (subscription-fabric P3 gate)

## Goal
Land the SubstrateRouter guest_service_url forwarding fix (HANDOFF §6 blocker — kills proxy + PTY ssh lanes), then continue the P3 cloud gate on the Sessions machine.

## Just did
- Verified guest setup had never run (log: `/opt/subsfab/setup.sh: No such file or directory`; bundle never extracted). Extracted + relaunched via approval relay (grants 19e6c90b, d04fd52b).
- Setup then failed at pnpm install: bundle archive was missing `patches/` (pnpm-workspace.yaml patchedDependencies follow-redirects@1.15.11 + webpack@5.106.2). Regenerated gateway.tar.gz with patches/, rebuilt setup-bundle.tar.gz, uploaded (grant 18b4d68b), relaunched setup (grant 5782257b). Awaiting completion.
- Applied the 5-line fix: `guest_service_url` forwarding in `impl ExecutionDriver for SubstrateRouter` (cmd/allternit-computer-cloud/src/router.rs, placed before get_desktop_endpoint_by_native_id). Comm check: it was the ONLY trait method the router didn't forward (comm verified against platform/contracts/driver-interface).
- Tests: router_forwards_guest_service_url_to_handle_driver (real IncusDriver over scripted HTTP, existing svc6010 proxy device → http://host:36010) + router_guest_service_url_without_driver_errors_from_routing (no-driver error must NOT be the trait default's "guest service url"). cargo test -p allternit-computer-cloud router → 7/7 pass.

## Next
- Commit + push + PR + merge (merge commit), sync shared checkout main.
- Run scratch allternit-api from this worktree on 18013 per HANDOFF §6 recommended path; verify proxy lane live (was 9 consecutive 501s).
- Gate steps §6.4: login (Eoj drives) → subs connect chatgpt → task run chat.create → kill -9 adoption check → image.generate + quarantine.

## Open questions
- None for the fix. Gate: whether ChatGPT login succeeds in the streamed display (selectors v1-unverified).
