// Request templates for the banks the harvest can label, copied verbatim from
// their live callers (instructions, candidates, primitive ids, state shape), so
// a harvested row binds to the same Q26 scope (candidate schema/set hashes) as a
// live shadow row. If a caller changes, update it here too (see `caller`).
import type { Candidate } from "../decision/contract.ts";
import { GEN_CLASSES, ROUTE_OPTIONS } from "./labels.ts";
import { buildPack } from "../hook/pack.ts";
import type { BankSpec } from "./types.ts";

const withUnknown = (opts: readonly string[]): Candidate[] => [
  ...opts.map((o) => ({ candidate_id: o, label: o })),
  { candidate_id: "unknown", label: "unknown", is_unknown: true },
];

const PERMISSION_INSTRUCTIONS = "Should this tool call proceed without asking the person first? Answer true only if it is clearly safe and routine.";

export const BANKS = {
  route: {
    bank: "bank.route.v0", primitive_id: "bank.route.v0", operation: "CHOICE", question_id: null,
    instructions: "classify what this turn needs", candidates: withUnknown(ROUTE_OPTIONS),
    caller: "cmd/gizzi-code/src/runtime/routing/turn-router.ts s1Decide(ROUTE_BANK, ...)",
  },
  route_model: {
    bank: "bank.route_model.v0", primitive_id: "bank.route_model.v0", operation: "CHOICE", question_id: null,
    instructions: "pick the smallest model class that can answer this turn", candidates: withUnknown(GEN_CLASSES),
    caller: "cmd/gizzi-code/src/runtime/routing/turn-router.ts s1Decide(ROUTE_MODEL_BANK, ...)",
  },
  permission_judge: {
    bank: "bank.permission_gate", primitive_id: "permission.commrails_judge", operation: "GATE", question_id: "may_proceed",
    instructions: PERMISSION_INSTRUCTIONS,
    caller: "commrails/src/gate/gate_judge.rs:938-960 (outcomes: cli_hook.* by cc-tool subject_ref)",
  },
  permission_cli_guard: {
    bank: "bank.permission_gate", primitive_id: "permission.cli_guard", operation: "GATE", question_id: "may_proceed",
    instructions: PERMISSION_INSTRUCTIONS,
    caller: "tools/system-one-local/src/hook/guard.ts:151-160 (state = buildPack JSON)",
  },
} satisfies Record<string, BankSpec>;

/** turn-router.ts sends the turn text, last 2,000 chars. */
export const routeState = (text: string) => text.slice(-2000);

/** gate_judge.rs: format!("tool: {tool}\ncommand: {}\npaths: {}", command preview (2000), paths.join(", ")). */
export function permissionState(tool: string, input: Record<string, unknown>): string {
  const cmd = typeof input.command === "string" ? input.command.slice(0, 2000) : "";
  const paths = ["file_path", "notebook_path", "path"].map((k) => input[k]).filter((v): v is string => typeof v === "string");
  return `tool: ${tool}\ncommand: ${cmd}\npaths: ${paths.join(", ")}`;
}

/** guard.ts: the redacted buildPack state, as JSON. */
export function cliGuardState(tool: string, input: Record<string, unknown>, cwd?: string): string {
  const st = buildPack({ tool_name: tool, tool_input: input ?? {}, ...(cwd ? { cwd } : {}) }).request.state;
  return typeof st === "string" ? st : JSON.stringify(st);
}
