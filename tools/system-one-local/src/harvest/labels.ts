// Label rules ported VERBATIM from the live labelers so backfilled labels mean
// exactly what live labels mean. Keep in sync with:
//   cmd/gizzi-code/src/runtime/routing/turn-router.ts  routeLabel / labelPreviousRouteModel / escalate
export const ROUTE_OPTIONS = ["answer_from_memory", "retrieval", "single_tool", "agent_run", "coding", "computer_use", "template", "clarify"] as const;
export type RouteOption = (typeof ROUTE_OPTIONS)[number];
export const GEN_CLASSES = ["gen.small", "gen.standard", "gen.deep"] as const;
export type GenClass = (typeof GEN_CLASSES)[number];

const TEMPLATE_TOOL = /(run|use|apply|exec|execute|start)[_.\-]?templates?($|[_.\-])|templates?[_.\-](run|use|apply|exec|execute|start)/;

/** turn-router.ts routeLabel: what the turn actually needed, from its tool calls. */
export function routeLabel(tools: string[], opts: { template?: boolean } = {}): RouteOption {
  const t = tools.map((x) => x.toLowerCase());
  const has = (re: RegExp) => t.some((x) => re.test(x));
  if (opts.template || has(TEMPLATE_TOOL)) return "template";
  if (t.length === 0) return "answer_from_memory";
  if (has(/^(question|ask_?user)/)) return "clarify";
  if (has(/computer|browser_|desktop|screenshot|mouse|keyboard/)) return "computer_use";
  if (has(/^(edit|write|multiedit|patch|apply_patch)$/)) return "coding";
  if (has(/^(task|agent|subagent)$/) || t.length > 3) return "agent_run";
  if (t.length === 1 && !has(/^(read|grep|glob|list|ls|webfetch|websearch|search|memory)/)) return "single_tool";
  if (t.every((x) => /^(read|grep|glob|list|ls|webfetch|websearch|search|memory|codesearch)/.test(x))) return "retrieval";
  return t.length === 1 ? "single_tool" : "agent_run";
}

/** turn-router.ts escalate: one class up. */
export function escalate(c: GenClass): GenClass {
  return c === "gen.small" ? "gen.standard" : "gen.deep";
}

/**
 * The logical class of a concrete model id. Mirrors the pool's tiering (small /
 * standard / deep); null when the id is unknown, and such turns get no ROUTE_MODEL label.
 */
export function genClassOf(model: string | null | undefined): GenClass | null {
  const m = (model ?? "").toLowerCase();
  if (!m || m === "<synthetic>") return null;
  if (/haiku|mini|nano|flash|small|8b|7b|3b|1b/.test(m)) return "gen.small";
  if (/opus|fable|o1|o3|pro|ultra|deep|reason|405b|70b|k2/.test(m)) return "gen.deep";
  if (/sonnet|gpt|grok|gemini|kimi|qwen|llama|mistral|deepseek|glm/.test(m)) return "gen.standard";
  return null;
}

/**
 * turn-router.ts labelPreviousRouteModel: ROUTE_MODEL truth for a turn from the
 * person's next move (switch model -> that class or one up; exact retry -> one up;
 * otherwise the incumbent was accepted). `next` null = the session's last turn.
 */
export function routeModelLabel(cur: { text: string; cls: GenClass; errored: boolean; requested: string }, next: { text: string; cls: GenClass | null; requested: string } | null): { truth: GenClass; source: string } {
  if (cur.errored) return { truth: escalate(cur.cls), source: "turn_error" };
  if (!next) return { truth: cur.cls, source: "session_end" };
  if (next.requested !== cur.requested) return { truth: next.cls ?? escalate(cur.cls), source: "user_model_switch" };
  if (next.text.trim() !== "" && next.text.trim() === cur.text.trim()) return { truth: escalate(cur.cls), source: "user_retry" };
  return { truth: cur.cls, source: "turn_accepted" };
}

/**
 * agency_api/executor.rs s0_error_code, ported VERBATIM (same precedence, same
 * keyword set, same UNKNOWN fallback): the deterministic S0 diagnostic is the
 * ground truth for dec.classify_error, exactly as bug_fix.s0 reports it live.
 */
export function s0ErrorCode(out: string): string {
  const o = out.toLowerCase();
  const has = (ks: string[]) => ks.some((k) => o.includes(k));
  if (has(["syntaxerror", "syntax error"])) return "SYNTAX_ERROR";
  if (has(["modulenotfounderror", "importerror", "cannot find module"])) return "IMPORT_ERROR";
  if (has(["typeerror"])) return "TYPE_ERROR";
  if (has(["assertionerror", "assertion failed", "expected"])) return "TEST_ASSERTION";
  if (has(["timed out", "timeout"])) return "TEST_TIMEOUT";
  return "UNKNOWN";
}
