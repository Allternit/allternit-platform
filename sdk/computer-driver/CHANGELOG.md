# Changelog

## 0.2.0 — 2026-10-10

### Added

- **Contract v2 (`allternit.computer.v2`) structured members.** Typed client methods for all ten driver-backed members — `read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`, `run_subtask`, `run_parallel`, `run_skill`, `skills` — on the new `ComputerV2Driver` (exported from the package root). Input and result types are generated from `contracts/computer-toolset/allternit-computer-v2.json` (`src/v2-generated.ts`, emitted by `contracts/computer-toolset/generate.mjs`), so they cannot drift from the server contract.
- **`computer_v2` function tool next to the pixel tool in every adapter**, with the same steering guidance gizzi sends: `computerV2AnthropicTool` + `runAnthropicComputerV2` (anthropic subpath), `openaiComputerV2Tool` + `runOpenAIV2Call`, `geminiComputerV2Declaration` + `runGeminiV2Call`, plus the provider-neutral `computerV2Tool()`.
- **Approval flow helper.** `client.toolsetWithApproval(id, call, onApproval)` answers a 409 `approval_required` hold (approve + resend with the single-use `approval_grant`) for any member, pixel or structured.
- **Typed errors.** `ComputerV2Error` (a structured member answered `is_error`), `ComputerBusyError` (423 `computer_busy` / `computer_controlled_elsewhere`), `SandboxRequiredError` (409 `sandbox_required`) and `ComputerConflictError` (409 `computer_conflict`).

### Notes

- Requires the Allternit Driver sidecar on the computer for the structured members (this-device today). The 17 pixel members are unchanged.
