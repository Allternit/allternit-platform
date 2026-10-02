// Claude Code transcripts (~/.claude/projects/**/*.jsonl) -> labelled S1 items.
//   ROUTE        backfill_observed: the turn's actual tool calls, labelled by the live routeLabel rule.
//   ROUTE_MODEL  backfill_observed: the person's next move (accept / exact retry / model switch), live rule.
//   permission   observed: a human rejected the call, or a human-authored deny rule blocked it (-> false);
//                a call that ran while the session asked before acting (default / acceptEdits / plan) -> true.
//                Calls in bypassPermissions mode carry no human judgement and are never labelled.
//   judge.first_pass.tool  same human allow/deny evidence as the permission rows, in the judge's state
//                shape (backends.rs tool_decision): the live first pass joins tool-call outcomes to the
//                judge by tool_call_id for exactly this label.
//   vendor_consequential   per turn (Claude is the vendor here): a turn whose call was human-denied
//                needed approval (-> true, observed); a turn that ran without an ask in a non-bypass
//                mode is the harness's own flag (-> false, backfill, x-incumbent = that flag);
//                bypass-mode turns carry no judgement and are skipped (tighten-only bank).
//   dec.classify_error     Bash outputs carrying a deterministic error signature, labelled by the
//                verbatim port of executor.rs s0_error_code (the live bug_fix.s0 labeler).
// Sidechain (subagent) transcripts are skipped: their "prompts" are agent-written.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { genClassOf, routeLabel, routeModelLabel, s0ErrorCode } from "../labels.ts";
import { BANKS, classifyErrorState, cliGuardState, judgeToolState, permissionState, routeState, vendorConsequentialState } from "../banks.ts";
import type { HarvestItem, HarvestSource } from "../types.ts";

const NOT_A_PROMPT = /^\s*(<command-name>|<command-message>|<local-command|<task-notification>|<system-reminder>|<bash-input>|<bash-stdout>|<user-memory-input>|Caveat:|\[Request interrupted|This session is being continued)/;

/** Tools whose tool_result text is command/test output (executor.rs runs Bash). */
const OUTPUT_TOOL = /^(bash|shell|cmd|terminal|exec|powershell|zsh)$/i;
/** Failure context guard: the live caller only classifies failing runs, not every output. */
const FAILURE_CTX = /fail|error|panic|exception|abort/i;

interface Turn { uuid: string; ts: string; text: string; tools: string[]; model: string | null; errored: boolean; interrupted: boolean; mode: string | null; denied: boolean }
interface ToolCall { id: string; name: string; input: Record<string, unknown>; ts: string; mode: string | null; cwd?: string }
interface ToolResult { denial: string | null; ts: string; content: string; isError: boolean }

export function* walkJsonl(root: string, maxDepth = 3): Generator<string> {
  let ents: string[];
  try { ents = readdirSync(root); } catch { return; }
  for (const e of ents) {
    const p = join(root, e);
    let st;
    try { st = statSync(p); } catch { continue; }
    if (st.isDirectory()) { if (maxDepth > 0 && e !== "subagents") yield* walkJsonl(p, maxDepth - 1); }
    else if (e.endsWith(".jsonl")) yield p;
  }
}

function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((b) => (typeof b?.text === "string" ? b.text : typeof b === "string" ? b : "")).join("\n");
  return "";
}

function promptText(content: unknown): string | null {
  if (typeof content === "string") return NOT_A_PROMPT.test(content) || !content.trim() ? null : content;
  if (!Array.isArray(content)) return null;
  if (content.some((b) => b?.type === "tool_result")) return null;
  const text = content.filter((b) => b?.type === "text").map((b) => String(b.text ?? "")).join("\n");
  return !text.trim() || NOT_A_PROMPT.test(text) ? null : text;
}

/** Parse one transcript into turns and tool calls with their permission outcome. */
export function parseTranscript(lines: string[]) {
  const turns: Turn[] = [];
  const calls = new Map<string, ToolCall>();
  const results = new Map<string, ToolResult>();
  let mode: string | null = null;
  let sessionId = "";
  for (const line of lines) {
    if (!line.trim()) continue;
    let r: any;
    try { r = JSON.parse(line); } catch { continue; }
    if (r.isSidechain) continue;
    if (r.sessionId) sessionId = r.sessionId;
    if (typeof r.permissionMode === "string") mode = r.permissionMode;
    const cur = turns[turns.length - 1];
    if (r.type === "user") {
      const content = r.message?.content;
      if (Array.isArray(content)) {
        for (const b of content) {
          if (b?.type === "tool_result" && b.tool_use_id) {
            results.set(b.tool_use_id, { denial: r.toolDenialKind ?? null, ts: r.timestamp, content: resultText(b.content), isError: b.is_error === true });
            if (r.toolDenialKind && cur) cur.denied = true;
          }
          if (b?.type === "text" && /^\[Request interrupted/.test(String(b.text)) && cur) cur.interrupted = true;
        }
      } else if (typeof content === "string" && /^\[Request interrupted/.test(content) && cur) cur.interrupted = true;
      if (r.isMeta || r.isCompactSummary) continue;
      const text = promptText(content);
      if (text !== null && r.uuid && r.timestamp) turns.push({ uuid: r.uuid, ts: r.timestamp, text, tools: [], model: null, errored: false, interrupted: false, mode, denied: false });
    } else if (r.type === "assistant" && cur) {
      if (r.isApiErrorMessage) cur.errored = true;
      if (r.message?.model && r.message.model !== "<synthetic>") cur.model = r.message.model;
      for (const b of r.message?.content ?? []) {
        if (b?.type !== "tool_use") continue;
        cur.tools.push(String(b.name));
        calls.set(b.id, { id: b.id, name: String(b.name), input: b.input ?? {}, ts: r.timestamp, mode, cwd: r.cwd });
      }
    }
  }
  return { sessionId, turns, calls, results };
}

export function itemsFromTranscript(lines: string[], fileKey: string): HarvestItem[] {
  const { sessionId, turns, calls, results } = parseTranscript(lines);
  const sid = sessionId || fileKey;
  const out: HarvestItem[] = [];
  turns.forEach((t, i) => {
    if (t.interrupted) return; // the turn never finished: its tool list is not what it needed
    const state = routeState(t.text);
    out.push({ spec: BANKS.route, key: `${sid}:${t.uuid}`, source: "claude-code", ts: t.ts, state, truth: routeLabel(t.tools), label_source: "backfill_observed", outcome_source: "replay:claude-code.turn_tools" });
    const cls = genClassOf(t.model);
    if (cls) {
      const n = turns[i + 1];
      const lbl = routeModelLabel({ text: t.text, cls, errored: t.errored, requested: t.model ?? "" }, n ? { text: n.text, cls: genClassOf(n.model), requested: n.model ?? t.model ?? "" } : null);
      out.push({ spec: BANKS.route_model, key: `${sid}:${t.uuid}`, source: "claude-code", ts: t.ts, state, truth: lbl.truth, label_source: "backfill_observed", outcome_source: `replay:claude-code.${lbl.source}`, incumbent: cls });
    }
    // vendor consequential (gateway_routing.rs): a human denial observed -> the turn
    // needed approval; a turn that ran in a non-bypass mode without an ask is the
    // harness caller's own flag (false); bypass turns carry no judgement.
    if (t.mode !== "bypassPermissions") {
      const truth = t.denied ? "true" : "false";
      out.push({
        spec: BANKS.vendor_consequential, key: `${sid}:${t.uuid}:vc`, source: "claude-code", ts: t.ts,
        state: vendorConsequentialState(t.text), truth, incumbent: truth,
        label_source: t.denied ? "observed" : "backfill_observed",
        outcome_source: t.denied ? "human:claude-code.user_rejected" : "replay:claude-code.harness_no_ask",
      });
    }
  });
  // Permission + judge first pass: the live cli_hook labelers (post_tool_use -> true,
  // permission_denied -> false), replayed. A human rejection or a human-authored deny rule is `observed`;
  // a call that ran while the session asked before acting is `observed` (the person allowed it); a call that
  // ran in bypassPermissions mode is the live post_tool_use label replayed (`backfill_observed`,
  // capped in its own bucket so it cannot crowd out the rare human labels).
  for (const c of calls.values()) {
    const res = results.get(c.id);
    if (!res) continue;
    let truth: string, src: string, ls: HarvestItem["label_source"] = "observed", capGroup: string | undefined;
    if (res.denial === "user-rejected") { truth = "false"; src = "human:claude-code.user_rejected"; }
    else if (res.denial === "permission-rule") { truth = "false"; src = "human:claude-code.deny_rule"; }
    else if (res.denial) continue; // interrupted: no judgement
    else if (c.mode && c.mode !== "bypassPermissions") { truth = "true"; src = `human:claude-code.ran_in_${c.mode}`; }
    else { truth = "true"; src = "replay:claude-code.post_tool_use"; ls = "backfill_observed"; capGroup = "bypass"; }
    const base = { key: `${sid}:${c.id}`, source: "claude-code", ts: c.ts, truth, label_source: ls, outcome_source: src, ...(capGroup ? { capGroup } : {}) };
    out.push({ ...base, spec: BANKS.permission_judge, state: permissionState(c.name, c.input) });
    out.push({ ...base, spec: BANKS.permission_cli_guard, state: cliGuardState(c.name, c.input, c.cwd) });
    out.push({ ...base, spec: BANKS.judge_tool, state: judgeToolState(c.name, c.input, null) });
    // dec.classify_error: Bash output with a failure context, labelled by the
    // verbatim s0_error_code port (the same deterministic labeler as bug_fix.s0).
    // Denied calls never ran: their "result" is the rejection notice, not output.
    if (!res.denial && OUTPUT_TOOL.test(c.name) && (res.isError || FAILURE_CTX.test(res.content))) {
      const code = s0ErrorCode(res.content);
      if (code !== "UNKNOWN" || res.isError) {
        // x-incumbent = the deterministic S0 diagnostic's own answer (Q26 non-inferiority vs S0).
        out.push({ spec: BANKS.classify_error, key: `${sid}:${c.id}:err`, source: "claude-code", ts: res.ts, state: classifyErrorState(res.content), truth: code, label_source: "backfill_observed", outcome_source: "replay:bug_fix.s0", incumbent: code });
      }
    }
  }
  return out;
}

export function claudeCodeSource(root: string): HarvestSource {
  return {
    name: "claude-code",
    *items() {
      for (const f of walkJsonl(root)) {
        let text: string;
        try { text = readFileSync(f, "utf8"); } catch { continue; }
        yield* itemsFromTranscript(text.split("\n"), f.slice(root.length));
      }
    },
  };
}
