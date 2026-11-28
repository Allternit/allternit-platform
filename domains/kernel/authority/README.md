# allternit-authority

The AllternitOS kernel interface: one `Authority` trait (the six calls plus admit) and the os/v1
wire types it carries. **Trait and types only.** There is no store, no signing and no network code
in this crate. Spec: `handoffs/allternit-os-2026-11-27/CONTRACT.md` sections 2 to 5 and 11.

| Module | What it holds |
|---|---|
| `types` | `ApprovalRequestV1`, `ApprovalTokenV1`, `SecretUseV1`, `BudgetReservationV1`, `DeployAdmissionV1`, `ActionClass`, `AutonomyLevel`, `Principal`/`PrincipalKind` (human, agent, bot, run), `KernelReceipt` with `ReceiptKind` |
| `calls` | `Request` / `Response` for check, lease, secret.use, approval.request, approval.redeem, audit.append, budget.reserve/commit/release, deploy.admit, plus the wire envelopes |
| `error` | `AuthorityError` (code, message, receipt id) |
| `framing` | 4-byte big-endian length + JSON, max 1 MiB (std only) |
| `policy` | `class_rule(class, autonomy) -> Allow \| Ask \| HumanOnly`, the CONTRACT 4.2 table as a pure function. The five human-only classes (money, client_message, deploy, secret_new_host, irreversible) are never `Allow` |

`approval.resolve` is not in the trait on purpose: only a human session route can resolve an approval.

## Features

`std` (default) and `serde` (default). `--no-default-features` is `no_std` + `alloc`: types, the trait,
errors and policy, with JSON bodies as raw strings. Frozen ABI types (`PolicyCheckV1`, `PolicyDecisionV1`,
`LeaseV1`, `ReceiptChainV1`) stay opaque JSON objects here; `allternit-kernel-abi` owns them.
The types mirror `spec/Contracts/os/v1/generated/rust` (PR #3279) field for field; the build train
checks they agree, and `tests/conformance.rs` decodes the os/v1 conformance examples.

## Three depths, one set of messages

- **Hosted (`allternit-api`, cloud-api):** an implementation backed by SQL (`kernel_receipts`,
  `approval_token_spends`) is embedded and called in-process with `Authority::call`. The gate sits
  inside `execute_tool_internal` and `dispatch_call`, not the HTTP layer.
- **PID 1 (`os/`):** the kernel serves the same JSON over a 0600 Unix socket using `framing`. The
  caller kind comes from `SO_PEERCRED`, never from the message. This crate replaces the Stage 2
  `os/authority-stub` (PR #3275).
- **Kernel IPC (Stage 3):** the same JSON payload as the IPC message body.

## Verify

```
cargo test -p allternit-authority
cargo build -p allternit-authority --no-default-features
```
