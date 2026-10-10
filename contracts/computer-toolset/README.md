# Computer toolset contract

One contract, one executor, many model adapters.

| File | What |
|---|---|
| `allternit-computer-v1.json` | `allternit.computer.v1`: 17 members, identical names and input fields to Anthropic `computer_toolset_20260801`. Kept as the pixel-only reference for the Claude-native wire configs. |
| `allternit-computer-v2.json` | `allternit.computer.v2`: additive over v1 — the same 17 pixel members plus 8 structured members (`read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`, `run_subtask`, `run_parallel`) backed by the Allternit Driver sidecar. allternit-api serves this as the `computer` toolset; v1 calls remain valid. |
| `allternit-browser-v1.json` | `allternit.browser.v1`: 31 members, identical to Anthropic `browser_toolset_20260801` |
| `generate.mjs` | Writes the TS (`sdk/computer-toolset`, gizzi `runtime/tools/computer-toolset/contract.gen.ts`) and Python (`domains/computer-use/core/contracts/toolset_v1.py`) bindings. `--check` exits 1 on drift. TS keys `CONTRACTS.computer` at v1 (the Claude-native configs may only name native members) and exports `COMPUTER_V2_CONTRACT` alongside; Python keys `CONTRACTS["computer"]` at v2 (a superset, so membership checks pass for both generations). |
| `tools/import-anthropic-sdk.mjs` | Refreshes member descriptions and input schemas from a published `@anthropic-ai/sdk`. Only run on an Anthropic toolset bump. |

allternit-api reads the JSON directly (`include_str!` in `computer_toolset.rs`), so the executor
cannot drift from the contract.

## Per-member metadata (ours, not Anthropic's)

- `risk`: `reversible` / `risky` / `irreversible`, mapped onto `aci_safety::ConfirmationClass`.
- `default_enabled`: offered to the model unless a target or config turns it off. Browser
  `file_upload`, `read_console`, `read_network` and `javascript_exec` are off by default.
- `needs_confirm` + `confirm`: `non_sandbox` (type, key, hold_key, form_input, run_batch,
  use_credential) needs a single-use action-hash approval grant on this-device and paired
  computers; `always` (javascript_exec, file_upload) always needs one. Any `irreversible`
  member needs one. `act`'s text-entering ops (set_value, select, press) upgrade to the same
  approval per call on non-sandbox targets, in code (`computer_v2::v2_member_needs_approval`).
- `result_kind` / `ack_text`: what a successful call returns (Anthropic's SDK ack texts).
- `scale_fields`: input fields holding coordinates (`point` = `[x, y]`, `rect` = `[x0, y0, x1, y1]`,
  `target` = a browser `{type:"coordinate", x, y}` target). The executor scales these from the model
  frame to screen pixels; nothing else scales.

## Batch rule

Calls of one turn run in order; after the first failure, every later call returns `is_error: true`
with exactly `batch_halt_text`:

- computer: `Not executed: an earlier computer action in this turn failed.`
- browser: `Not executed: an earlier action in this turn failed.`

## Model frame

Screenshots are downscaled (never upscaled) so the long edge is at most 1568 px and the area at most
1.15 MP. The model sees and answers in that frame; the executor maps back to screen pixels. Optional
`coordinate_space: "normalized_1000"` lets grid-trained GUI models (UI-TARS, Qwen-VL) send 0–1000
coordinates instead.
