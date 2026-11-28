# AllternitOS Contracts Package os/v1 1.0.0 (DRAFT, gate G0)

The wire contracts for the AllternitOS authority kernel, the model shell and the inbox:
three JSON Schema (draft 2020-12) files defining **35 contracts** (8 types and 27 call
bodies, results, tool shapes and ABI attachments), built from `CONTRACT.md` (AllternitOS Stage 0, step 2,
in `handoffs/allternit-os-2026-11-27/`). Status is **DRAFT** until Eoj approves that
contract (gate G0). After G0 the MANIFEST status becomes `FROZEN` and the freeze rules
below apply.

## A sibling of the frozen Kernel ABI

This package sits next to `spec/Contracts/kernel/v1/` (Kernel ABI 1.0.0, FROZEN 2026-09-30)
and never edits it. Reused ABI types (`PolicyCheckV1`, `PolicyDecisionV1`, `LeaseV1`,
`TaskIRV1`, `ReceiptChainV1`, `AttentionRequestV1`, `Id`, `Timestamp`, `SemVer`,
`StateVersion`, `ResourceRef`, `ExtensionMap`) are referenced by `$ref` to their schema
URIs under `https://schemas.allternit.com/kernel/1.0.0/`, never redefined. Where the two
disagree the ABI wins (CONTRACT.md 2.9). The conformance suite checks the ABI MANIFEST
hashes, so a change that touches a frozen file fails here too.

```
MANIFEST.json              package metadata, version axes, contract list (file, def, kind, source), resolutions, sha256 per file
schemas/authority.schema.json  kernel envelope, the six calls plus admit (bodies and results), ApprovalRequestV1,
                               ApprovalTokenV1, SecretUseV1, BudgetReservationV1, DeployAdmissionV1, action classes, autonomy levels
schemas/inbox.schema.json      InboxItemV1, the inbox tool input (list, summarise, snooze, prepare; no approve) and results
schemas/shell.schema.json      ViewDescriptorV1, ShellStateV1, ShellCommandV1, shell tool result
conformance/               pytest suite + one valid and >=1 invalid example per contract
generated/ts/              TypeScript types (json-schema-to-typescript, same tool and version as the ABI)
generated/rust/            `allternit-os-contracts` crate (serde only, standalone workspace, depends on `allternit-kernel-abi`)
scripts/regenerate.sh      reproduces both generated outputs and the MANIFEST hashes
```

## Versioning

| Axis | Value | Meaning |
|---|---|---|
| doc | CONTRACT.md (Stage 0, step 2) | the contract this package implements |
| os_package | **1.0.0** | this whole package (MANIFEST `package_version`) |
| requires_abi | Kernel ABI 1.0.0 | the frozen package every `$ref` points into |
| per_contract | `V1` suffix + `schema_version` 1.x.y | contract major in the name; semver per contract |

Types carry `schema_id` (`allternit.os.<Name>`), `schema_version` (`1.x.y`) and
`extensions` (the ABI `ExtensionMap`, `x-*` keys only). Call bodies and tool shapes are
written exactly as CONTRACT.md sections 2 and 7 give them, so they carry no `schema_id`.

Freeze rules (after G0), same as the ABI:

1. A breaking change (remove or rename a field, tighten a type, change meaning) needs a new
   contract major (`V2`) and package 2.0.0.
2. An additive change (new optional field, new enum member, new `ShellCommandV1.action`,
   new surface, new action class) is a minor bump (1.x.0). It is safe only because
   receivers fail closed: every object is `additionalProperties: false`, and an unknown
   enum value is rejected.
3. A new error code is additive. Callers treat an unknown code as `denied`.

## Reading rules applied to CONTRACT.md

- Section 3 tables: "Req yes" fields are required; "server" fields are optional in the
  schema because the kernel fills them in.
- Section 2 and 7 JSON bodies: every field shown is required; `<x|null>` means nullable;
  a field marked `?` (for example `limit?`) is optional.
- "... minus server fields" bodies (`ApprovalRequestBody`, `BudgetReserveBody`,
  `DeployAdmitBody`) keep their type's fields with the server fields not required.
  CONTRACT.md 3.1 names the ApprovalRequestV1 server fields. Sections 3.4 and 3.5 do not,
  so these are inferred: `reservation_id`, `status`, `receipt_id` for budgets, and
  `admission_id`, `status`, `receipt_id`, `action_hash` for deploys.
- Conditional rules the schema enforces: `approval.resolve` only from `caller.kind: human`;
  `approval.request` only from `run` or `service`; each `call` binds its body shape;
  an error other than `bad_request`, `frame_too_large` or `unsupported_version` must carry
  a `receipt_id`; `SecretUseV1` needs `target_host` for `http_header`/`connector_call`,
  `target_process` for `env_for_process`, and `approval_token` when `new_host` is true;
  `DeployAdmissionV1` in `admitted` needs `approval_token_id`; an `app` view needs
  `app_id`, `entry` and an `app:` view id; each `ShellCommandV1.action` needs its fields.
- Kernel rules a schema cannot express (the kernel enforces them): token TTL (300 s
  default, 900 s cap), the token signature, `action_hash` recomputation, a value-shaped
  `secret_ref`, "a human cannot approve its own run's request", and "protected views are
  built in only".

## Resolved conflicts (CONTRACT.md section 11)

Building the schemas turned up five places where CONTRACT.md and the frozen ABI disagreed
or the contract gave no type. CONTRACT.md section 11 settles each one, and the schemas
follow it:

1. **Extensions key: resolved.** os fields ride in `extensions["x-os-v1"]`, which the ABI
   `ExtensionMap` accepts. `PolicyCheckOsV1` is the check body (the ABI `PolicyCheckV1`
   with a `CheckExtensionV1` required under `x-os-v1`). `PolicyDecisionOsV1` and
   `LeaseOsV1` allow the optional `CheckExtensionV1` / `LeaseExtensionV1`. The ABI types are
   unchanged. Section 11.1 names no fields that belong only to the decision, so the
   decision echoes the check's fields. The old `"os.v1"` key is still refused (tested).
2. **Receipt domain: resolved.** `AuditAppendBody.domain` stays the one ABI value. The
   receipt kind (`check`, `approval`, `secret_use`, `budget`, `deploy`, `lease`) is the
   required `payload.kind`.
3. **Hashes: resolved.** Every hash is the ABI `ContentHash` (`sha256:<64 lowercase hex>`).
   The "payload_hash" of ApprovalTokenV1 in 11.3 is its `action_hash` field. The field
   name from 3.2 is kept.
4. **`new_app_id`: resolved.** It is a field of the `fork_app` `ShellToolResultV1`. A
   `fork_app` command that sends it is refused.
5. **Missing types: resolved.** `region` (in view instances and arrangement slots) matches
   `^[a-z]{2}-[a-z]+-[0-9]$` or is `"local"`. `runs[].status` is
   `queued | running | waiting | done | failed | stopped`. `InboxPrepareResult` is an inbox
   row with `state: "prepared"` and `draft {action_class, action_text, payload_hash}`.
   `action_text` is the field name chosen for "exact action text". A prepared item still
   needs a human approve.

## Tests

```sh
cd spec/Contracts/os/v1
uvx --with jsonschema --with referencing --with rfc3339-validator pytest conformance -q
(cd generated/rust && cargo test)   # build train only
```

Python checks: every schema passes `check_schema` with a pinned `$id`; every `$ref`
resolves (os and ABI); no ABI def is redefined; the frozen ABI hashes still match;
MANIFEST `files` hashes match; objects are closed and types carry `extensions`; each
contract has a valid example that passes and >=1 invalid example that fails; unknown enum
values fail closed. Contract rules: the five human-only classes; the inbox tool has no
approve, deny or resolve; resolve is refused from `run` and `service` callers; a token
without its payload hash, signature, key id or expiry is refused; os fields attach via
`x-os-v1` and `os.v1` is refused; every hash is the ABI `ContentHash`.

Rust: every valid example round-trips through the serde types, and the inbox tool type
rejects `approve`.

## Generated code

`generated/ts/os-contracts.d.ts` was produced by `SKIP_RUST=1 scripts/regenerate.sh`
(json-schema-to-typescript 15.0.4, the ABI's tool). `generated/rust/src/generated.rs` is
**hand-written** to match the schemas, because the train guard blocks cargo (and so
cargo-typify) in coding sessions. The build train runs `scripts/regenerate.sh` to replace
it with cargo-typify output, commits `generated/rust/Cargo.lock`, and re-hashes MANIFEST.
The codegen bundle (`scripts/bundle_for_codegen.py`) adds the ABI defs the os schemas
reach, inlines deep pointers such as `LeaseV1/properties/reclaim_policy`, and strips
conditional keywords, the same way as the ABI bundler.
