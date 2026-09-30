# Allternit Kernel ABI Package 1.0.0 (FROZEN 2026-09-30)

The frozen contract package for the Allternit agent kernel: 21 JSON Schema (draft 2020-12)
files defining **65 contracts**, the canonical primitive registry (**191 ids**), and the
built-in completion data (BUG_FIX policy + criteria). Approved by Eoj on 2026-09-30
(`09-v1.4-disposition.md` §E, in `Research/agency-kernel-reconciliation-2026-09-29/`).

```
MANIFEST.json            package metadata, version axes, contract list, decisions Q1–Q7, deferrals
schemas/*.json           normative contracts ($id https://schemas.allternit.com/kernel/1.0.0/<file>)
registry/primitives.json PrimitiveRegistryV1: 191 dotted ids + UPPER_SNAKE aliases
data/                    CompletionCriterionV1 built-ins, CompletionPolicyV1 for BUG_FIX
conformance/             pytest suite + valid/invalid examples + seed fixture
generated/ts/            TypeScript types (types only, no runtime deps)
generated/rust/          `allternit-kernel-abi` crate (serde derive only, standalone workspace)
scripts/regenerate.sh    reproduces both generated outputs
```

## Authority order

Living Architecture v1.4 > 07 decisions + 09 disposition > 02 canonical contracts >
**this ABI package** > conformance/reference packs (v1.2/v1.4 seeds) > code > donors.
Where a lower layer disagrees with a higher one, the higher one wins and the mismatch is
recorded, never silently inherited.

## Four version axes

| Axis | Value | Meaning |
|---|---|---|
| doc | Living Architecture v1.4 | the architecture this package implements |
| abi_package | **1.0.0** | this whole package (MANIFEST `package_version`) |
| runtime | unbound | runtimes declare `required_abi_versions`; none is bound here |
| per_contract | `V1` suffix + `schema_version` 1.0.0 | contract major in the name; semver per contract |

## Freeze rules

1. Frozen files are never edited in place. Errata that do not change validation results
   ship as a patch release (1.0.x).
2. **Additive changes** (new optional field, new enum member, new contract) need an ABI
   **minor** bump (1.1.0). They are safe only because **receivers fail closed**: an unknown
   enum value or an unknown field is rejected (every object is `additionalProperties:false`).
3. Unknown *optional* data travels in `extensions`, a map whose keys must match `^x-`.
   Every closed object carries it (09 §E Q3).
4. **Breaking changes** (removing/renaming a field, tightening a type, changing semantics)
   need **2.0.0** and a new contract major (`V2`).
5. Encodings (Q4): hashes are `sha256:<64 lowercase hex>`; timestamps are RFC 3339 strings.
6. No vendor or model names anywhere in schemas, registry or data (tested).

## Decisions applied at freeze (09 §E)

Q1 191 primitive ids · Q2 no `gen.*`/`vcs.*` namespaces (generation is class G in its domain
namespace; VCS under `obs.vcs.*`/`mut.vcs.*`) · Q3 closed contracts + `extensions` ·
Q4 encodings above · Q5 AgentStateV1 has no locks/lifecycle/lease/campaign fields, keeps
`budgets` as resolved limits and `user_visible_status` as display-only · Q6 Primitive Pack
and decision-bank manifests **deferred to 1.1** (referenced by opaque ids; BUG_FIX pack ships
as data) · Q7 class legend D deterministic, S System-1, G generative, V verifier, P policy,
M memory/state, R routing/resources. Phase-A classes were inferred under a different legend;
only P, V and G (same meaning) are kept — 25 ids classed, **166 null** until confirmed
against the §16 matrix.

Deferred to 1.1+: DeploymentProfileV1, DatasetReceiptV1, PrimitivePackManifestV1,
DecisionBankManifestV1, DecisionCalibrationManifest deep content (WP8). API v0.3
`content_hash` prefix alignment is an API change tracked outside this package.

## Tests

```sh
cd spec/Contracts/kernel/v1
uvx --with jsonschema --with referencing --with rfc3339-validator pytest conformance -q
(cd generated/rust && cargo test)
```

Python: every schema passes `check_schema`; every `$ref` resolves; each contract has a
valid example that passes and ≥1 invalid example that fails; unknown enum values and
unknown fields are rejected (09 §C #5); `extensions` accepts only `x-*` keys; encodings;
vendor-name scan; BUG_FIX `allow_partial=false`; registry shape; AgentState split.
Rust: every valid example round-trips through the generated serde types.

Codegen note: json-schema-to-typescript and typify cannot express cross-file refs or
conditional rules, so `scripts/bundle_for_codegen.py` makes a non-normative bundle with
`if/then/not/propertyNames` stripped. Conditional rules (e.g. RETRY requires `changed`,
AUTO forbidden when uncalibrated, COMPLETE requires all six predicates) are enforced by
schema validation only.

## Ported and rejected v1.4 reference behaviour

Ported (as ABI-level checks): vendor-name scan (`src/conformance.py VENDOR_PATTERNS`,
extended); capability ids in `cap.*` form; static graph conformance, **inverted** — the
seed BUG_FIX graph must be rejected. Runtime tests that conform (state-store CAS,
calibration invalidation on candidate-set change, oracle headroom, router fallback,
capability routing without model identity) have no runtime in this package and move to
WP2+ with the kernel.

Rejected (conflict with authority; see `10-seed-diff.md` §4):

| Seed behaviour | Why rejected |
|---|---|
| `graphs/bug_fix.v1.json`: S1 node `dec.assess_completion` → `complete` edge finishes the run | a model decides completion; completion is SYSTEM-decided via L2882 (CompletionDecisionV1.decided_by = SYSTEM) |
| `fixture_runtime.complete()` / `test_fixture_run` (tests_passed + string match ⇒ succeeded) | lacks 3 of 5 BUG_FIX criteria; `allow_partial=false` |
| `reference_runtime.py` hard-coded `authorize_mutation:"allow"`, `assess_completion:"complete"` | forces policy and completion; violates policy-floor laws 2–4 |
| `test_bugfix_graph_static_conformance` passing a graph with `gen.*` ids, non-`cap.*` capabilities, no completion node | static gate weaker than 02 §7; inverted into `test_seed_bug_fix_graph_is_rejected` |
| donor/model names in reference runtime + tests (`remote-jev`, GLiNER backend, "AnyJev-like") | no kernel coupling to vendors/models |
| `capability_id=="decision.tool_selection"` | capability ids must be `cap.*` |
| unix-float timestamps, bare-hex hashes | Q4 encodings |
