# D2 checkpoint (feat/computer-contract-v2)

Goal: Allternit Driver spec phase D2 — contract `allternit.computer.v2` (6 structured members added to the 17 pixel members), executor wiring (approvals+audit, run_batch, request_human lease pause), credential backends (vault/Keychain/Bitwarden/1Password/TOTP), Windows TCP transport for the sidecar client, per-family adapters + prompt guidance, docs, verification.

Just did: worktree `allternit-wt-d2-contract` off origin/main (d6799e78fd); explored executor (computer_toolset.rs), driver sidecar (read_ui/act/run_batch/verify already server-side), lease, aci_credentials vault (totp_code exists), gizzi adapters.

Next: write contracts/computer-toolset/allternit-computer-v2.json; update generate.mjs; Rust executor + computer_v2.rs + this_device_input TCP transport; gizzi computer_v2 tool; tests; docs; per-family live verification; PR + merge.

Open questions: guests unreachable (same as D1) — cloud-computer verification will be recorded as an open item. Bitwarden/1Password CLIs — check availability on this machine.
