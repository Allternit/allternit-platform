# AllternitOS Contracts Package os/v1 1.0.0 (DRAFT, gate G0)

The wire contracts for the AllternitOS authority kernel, the model shell and the inbox:
three JSON Schema (draft 2020-12) files defining **31 contracts** (8 types and 23 call
bodies, results and tool shapes), built from `CONTRACT.md` (AllternitOS Stage 0, step 2,
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
MANIFEST.json              package metadata, version axes, contract list (file, def, kind, source), open conflicts, sha256 per file
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

## Open conflicts for G0

The schemas follow the frozen ABI where CONTRACT.md and the ABI disagree, and the
conformance suite pins each fact so nobody wires around it by accident.

1. **`extensions["os.v1"]`** (CONTRACT.md 2.1, 2.2, 2.9). The ABI `ExtensionMap` only
   accepts keys that match `^x-[a-z0-9_.-]+$`, so a `PolicyCheckV1` or `LeaseV1` carrying
   `extensions["os.v1"]` fails ABI validation. `CheckExtensionV1` and `LeaseExtensionV1`
   define the fields. They are not wired into the ABI types until G0 picks a fix: an
   `x-os.v1` key, or an ABI 1.1 minor bump.
2. **Receipt domain** (CONTRACT.md 2.5, 5). The ABI fixes `ReceiptChainV1.domain` to the
   const `allternit.receipt.v1` and `schema_id` to `^allternit\.kernel\.`. The per-kind
   domains (`check`, `approval`, ...) in the `kernel_receipts` table and any `allternit.os.*`
   receipt cannot travel in a `ReceiptChainV1`. `AuditAppendBody.domain` follows the ABI.
3. **Hash encoding.** CONTRACT.md uses bare 64-hex (`Sha256Hex`) for `action_hash` and
   digests. The ABI's Q4 rule is `sha256:<hex>`. The schemas use the contract's bare hex.
4. **`new_app_id`** (3.8, "Plus `new_app_id`"). The contract does not say where this field
   goes. It is a top-level `ShellCommandV1` field, required for `fork_app`.
5. **Unspecified types.** The contract gives no type for `region`, `runs[].status` or the
   `prepare` result. `region` and `status` are strings, and the `prepare` result has no
   schema.

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
without its payload hash, signature, key id or expiry is refused; open conflict 1.

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
