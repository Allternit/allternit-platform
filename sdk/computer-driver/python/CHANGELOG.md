# Changelog

## 0.2.0 — 2026-10-10

### Added

- **Contract v2 (`allternit.computer.v2`) structured members.** Typed client methods for all ten driver-backed members — `read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`, `run_subtask`, `run_parallel`, `run_skill`, `skills` — on the new `ComputerV2Driver` (exported from the package root). Input types are generated from `contracts/computer-toolset/allternit-computer-v2.json` (`allternit_computer_driver/_v2_types.py`, emitted by `contracts/computer-toolset/generate.mjs`), so they cannot drift from the server contract; result dicts are documented in the class docstring.
- **`computer_v2` function tool next to the pixel tool in every adapter**, with the same steering guidance gizzi sends: `ComputerV2Tool` (anthropic module), `openai_computer_v2_tool` + `run_openai_v2_call`, `gemini_computer_v2_declaration` + `run_gemini_v2_call`, plus the provider-neutral `computer_v2_tool()`.
- **Approval flow helper.** `client.toolset_with_approval(computer_id, call, on_approval)` answers a 409 `approval_required` hold (approve + resend with the single-use `approval_grant`) for any member, pixel or structured.
- **Typed errors.** `ComputerV2Error` (a structured member answered `is_error`), `ComputerBusyError` (423 `computer_busy` / `computer_controlled_elsewhere`), `SandboxRequiredError` (409 `sandbox_required`) and `ComputerConflictError` (409 `computer_conflict`).

### Notes

- Requires the Allternit Driver sidecar on the computer for the structured members (this-device today). The 17 pixel members are unchanged.
